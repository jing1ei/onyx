//! Onyx — Tauri 2 application layer.
//!
//! The audio engine lives in `onyx-core`; this crate is the shell: state,
//! commands, events, and the platform plumbing that turns "open with Onyx" into
//! "play this file now".
//!
//! # Diagnostics
//!
//! One logger, installed here at startup by `tauri-plugin-log`, is the only sink
//! in the product:
//!
//! * a rotating file in the platform log directory (see [`LOG_FILE_STEM`]) — the
//!   file you ask a user to attach to a bug report;
//! * stderr as well, but only in debug builds: a shipped desktop app is not
//!   attached to a terminal, so writing there is a no-op with a cost;
//! * `warn` and above by default, overridable with `ONYX_LOG` (see
//!   [`level_from_env`]) without a rebuild.
//!
//! Two things feed it from outside this crate's own `log::` calls: the real-time
//! fault counters, drained by the frame thread (see [`crate::frame`]), and the
//! webview's own errors (see [`crate::clientlog`]).

/// The A/B assignment rule. Public so `tests/ab_assign_contract.rs` can hold it
/// against the fixture the front-end mock backend is checked against too.
pub mod abrules;
/// The native Appearance menu — the escape hatch of SPEC §20.
mod appmenu;
/// Public so the hostile-archive corpus in `tests/hostile_archives.rs` can
/// drive exactly the entry points the open path uses.
pub mod archive;
mod blind;
mod cache;
mod clientlog;
mod commands;
mod eqwindow;
mod frame;
mod loader;
mod persist;
mod playlist;
/// Public so the hostile-input corpus in `tests/malformed_input.rs` can drive
/// exactly the entry points the load path uses.
pub mod safe_decode;
mod settings;
mod state;
/// The window *surface* — the colour the window manager paints under the
/// webview. Public to the crate only; the parity checks that tie it to the
/// theme live inside it.
mod surface;
mod themewindow;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use log::LevelFilter;
use onyx_core::engine::{AudioEngine, EngineConfig, FALLBACK_RATE};
use tauri::{AppHandle, Manager, RunEvent};
use tauri_plugin_log::{RotationStrategy, Target, TargetKind, TimezoneStrategy};

use crate::cache::LoudnessCache;
use crate::playlist::ProbePool;
use crate::settings::{Settings, SettingsStore};
use crate::state::{AppState, MAIN_WINDOW};

/// Log file name (the plugin appends `.log`), in the platform log directory:
/// `~/Library/Logs/com.onyxaudio.player/` on macOS,
/// `%LOCALAPPDATA%\com.onyxaudio.player\logs\` on Windows,
/// `$XDG_DATA_HOME/com.onyxaudio.player/logs/` on Linux.
const LOG_FILE_STEM: &str = "onyx";
/// Environment variable that overrides the level, e.g. `ONYX_LOG=debug`.
const LOG_LEVEL_ENV: &str = "ONYX_LOG";
/// Rotate at 4 MiB and keep two older files. Enough history to cover a whole
/// listening session; small enough that nothing can eat a disk.
const MAX_LOG_BYTES: u128 = 4 * 1024 * 1024;
const KEEP_LOG_FILES: usize = 2;

/// Resolve the level filter from `ONYX_LOG`.
///
/// The default is deliberately `Warn`: a shipping player must not write a line
/// per frame to a user's disk for ever, and everything above `warn` is
/// something a user might reasonably be asked about. `ONYX_LOG=info` adds the
/// lifecycle (device, rate, loads), `debug`/`trace` add the detail.
///
/// An unusable value is *not* a startup failure — the app is a music player, not
/// a log level validator — but it is reported once the logger is up, which is
/// why the complaint comes back as a string rather than being logged here.
fn level_from_env(raw: Option<&str>) -> (LevelFilter, Option<String>) {
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return (LevelFilter::Warn, None);
    };
    match raw.to_ascii_lowercase().parse::<LevelFilter>() {
        Ok(level) => (level, None),
        Err(_) => (
            LevelFilter::Warn,
            Some(format!(
                "{LOG_LEVEL_ENV}=\"{raw}\" is not a log level \
                 (off, error, warn, info, debug, trace); using warn"
            )),
        ),
    }
}

/// Build the logging plugin: rotating file always, stderr in debug builds.
fn log_plugin(level: LevelFilter) -> tauri::plugin::TauriPlugin<tauri::Wry> {
    let mut targets = vec![Target::new(TargetKind::LogDir {
        file_name: Some(LOG_FILE_STEM.into()),
    })];
    // `tauri dev` runs in a terminal and a developer wants the lines there too.
    // A packaged build has no stderr worth writing to, so it does not pay for
    // the formatting.
    //
    // `if cfg!(…)` rather than `#[cfg(…)]`: the attribute form made `targets`
    // provably immutable in release builds, which is an `unused_mut` warning
    // that only ever appeared in `cargo build --release`. This form compiles the
    // push in both profiles and lets the optimiser drop the dead branch.
    if cfg!(debug_assertions) {
        targets.push(Target::new(TargetKind::Stderr));
    }

    tauri_plugin_log::Builder::new()
        .clear_targets()
        .targets(targets)
        .level(level)
        // Timestamps in a log file are compared against other machines' logs and
        // against wall-clock reports from users; local time without an offset is
        // ambiguous, UTC is not.
        .timezone_strategy(TimezoneStrategy::UseUtc)
        .max_file_size(MAX_LOG_BYTES)
        .rotation_strategy(RotationStrategy::KeepSome(KEEP_LOG_FILES))
        .build()
}

/// Build the engine, preferring the remembered host/device/rate/buffer but
/// never refusing to start because that device has been unplugged.
///
/// The second half of the pair is a message for the *user*: silently playing
/// through the laptop speakers when someone expects their interface is a
/// mastering-session-ruining surprise, and a log line nobody opened is not
/// telling them. `run` queues it (see [`AppState::warn_at_startup`]) so it is
/// toasted once the webview is listening.
///
/// A remembered *host* that is gone does not need handling here: `resolve_host`
/// in the core already falls back to the platform default with a warning, so a
/// settings file written on a machine with ASIO still makes a sound on one
/// without it.
fn build_engine(settings: &Settings) -> Result<(Arc<AudioEngine>, Option<String>), String> {
    let wanted = engine_config(settings, settings.device.clone());
    match AudioEngine::new(wanted) {
        Ok(engine) => Ok((engine, None)),
        Err(first) => {
            if let Some(device) = settings.device.as_deref() {
                log::warn!(
                    "remembered output \"{device}\" is unavailable ({first}); \
                     falling back to the system default"
                );
                let engine = AudioEngine::new(engine_config(settings, None))
                    .map_err(|e| format!("no usable audio output device: {e}"))?;
                Ok((
                    engine,
                    Some(format!(
                        "“{device}” is not available — playing through the system default output"
                    )),
                ))
            } else {
                Err(format!("no usable audio output device: {first}"))
            }
        }
    }
}

/// Translate the persisted preferences into an [`EngineConfig`].
///
/// `fallback_rate` carries two meanings in the core — the rate to fall back to
/// while following the source, and the *fixed* rate when not following — which
/// is why a stored `sample_rate` is only honoured in the second case.
fn engine_config(settings: &Settings, device_name: Option<String>) -> EngineConfig {
    EngineConfig {
        host_id: settings.host_id.clone(),
        device_name,
        follow_source_rate: settings.follow_source_rate,
        fallback_rate: settings.sample_rate.unwrap_or(FALLBACK_RATE),
        buffer_frames: settings.buffer_frames,
    }
}

/// Check the configured SoundFont at startup, returning what to tell the user
/// if it cannot be used (SPEC §18).
///
/// `None` means "nothing to say": no user bank configured, or the one that is
/// loads. Loading a bank costs a parse of the file, so this runs only when the
/// user has actually chosen one, and its result is cached by the core for the
/// first MIDI file that follows.
fn soundfont_complaint(state: &AppState) -> Option<String> {
    let path = state.settings.lock().soundfont_path()?;
    let opts = onyx_core::midi::MidiOptions {
        soundfont: Some(path.clone()),
    };
    match onyx_core::midi::load_bank(&opts) {
        Ok((_, bank)) => bank.fallback_reason.map(|reason| {
            format!(
                "The SoundFont “{}” cannot be used ({reason}) — MIDI will play through the \
                 bundled General MIDI bank",
                path.file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.to_string_lossy().to_string())
            )
        }),
        Err(e) => Some(format!(
            "No General MIDI SoundFont could be loaded ({e}) — MIDI files will not play"
        )),
    }
}

/// Resolve a per-app directory, creating it if need be.
///
/// `None` means "we have nowhere to persist to": the app still runs, settings
/// and the loudness cache just stay in memory. That is strictly better than
/// refusing to start a player because a config directory could not be made.
fn ensure_dir(what: &str, dir: Result<PathBuf, tauri::Error>) -> Option<PathBuf> {
    match dir {
        Ok(dir) => match std::fs::create_dir_all(&dir) {
            Ok(()) => Some(dir),
            Err(e) => {
                log::warn!(
                    "could not create the {what} directory {} ({e}); \
                     continuing without persistence",
                    dir.display()
                );
                None
            }
        },
        Err(e) => {
            log::warn!(
                "no {what} directory on this platform ({e}); continuing without persistence"
            );
            None
        }
    }
}

/// Files the OS handed us before the engine existed. Drained by `setup`.
///
/// On macOS, double-clicking a file in Finder *launches* the app and delivers
/// `RunEvent::Opened` for it, and there is no guarantee that lands after the
/// setup hook has managed the state — the file the user asked for was simply
/// logged and dropped. Queueing costs one lock on a path that runs at most a
/// handful of times per launch.
static PENDING_OPEN: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// SPEC §2 rule 1: anything the OS hands us replaces the playlist and plays.
fn open_from_os(app: &AppHandle, paths: Vec<String>) {
    if paths.is_empty() {
        return;
    }
    let Some(state) = app.try_state::<Arc<AppState>>() else {
        log::info!(
            "{} file(s) arrived from the OS before the engine was ready; queued",
            paths.len()
        );
        if let Ok(mut pending) = PENDING_OPEN.lock() {
            pending.extend(paths);
        }
        return;
    };
    let state = Arc::clone(state.inner());
    // Loading touches the disk and re-clocks the device: never on the UI thread.
    let spawned = std::thread::Builder::new()
        .name("onyx-os-open".into())
        .spawn(move || {
            if let Err(e) = loader::open_paths(&state, paths, true) {
                state.error(e);
            }
        });
    if let Err(e) = spawned {
        // The user asked for a file and will not get it, so this is a failure
        // and it has to be visible in the window, not only in the log.
        log::error!("could not start the open handler thread: {e}");
        if let Some(state) = app.try_state::<Arc<AppState>>() {
            state.error("Could not open that file: the system refused a new thread");
        }
    }
}

/// Bring the one window back to the front (second instance / re-open).
fn focus_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Push the user's theme choice at everything about a window the *webview* does
/// not draw, on every window: the frame's theme and the window's own surface
/// colour.
///
/// The webview is themed by the token layer (`src/lib/theme.ts`), but the parts
/// of a window the webview does not draw — the macOS traffic lights and their
/// title bar, the Windows caption, native scrollbars, the
/// `prefers-color-scheme` a webview reports to CSS, and the window background
/// the window manager paints *under* the webview — are the window manager's.
/// Neither `tauri.conf.json` nor the macOS override may pin a `theme` for them:
/// a hard-coded window theme fights `theme: "light"` and defeats
/// `theme: "system"` outright, which is the whole point of having the setting.
/// So the config leaves it unset (= follow the OS) and the choice is applied
/// here instead, once at startup and again whenever it changes.
///
/// `None` means "follow the OS", which is exactly [`settings::Theme::System`].
///
/// The second half is the window *surface*: an untold window keeps the system's
/// own `windowBackgroundColor`, which is never one of Onyx's two themes, and
/// shows as a light rim around the rounded corners of a dark window and as a
/// grey flash before the first webview frame. [`surface::apply`] resolves the
/// theme's real background — including a theme document's (SPEC §20) — and
/// paints it on every window. It is deliberately in this function and not
/// beside its call sites: the frame theme and the surface must never be
/// retargeted separately, or one of them drifts.
///
/// Platform note: on Linux the *theme* half is a no-op in `tauri`/`tao` — the
/// theme is not per-window there and GTK takes it from the desktop — so on the
/// machine this was developed on nothing observable happens. It is still the
/// correct call to make: it is what carries the setting to the macOS and
/// Windows frames.
pub(crate) fn apply_native_appearance(app: &AppHandle, theme: settings::Theme) {
    let native = match theme {
        settings::Theme::Dark => Some(tauri::Theme::Dark),
        settings::Theme::Light => Some(tauri::Theme::Light),
        settings::Theme::System => None,
    };
    for (label, window) in app.webview_windows() {
        if let Err(e) = window.set_theme(native) {
            // Cosmetic, and unsupported on some platforms: never a failure.
            log::debug!("could not set the window theme on `{label}`: {e}");
        }
    }
    surface::apply(app, theme);
}

/// Files named on the command line. `cargo run -- file.wav`, `Onyx.exe x.flac`,
/// and the Windows/Linux "open with" path all arrive here.
///
/// `cwd` is the working directory the arguments were written in — the *second*
/// instance's, when the single-instance plugin forwards them, which is not this
/// process's. A relative path resolved against the wrong directory opens the
/// wrong file or none at all, so it is resolved here.
///
/// `OsString` in, `String` out: `std::env::args()` *panics* when an argument is
/// not valid Unicode, which on Linux (and, with unpaired surrogates, on
/// Windows) is a file name a user can really have. A path that cannot be
/// represented as UTF-8 cannot cross the JSON IPC boundary into the playlist
/// either, so it is refused with a log line rather than crashing the launch.
fn files_from_args<I: IntoIterator<Item = OsString>>(args: I, cwd: &Path) -> Vec<String> {
    args.into_iter()
        .skip(1)
        .filter_map(|arg| match arg.into_string() {
            Ok(arg) => Some(arg),
            Err(lossy) => {
                log::warn!(
                    "ignoring a command-line argument that is not valid UTF-8: {}",
                    lossy.to_string_lossy()
                );
                None
            }
        })
        // Skip switches and the WebView2/Chromium flags Tauri may add.
        .filter(|a| !a.starts_with('-') && a != "--")
        .map(|arg| {
            let path = Path::new(&arg);
            if path.is_absolute() {
                arg
            } else {
                // `join` is a no-op for a Windows drive-relative path like
                // `C:file.wav`, which is the correct answer here: only the
                // process that owns that drive's cwd could resolve it.
                cwd.join(path).to_string_lossy().to_string()
            }
        })
        .collect()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let (level, level_complaint) = level_from_env(std::env::var(LOG_LEVEL_ENV).ok().as_deref());
    let mut builder = tauri::Builder::default();

    // A second launch (including every "open with" on Windows, which really
    // does start a new process) forwards its argv to the running instance
    // instead of starting a second engine.
    //
    // Registered before every other plugin on purpose: the second instance
    // exits from inside this plugin's setup, and anything registered earlier
    // would first have initialised itself against the same files the running
    // instance is using — the log plugin would try to rotate a log file the
    // first instance holds open.
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
            focus_main_window(app);
            let argv = argv.into_iter().map(OsString::from);
            open_from_os(app, files_from_args(argv, Path::new(&cwd)));
        }));
    }

    let mut builder = builder.plugin(log_plugin(level));

    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    {
        builder = builder.plugin(tauri_plugin_window_state::Builder::default().build());
    }

    let app = builder
        // Just these two, and both only for what Rust asks them to do: the
        // native file picker (SPEC §2) and "Reveal in Finder" (§5.4).
        // `tauri-plugin-fs` used to be registered as well, which added its
        // whole command surface to the invoke table for nothing — Onyx reads
        // files with `std::fs` from Rust, the webview never touches the disk,
        // and the dialog plugin only consults the fs scope from its own
        // webview commands, which are not reachable here.
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            commands::app_state,
            commands::open_files,
            commands::pick_and_open_files,
            commands::playlist_play_index,
            commands::playlist_play_entry,
            commands::playlist_remove,
            commands::playlist_clear,
            commands::playlist_move,
            commands::playlist_next,
            commands::playlist_prev,
            commands::transport_toggle,
            commands::transport_play,
            commands::transport_pause,
            commands::transport_stop,
            commands::transport_seek,
            commands::transport_nudge,
            commands::set_volume,
            commands::set_muted,
            commands::set_loop_enabled,
            commands::set_loop_region,
            commands::set_monitor_mode,
            commands::ab_set_enabled,
            commands::ab_select,
            commands::ab_toggle_deck,
            commands::ab_assign,
            commands::ab_set_crossfade_ms,
            commands::set_level_match,
            commands::set_ab_offset,
            commands::auto_align_ab,
            commands::set_deck_invert,
            commands::blind_start,
            commands::blind_switch,
            commands::blind_vote,
            commands::blind_abort,
            commands::set_eq,
            commands::set_eq_audition,
            commands::set_spectrum_enabled,
            commands::eq_window_open,
            commands::eq_window_close,
            commands::eq_window_toggle,
            commands::eq_window_set_pinned,
            commands::eq_window_state,
            commands::waveform_get,
            commands::devices_list,
            commands::device_set,
            commands::set_follow_source_rate,
            commands::audio_hosts,
            commands::audio_devices,
            commands::audio_source,
            commands::audio_source_set,
            commands::soundfont_get,
            commands::soundfont_set,
            commands::pick_soundfont,
            commands::set_appearance,
            commands::set_theme_doc,
            commands::set_window_surface,
            commands::reset_appearance,
            commands::theme_window_open,
            commands::theme_window_close,
            commands::theme_window_toggle,
            commands::theme_window_state,
            commands::meters_get,
            commands::reset_meters,
            commands::cache_stats,
            commands::cache_clear,
            commands::reveal_in_finder,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            if let Some(complaint) = &level_complaint {
                // Deliberately here and not in `level_from_env`: there was no
                // logger when that ran.
                log::warn!("{complaint}");
            }
            log::info!(
                "Onyx {} starting ({}, log level {level})",
                env!("CARGO_PKG_VERSION"),
                std::env::consts::OS
            );
            // Front-end errors, unhandled rejections and error-boundary
            // failures end up in the same file, marked as coming from the
            // webview.
            clientlog::install(&handle);
            // SPEC §8/§12: settings live in the config dir, the loudness
            // cache in the cache dir (it is regenerable, so it must not be
            // backed up or synced with the settings).
            // SPEC §19: destructors do not run when a process is killed,
            // so an unlucky crash leaves an album of extracted WAV in the temp
            // directory. Sweeping is a directory listing and happens before
            // anything else can create one of ours.
            let swept = archive::sweep_stale();
            if swept > 0 {
                log::info!("removed {swept} archive temp folder(s) left by a previous run");
            }
            let config_dir = ensure_dir("config", handle.path().app_config_dir());
            let cache_dir = ensure_dir("cache", handle.path().app_cache_dir());
            let (settings, store) = SettingsStore::open(config_dir.as_deref());
            let cache = LoudnessCache::open(cache_dir.as_deref());
            let (engine, device_complaint) = build_engine(&settings).map_err(|e| {
                // Without an output device there is nothing to be a player of.
                log::error!("{e}");
                e
            })?;

            let state = Arc::new(AppState::new(engine, settings, store, cache));
            state.set_app(handle.clone());
            state.apply_settings();
            let _ = state.probes.set(ProbePool::spawn(Arc::clone(&state)));
            app.manage(Arc::clone(&state));

            // Startup facts the user has to know about. They are queued rather
            // than toasted: there is no webview listening yet, so an event
            // emitted here would be logged and then dropped on the floor. The
            // frontend's first `app_state` call flushes the queue.
            if let Some(message) = device_complaint {
                state.warn_at_startup(message);
            }
            if config_dir.is_none() {
                state.warn_at_startup(
                    "No writable settings folder — your preferences will be forgotten on exit",
                );
            }
            // SPEC §18: a `.sf2` that has been moved or deleted since it was
            // chosen must be reported, not silently swapped for the bundled
            // bank on the first MIDI file. Only paid for when one is set — and
            // paid for on a thread of its own, because the check is a full
            // parse of a file the user chose and could be hundreds of
            // megabytes on a network share. The window must not wait for it;
            // `warn_at_startup` toasts directly if it arrives after the queue
            // has been flushed.
            if state.settings.lock().soundfont_path().is_some() {
                let for_bank = Arc::clone(&state);
                std::thread::Builder::new()
                    .name("onyx-soundfont-check".to_string())
                    .spawn(move || {
                        if let Some(complaint) = soundfont_complaint(&for_bank) {
                            for_bank.warn_at_startup(complaint);
                        }
                    })
                    .ok();
            }

            let handle_for_windows = handle.clone();
            frame::spawn(Arc::clone(&state), handle);

            // Closing the main window quits the app even with the EQ window
            // open, and the EQ window comes back if it was open at exit
            // (SPEC §12). Both live in `eqwindow`; the order matters only in
            // that the main window must be watched before a second window can
            // exist.
            eqwindow::watch_main(&handle_for_windows);
            eqwindow::restore(&handle_for_windows);
            // SPEC §20: the theme editor is a third window, and closing the
            // main one must take it with it or the app cannot quit. The menu
            // is the escape hatch from a theme that has made the UI
            // unreadable; on macOS it is app-wide and installed here, on
            // Windows and Linux it hangs off the editor window itself.
            themewindow::watch_main(&handle_for_windows);
            appmenu::install(&handle_for_windows);
            // The window frames *and* the window surfaces follow the persisted
            // theme, not the config: see `apply_native_appearance`. After
            // `restore`, so the EQ window is included when it comes back open,
            // and before the run loop starts, so the first frame the user sees
            // is the theme's colour rather than the system's window grey.
            //
            // The guard is released *before* the call rather than living inside
            // its argument list, which is where a temporary would keep it. It is
            // defensive, not a fix: nothing under `apply_native_appearance`
            // takes `state.settings` today, but `surface` does read it
            // (`settings_theme`, for the paths that resolve a theme for
            // themselves) and `parking_lot::Mutex` is not reentrant, so the
            // shape that would deadlock is one edit away.
            let persisted_theme = state.settings.lock().appearance.theme;
            apply_native_appearance(&handle_for_windows, persisted_theme);
            // …and again whenever macOS or Windows changes appearance under a
            // `system` theme, which is the one theme change no command carries.
            surface::watch_os_appearance(&handle_for_windows);
            if eqwindow::is_open(&handle_for_windows) {
                // The window-state plugin shows and focuses every window it
                // restores, so the tool window would otherwise be the one
                // holding the keyboard on launch.
                focus_main_window(&handle_for_windows);
            }

            // First launch with files: the command line, plus anything macOS
            // delivered through `RunEvent::Opened` before this point.
            // `args_os`, not `args`: see `files_from_args`.
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let mut files = files_from_args(std::env::args_os(), &cwd);
            if let Ok(mut pending) = PENDING_OPEN.lock() {
                files.append(&mut pending);
            }
            if !files.is_empty() {
                log::info!("opening {} file(s) handed over at launch", files.len());
                let opener = Arc::clone(&state);
                let spawned = std::thread::Builder::new()
                    .name("onyx-cli-open".into())
                    .spawn(move || {
                        if let Err(e) = loader::open_paths(&opener, files, true) {
                            opener.error(e);
                        }
                    });
                if let Err(e) = spawned {
                    // Not fatal, and it used to be: `?` here turned "the system
                    // would not give us one more thread" into "Onyx will not
                    // start at all", losing the whole window over a file the
                    // user can still open from the menu.
                    log::error!("could not start the command-line open thread: {e}");
                    state.error("Could not open the files given on the command line");
                }
            }
            Ok(())
        })
        .build(tauri::generate_context!());

    let app = match app {
        Ok(app) => app,
        Err(e) => {
            // Pre-logger on purpose. `build()` is what installs the plugins, so
            // if it failed there may be no logger and no log file to look in —
            // and this is the one message that has to survive that. Everything
            // after this point goes through `log::`.
            eprintln!("Onyx could not start: {e}");
            std::process::exit(1);
        }
    };

    app.run(|app, event| match event {
        // macOS: Finder "Open with", dropping files on the Dock icon, and
        // double-clicking an associated file while Onyx is already running.
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        RunEvent::Opened { urls } => {
            let paths: Vec<String> = urls
                .iter()
                .filter_map(|url| {
                    if url.scheme() == "file" {
                        url.to_file_path()
                            .ok()
                            .map(|p| p.to_string_lossy().to_string())
                    } else {
                        None
                    }
                })
                .collect();
            focus_main_window(app);
            open_from_os(app, paths);
        }
        RunEvent::ExitRequested { .. } => {
            log::info!("exiting; writing settings and the loudness cache");
            // Before anything is persisted: windows torn down from here must
            // not be recorded as "the user closed the EQ", or an EQ window
            // that was open at quit would not come back (see
            // `eqwindow::begin_shutdown`).
            eqwindow::begin_shutdown();
            if let Some(state) = app.try_state::<Arc<AppState>>() {
                // Mirrors the live engine state into the settings, writes both
                // files and waits for them: the last chance to persist.
                state.persist_flush();
                // SPEC §19: the extracted archives go now, while there is
                // still a thread to remove them on. Relying on `AppState`'s
                // own drop would not do: the managed state is not guaranteed
                // to be dropped before the process ends.
                let removed = state.clear_archives();
                if removed > 0 {
                    log::info!("removed {removed} extracted archive(s) on exit");
                }
            }
        }
        _ => {}
    });
}

#[cfg(test)]
mod tests {
    use super::{files_from_args, level_from_env, LevelFilter, OsString, Path};

    /* ── logging configuration ────────────────────────────────────────── */

    #[test]
    fn the_default_log_level_is_quiet_and_the_env_var_overrides_it() {
        // A shipping player writes warnings and failures, nothing else.
        assert_eq!(level_from_env(None), (LevelFilter::Warn, None));
        assert_eq!(level_from_env(Some("")), (LevelFilter::Warn, None));
        assert_eq!(level_from_env(Some("   ")), (LevelFilter::Warn, None));

        for (raw, expected) in [
            ("off", LevelFilter::Off),
            ("error", LevelFilter::Error),
            ("warn", LevelFilter::Warn),
            ("info", LevelFilter::Info),
            ("DEBUG", LevelFilter::Debug),
            (" trace ", LevelFilter::Trace),
        ] {
            let (level, complaint) = level_from_env(Some(raw));
            assert_eq!(level, expected, "ONYX_LOG={raw}");
            assert!(complaint.is_none(), "ONYX_LOG={raw} complained");
        }
    }

    #[test]
    fn a_nonsense_log_level_falls_back_and_says_so() {
        // Never a startup failure: the app is a music player.
        let (level, complaint) = level_from_env(Some("verbose"));
        assert_eq!(level, LevelFilter::Warn);
        let complaint = complaint.expect("a bad level must be reported");
        assert!(complaint.contains("ONYX_LOG"), "{complaint}");
        assert!(complaint.contains("verbose"), "{complaint}");
    }

    /// `src/lib/api.ts` is the contract. Nothing here can be checked by the
    /// compiler — a renamed command or argument is a runtime "command not
    /// found" in a shipped app — so it is checked by reading both sides.
    ///
    /// These two tests hold Rust and `api.ts` equal. The third implementation of
    /// the same surface, `src/lib/mock.ts`, is held to it by
    /// `scripts/check-ipc.mjs` instead of from here, because the failure it
    /// guards against is a *preview* that throws `mock: unknown command "…"`,
    /// and the preview is built by `npm run build:mock`, which never runs cargo.
    /// Same sets, same both-ways comparison, in the pipeline that can act on it.
    const API_TS: &str = include_str!("../../src/lib/api.ts");
    const THIS_FILE: &str = include_str!("lib.rs");
    const COMMANDS_RS: &str = include_str!("commands.rs");

    /// `(command, argument keys)` for every `call("…", { … })` in `api.ts`.
    fn frontend_calls() -> Vec<(String, Vec<String>)> {
        let mut out = Vec::new();
        for line in API_TS.lines() {
            let Some(rest) = line.split_once("call(\"") else {
                continue;
            };
            let Some((name, rest)) = rest.1.split_once('"') else {
                continue;
            };
            let args = match (rest.find('{'), rest.rfind('}')) {
                (Some(open), Some(close)) if close > open => rest[open + 1..close]
                    .split(',')
                    .map(|part| part.split(':').next().unwrap_or("").trim().to_string())
                    .filter(|key| !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric()))
                    .collect(),
                _ => Vec::new(),
            };
            out.push((name.to_string(), args));
        }
        out
    }

    /// Commands actually registered with `generate_handler!`.
    fn registered_commands() -> Vec<String> {
        let block = THIS_FILE
            .split_once("generate_handler![")
            .expect("the handler list must exist")
            .1
            .split_once("])")
            .expect("the handler list must be closed")
            .0;
        block
            .lines()
            .filter_map(|line| line.trim().strip_prefix("commands::"))
            .map(|name| name.trim_end_matches(',').to_string())
            .collect()
    }

    fn snake(camel: &str) -> String {
        let mut out = String::new();
        for c in camel.chars() {
            if c.is_ascii_uppercase() {
                out.push('_');
                out.push(c.to_ascii_lowercase());
            } else {
                out.push(c);
            }
        }
        out
    }

    #[test]
    fn every_front_end_command_is_registered_and_nothing_extra_is() {
        let wanted: Vec<String> = frontend_calls().into_iter().map(|(n, _)| n).collect();
        let registered = registered_commands();
        assert!(wanted.len() > 40, "parsed too few calls: {wanted:?}");
        for name in &wanted {
            assert!(
                registered.contains(name),
                "`{name}` is called by src/lib/api.ts but not in generate_handler!"
            );
            assert!(
                COMMANDS_RS.contains(&format!("pub async fn {name}(")),
                "`{name}` is registered but not defined in commands.rs"
            );
        }
        for name in &registered {
            assert!(
                wanted.contains(name),
                "`{name}` is registered but no front-end wrapper calls it"
            );
        }
    }

    #[test]
    fn every_front_end_argument_name_exists_on_the_rust_command() {
        // Tauri maps camelCase JS keys onto snake_case parameters, so `freqHz`
        // has to arrive as `freq_hz`. A typo here is silently `null` on the
        // Rust side (or a deserialisation error the user sees as gibberish).
        for (name, args) in frontend_calls() {
            let needle = format!("pub async fn {name}(");
            let Some(after) = COMMANDS_RS.split_once(&needle) else {
                panic!("`{name}` is not defined in commands.rs");
            };
            let signature = after
                .1
                .split_once("->")
                .map(|(sig, _)| sig)
                .unwrap_or(after.1);
            for arg in args {
                let expected = snake(&arg);
                assert!(
                    signature.contains(&format!("{expected}:")),
                    "`{name}` is called with `{arg}` but has no `{expected}` parameter: \
                     {signature}"
                );
            }
        }
    }

    /* ── the appearance seam (SPEC §14) ───────────────────────────────── */

    const THEME_TS: &str = include_str!("../../src/lib/theme.ts");
    /// The appearance model itself lives in a DOM-free module so the theme
    /// document validator (SPEC §20) can be run in Node; `theme.ts` re-exports
    /// it, which is what the assertion below also checks.
    const APPEARANCE_TS: &str = include_str!("../../src/lib/appearance.ts");
    /// The accent default is a named constant over in `accent.ts`, because the
    /// accent family derivation needs it too; the test below resolves the name.
    const ACCENT_TS: &str = include_str!("../../src/lib/accent.ts");

    /// `accent: DEFAULT_ACCENT` is a reference, not a literal. Resolve it the
    /// only way that cannot drift: by reading the constant it names out of the
    /// module that exports it. An unresolvable name fails the test rather than
    /// comparing the identifier itself against `#c9a227` and "passing" the day
    /// somebody renames it.
    fn resolve_ts_value(raw: &str) -> String {
        if !raw.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
            return raw.to_string();
        }
        let needle = format!("export const {raw} = \"");
        let literal = ACCENT_TS
            .split_once(&needle)
            .unwrap_or_else(|| panic!("`{raw}` is referenced but accent.ts does not export it"))
            .1
            .split_once('"')
            .expect("...as a string literal")
            .0;
        literal.to_string()
    }

    /// `src/lib/theme.ts` carries its own copy of the default appearance, so a
    /// window can paint the right theme before the first IPC round trip. Two
    /// copies of a default is exactly the kind of seam that rots: the front end
    /// would show champagne-on-obsidian while `settings.json` said bronze, and
    /// nothing would fail until a user complained. So the copy is checked
    /// against the real one, field name by field name — the field *names* are
    /// the other half of the contract, because `set_appearance` takes this
    /// object whole and serde would reject a mismatch at runtime only.
    #[test]
    fn the_front_ends_default_appearance_matches_the_rust_one() {
        // `theme.ts` is still the front door every component imports through,
        // so a rename that left it behind would be caught here too.
        assert!(
            THEME_TS.contains("DEFAULT_APPEARANCE"),
            "theme.ts must keep re-exporting DEFAULT_APPEARANCE"
        );
        let block = APPEARANCE_TS
            .split_once("export const DEFAULT_APPEARANCE: Appearance = {")
            .expect("appearance.ts must declare DEFAULT_APPEARANCE")
            .1
            .split_once("};")
            .expect("...and close it")
            .0;
        let mut ts: Vec<(String, String)> = Vec::new();
        for line in block.lines() {
            let line = line.trim();
            // skip the comments the block is annotated with
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            if key.starts_with("/") || key.starts_with("*") {
                continue;
            }
            let value = value.trim().trim_end_matches(',').trim_matches('"');
            ts.push((key.trim().to_string(), resolve_ts_value(value)));
        }

        let rust =
            serde_json::to_value(crate::settings::Appearance::default()).expect("serialisable");
        let rust = rust.as_object().expect("an object");
        assert_eq!(
            ts.len(),
            rust.len(),
            "theme.ts declares {} fields, Appearance has {}: {ts:?} vs {rust:?}",
            ts.len(),
            rust.len()
        );
        for (key, value) in ts {
            let mine = rust.get(&key).unwrap_or_else(|| {
                panic!("`{key}` is in theme.ts but not on Appearance: {rust:?}")
            });
            assert_eq!(
                mine.as_str(),
                Some(value.as_str()),
                "`{key}` defaults differ between theme.ts and settings.rs"
            );
        }
    }

    /* ── file associations (SPEC §5) ──────────────────────────────────── */

    /// Every extension the installer claims must be one Onyx will actually
    /// open: an association Onyx cannot open turns "Open with > Onyx" into an
    /// error toast, on a file the user was told this app handles.
    ///
    /// "Open" now means audio *or* an archive (SPEC §19), which is exactly
    /// what `playlist::openable_extensions` is — the same list behind the file
    /// dialog and behind the front end's drag-and-drop filter, so the three
    /// cannot disagree about `.zip`.
    ///
    /// The converse is deliberately *not* asserted — `webm` and `adpcm` are
    /// decodable but are container extensions a mastering engineer expects to
    /// belong to a video player, so Onyx does not claim them in Finder or
    /// Explorer.
    #[test]
    fn every_associated_extension_is_one_onyx_can_open() {
        let conf: serde_json::Value = serde_json::from_str(BASE_CONF).expect("valid JSON");
        let associations = conf["bundle"]["fileAssociations"]
            .as_array()
            .expect("file associations are required by SPEC §5");
        let openable = crate::playlist::openable_extensions();
        let mut claimed: Vec<String> = Vec::new();
        for association in associations {
            for ext in association["ext"].as_array().expect("an ext list") {
                let ext = ext.as_str().expect("a string extension");
                assert!(
                    openable.contains(&ext),
                    "the bundle claims .{ext} but Onyx cannot open it"
                );
                assert_eq!(
                    ext,
                    ext.to_ascii_lowercase(),
                    "extensions must be lower case"
                );
                claimed.push(ext.to_string());
            }
        }
        assert!(claimed.len() >= 12, "only {} extensions", claimed.len());
        // The v3 formats have to be claimed, or "Open with > Onyx" does not
        // appear for the very files this release added support for.
        for ext in ["mid", "midi", "zip", "mp4", "mov", "opus", "flac"] {
            assert!(
                claimed.iter().any(|c| c == ext),
                ".{ext} is supported but not associated"
            );
        }
    }

    /// The Linux bundle is what proves `tauri.conf.json` still parses, and it
    /// only gets built if `deb` is a target.
    #[test]
    fn the_bundle_targets_include_the_linux_package() {
        let conf: serde_json::Value = serde_json::from_str(BASE_CONF).expect("valid JSON");
        let targets = conf["bundle"]["targets"].as_array().expect("a target list");
        assert!(
            targets.iter().any(|t| t == "deb"),
            "the deb target is what CI builds to validate the bundle: {targets:?}"
        );
    }

    /* ── the macOS window overrides (SPEC §4) ─────────────────────────── */

    const BASE_CONF: &str = include_str!("../tauri.conf.json");
    const MACOS_CONF: &str = include_str!("../tauri.macos.conf.json");

    /// Tauri merges `tauri.macos.conf.json` over `tauri.conf.json` with an RFC
    /// 7386 merge, which replaces arrays wholesale — so the macOS override has
    /// to repeat the *whole* window object, and a field added to one and not
    /// the other silently disappears on macOS. Nobody can notice that on
    /// Linux, so it is checked here instead.
    #[test]
    fn the_macos_window_config_only_differs_where_it_must() {
        fn window(conf: &str) -> serde_json::Map<String, serde_json::Value> {
            let parsed: serde_json::Value = serde_json::from_str(conf).expect("valid JSON");
            parsed["app"]["windows"][0]
                .as_object()
                .expect("one window object")
                .clone()
        }
        let base = window(BASE_CONF);
        let mac = window(MACOS_CONF);

        // Only the decoration keys may differ: an undecorated NSWindow has no
        // traffic lights, and the 78 px inset in the title bar is reserved for
        // them, so macOS keeps its decorations and hides the title instead.
        let decoration_keys = ["decorations", "titleBarStyle", "hiddenTitle"];
        assert_eq!(
            base.keys().collect::<Vec<_>>(),
            mac.keys().collect::<Vec<_>>(),
            "the two window objects must describe the same fields"
        );
        for (key, value) in &base {
            if decoration_keys.contains(&key.as_str()) {
                continue;
            }
            assert_eq!(mac.get(key), Some(value), "`{key}` drifted on macOS");
        }
        assert_eq!(base["decorations"], serde_json::json!(false));
        assert_eq!(mac["decorations"], serde_json::json!(true));
        assert_eq!(mac["titleBarStyle"], serde_json::json!("Overlay"));
        assert_eq!(mac["hiddenTitle"], serde_json::json!(true));
    }

    /// Neither config may pin a window `theme`.
    ///
    /// The macOS override used to carry `"theme": "Dark"`, from when obsidian
    /// was the only theme there was. It is wrong now on the merits, not just
    /// asymmetric: SPEC §14 gives the user `dark | light | system`, and a
    /// hard-coded window theme both fights `light` (a dark frame around a paper
    /// window) and defeats `system` outright, since the window would no longer
    /// follow the OS at all. The frame therefore follows the OS by default and
    /// the user's choice is pushed to it at runtime by
    /// [`crate::apply_native_appearance`] — `None` for `system`.
    #[test]
    fn neither_config_hard_codes_a_window_theme() {
        for (which, conf) in [("tauri.conf.json", BASE_CONF), ("macOS", MACOS_CONF)] {
            let parsed: serde_json::Value = serde_json::from_str(conf).expect("valid JSON");
            for window in parsed["app"]["windows"].as_array().expect("windows") {
                assert!(
                    window.get("theme").is_none(),
                    "{which} pins a window theme; the user's theme choice would lose"
                );
            }
        }
    }

    #[test]
    fn argv_is_filtered_down_to_file_arguments() {
        let argv = args([
            "/Applications/Onyx.app/Contents/MacOS/onyx",
            "--",
            "-psn_0_12345",
            "/music/take 1.flac",
            "/music/take 2.wav",
        ]);
        assert_eq!(
            files_from_args(argv, Path::new("/tmp")),
            ["/music/take 1.flac", "/music/take 2.wav"]
        );
        // The binary path on its own must not look like a file to open.
        assert!(files_from_args(args(["onyx"]), Path::new("/tmp")).is_empty());
        assert!(files_from_args(Vec::new(), Path::new("/tmp")).is_empty());
    }

    /// A second instance is launched in the directory the user is standing in,
    /// and the single-instance plugin forwards its argv to *this* process,
    /// which is standing somewhere else. `onyx take.wav` from a session folder
    /// used to resolve against the running instance's directory — "no
    /// supported audio files in that selection", or worse, a different take of
    /// the same name.
    #[test]
    fn a_relative_path_is_resolved_against_the_directory_it_came_from() {
        // The expectation is built with `join` rather than spelled out: the
        // separator belongs to the platform, and `/sessions/mix\take 1.flac`
        // on Windows is the same answer as `/sessions/mix/take 1.flac` here.
        let cwd = Path::new("/sessions/mix");
        let files = files_from_args(args(["onyx", "take 1.flac"]), cwd);
        assert_eq!(
            files,
            [cwd.join("take 1.flac").to_string_lossy().to_string()]
        );

        // Absolute paths are left exactly as given: no canonicalisation, so a
        // symlinked library keeps the name the user knows it by.
        //
        // "Absolute" means whatever the platform means by it. `/music/take.wav`
        // has a root but no drive letter, which Windows does not count as
        // absolute — and the argv a Windows "open with" actually hands us is
        // `C:\…`, so that is what is worth asserting there.
        let absolute = if cfg!(windows) {
            r"C:\music\take.flac"
        } else {
            "/music/take.flac"
        };
        let files = files_from_args(args(["onyx", absolute]), Path::new("/sessions"));
        assert_eq!(files, [absolute]);
    }

    /// `std::env::args()` panics on an argument that is not valid Unicode, so
    /// the whole app used to die at launch on a file name a Linux user can
    /// create with `touch`. Nothing can be done with such a path — it cannot be
    /// serialised into the playlist as JSON — but refusing it is not crashing.
    #[test]
    #[cfg(unix)]
    fn a_non_utf8_argument_is_skipped_rather_than_fatal() {
        use std::os::unix::ffi::OsStringExt;
        let argv = vec![
            OsString::from("onyx"),
            OsString::from_vec(b"/music/\xff\xfe.wav".to_vec()),
            OsString::from("/music/fine.wav"),
        ];
        assert_eq!(
            files_from_args(argv, Path::new("/tmp")),
            ["/music/fine.wav"]
        );
    }

    fn args<const N: usize>(raw: [&str; N]) -> Vec<OsString> {
        raw.iter().map(OsString::from).collect()
    }
}
