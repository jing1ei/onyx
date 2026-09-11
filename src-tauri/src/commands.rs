//! Every IPC command in SPEC §3.1 / §6–§12, i.e. every wrapper in
//! `src/lib/api.ts`. That file is the contract: command names are snake_case,
//! payload and argument keys are camelCase, and nothing here may drift from it.
//!
//! All of them are `async` on purpose: a synchronous Tauri command runs on the
//! main thread, and opening a file or re-clocking the output device must never
//! block the window's event loop. Every one returns `Result<_, String>`, nothing
//! in here unwraps user input, and no command returns `Ok` after a partial
//! failure — a load that fails leaves the deck it failed on untouched.

use std::sync::Arc;

use onyx_core::align;
use onyx_core::engine::SourceRequest;
use onyx_core::midi::{self, MidiOptions};
use onyx_core::waveform::WaveformData;
use onyx_core::{
    Deck, DeviceInfo, EngineSource, EqConfig, HostInfo, MeterSnapshot, MonitorMode, MAX_BANDS,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

use crate::abrules;
use crate::blind::{BlindMode, BlindSnapshot, Slot};
use crate::cache::CacheStats;
use crate::eqwindow;
use crate::loader;
use crate::playlist::openable_extensions;
use crate::settings::{
    normalise_accent, normalise_font, normalise_theme_doc, theme_doc_is_blank, Appearance,
    MAX_THEME_DOC_BYTES,
};
use crate::state::{AppSnapshot, AppState};
use crate::surface;
use crate::themewindow;

type Res<T> = Result<T, String>;
type St<'a> = State<'a, Arc<AppState>>;

#[tauri::command]
pub async fn editor_io(app: AppHandle, request: crate::editor::Request) -> Res<tauri::ipc::Response> {
    let started = std::time::Instant::now();
    let action = request.action.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        if request.action == "read" { return crate::editor::read_chunk(&app, request).map(tauri::ipc::Response::new); }
        let reply = crate::editor::handle(&app, request)?;
        serde_json::to_string(&reply).map(tauri::ipc::Response::new).map_err(|e| e.to_string())
    })
        .await.map_err(|e| e.to_string())?;
    if started.elapsed().as_millis() >= 100 {
        log::warn!("Slow editor_io action={} elapsed_ms={}", action, started.elapsed().as_millis());
    }
    result
}

/* ── app / playlist ──────────────────────────────────────────────────────── */

#[tauri::command]
pub async fn app_state(state: St<'_>) -> Res<AppSnapshot> {
    // The frontend calls this once it has subscribed to `onyx://toast`, so it
    // is the first moment a startup complaint can actually reach the user.
    state.flush_startup_notices();
    Ok(state.snapshot())
}

#[tauri::command]
pub async fn open_files(state: St<'_>, paths: Vec<String>, replace: bool) -> Res<AppSnapshot> {
    loader::open_paths(state.inner(), paths, replace)?;
    Ok(state.snapshot())
}

#[tauri::command]
pub async fn pick_and_open_files(app: AppHandle, state: St<'_>, replace: bool) -> Res<AppSnapshot> {
    let (tx, rx) = std::sync::mpsc::channel();
    app.dialog()
        .file()
        .set_title(if replace {
            crate::editor::label("Open audio", "打开音频")
        } else {
            crate::editor::label("Add to playlist", "添加到播放列表")
        })
        .add_filter(crate::editor::label("Audio", "音频"), &openable_extensions())
        .pick_files(move |picked| {
            let _ = tx.send(picked);
        });
    // The dialog runs on the main thread and calls us back; waiting for it must
    // happen on a blocking thread, never on an async worker.
    let picked = tauri::async_runtime::spawn_blocking(move || rx.recv().ok().flatten())
        .await
        .map_err(|e| format!("file dialog failed: {e}"))?;

    let Some(files) = picked else {
        // Cancelled — not an error.
        return Ok(state.snapshot());
    };
    let paths: Vec<String> = files
        .into_iter()
        .filter_map(|f| f.into_path().ok())
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    if paths.is_empty() {
        return Ok(state.snapshot());
    }
    loader::open_paths(state.inner(), paths, replace)?;
    Ok(state.snapshot())
}

#[tauri::command]
pub async fn playlist_play_index(state: St<'_>, index: usize) -> Res<AppSnapshot> {
    let id = state.playlist.lock().id_at(index);
    let id = id.ok_or_else(|| format!("no playlist row at index {index}"))?;
    loader::load_entry_into_deck(state.inner(), Deck::A, id, true)?;
    Ok(state.snapshot())
}

#[tauri::command]
pub async fn playlist_play_entry(state: St<'_>, id: u64) -> Res<AppSnapshot> {
    loader::load_entry_into_deck(state.inner(), Deck::A, id, true)?;
    Ok(state.snapshot())
}

#[tauri::command]
pub async fn playlist_remove(state: St<'_>, id: u64) -> Res<AppSnapshot> {
    // Read which decks hold it first: `clear_deck` takes the same lock.
    let on_decks: Vec<Deck> = {
        let decks = state.decks.lock();
        [Deck::A, Deck::B]
            .into_iter()
            .filter(|d| decks[d.index()].entry_id == Some(id))
            .collect()
    };
    // Removing a row that is on a deck clears that deck, which is exactly the
    // material a running test is comparing. Removing any other row is harmless,
    // so it stays allowed even though the UI blocks it wholesale.
    if !on_decks.is_empty() {
        state.blind_guard("Removing a track that is on a deck")?;
    }
    for deck in on_decks {
        loader::clear_deck(state.inner(), deck);
    }
    let removed = state.playlist.lock().remove(id).is_some();
    if !removed {
        // The row was already gone; the UI is out of date, so hand it a
        // snapshot rather than an error it cannot act on.
        log::debug!("playlist_remove: no row with id {id}");
    }
    state.mark_state_dirty();
    Ok(state.snapshot())
}

#[tauri::command]
pub async fn playlist_clear(state: St<'_>) -> Res<AppSnapshot> {
    // Clears both decks, so it would leave the test with nothing behind its
    // slots. Guarded before anything is torn down.
    state.blind_guard("Clearing the playlist")?;
    state.engine.stop();
    for deck in [Deck::A, Deck::B] {
        loader::clear_deck(state.inner(), deck);
    }
    state.playlist.lock().clear();
    // SPEC §19: the extracted copies of any opened archives go with the
    // rows that pointed at them, rather than sitting in the temp directory
    // until the process exits.
    let removed = state.clear_archives();
    if removed > 0 {
        log::info!("playlist cleared; removed {removed} extracted archive(s)");
    }
    state.mark_state_dirty();
    Ok(state.snapshot())
}

#[tauri::command]
pub async fn playlist_move(state: St<'_>, from: usize, to: usize) -> Res<AppSnapshot> {
    let moved = state.playlist.lock().move_entry(from, to);
    if !moved {
        return Err(format!("no playlist row at index {from}"));
    }
    state.mark_state_dirty();
    Ok(state.snapshot())
}

#[tauri::command]
pub async fn playlist_next(state: St<'_>) -> Res<AppSnapshot> {
    loader::play_step(state.inner(), 1, true)?;
    Ok(state.snapshot())
}

#[tauri::command]
pub async fn playlist_prev(state: St<'_>) -> Res<AppSnapshot> {
    loader::play_step(state.inner(), -1, true)?;
    Ok(state.snapshot())
}

/* ── transport ───────────────────────────────────────────────────────────── */

#[tauri::command]
pub async fn transport_toggle(state: St<'_>) -> Res<()> {
    state.engine.toggle();
    Ok(())
}

#[tauri::command]
pub async fn transport_play(state: St<'_>) -> Res<()> {
    state.engine.play();
    Ok(())
}

#[tauri::command]
pub async fn transport_pause(state: St<'_>) -> Res<()> {
    state.engine.pause();
    Ok(())
}

#[tauri::command]
pub async fn transport_stop(state: St<'_>) -> Res<()> {
    state.engine.stop();
    Ok(())
}

#[tauri::command]
pub async fn transport_seek(state: St<'_>, secs: f64) -> Res<()> {
    state.engine.seek_secs(clamp_position(&state, secs));
    Ok(())
}

#[tauri::command]
pub async fn transport_nudge(state: St<'_>, secs: f64) -> Res<()> {
    if !secs.is_finite() {
        return Err("nudge must be a number".into());
    }
    let target = state.engine.shared().position_secs() + secs;
    state.engine.seek_secs(clamp_position(&state, target));
    Ok(())
}

/// Keep a seek inside the loaded material; a seek past the end would just sit
/// there in silence.
fn clamp_position(state: &Arc<AppState>, secs: f64) -> f64 {
    let duration = {
        let decks = state.decks.lock();
        decks[0].duration_secs().max(decks[1].duration_secs())
    };
    clamp_secs(secs, duration)
}

/// The rule on its own, so the boundaries can be tested without a device.
///
/// A seek to exactly the end is honoured rather than nudged back: the engine
/// reports the track as ended there, which is what dragging the playhead to the
/// far right is asking for. Nothing is loaded (`duration <= 0`) means the only
/// legal position is zero.
fn clamp_secs(secs: f64, duration: f64) -> f64 {
    // NaN has no place on a timeline; ±infinity clamps to the two ends like any
    // other out-of-range number.
    if secs.is_nan() || !duration.is_finite() || duration <= 0.0 {
        return 0.0;
    }
    secs.clamp(0.0, duration)
}

#[tauri::command]
pub async fn set_volume(state: St<'_>, value: f32) -> Res<()> {
    if !value.is_finite() {
        return Err("volume must be a number".into());
    }
    state.engine.set_volume(value.clamp(0.0, 1.0));
    state.remember_settings();
    Ok(())
}

#[tauri::command]
pub async fn set_muted(state: St<'_>, value: bool) -> Res<()> {
    state.engine.set_muted(value);
    state.remember_settings();
    Ok(())
}

#[tauri::command]
pub async fn set_loop_enabled(state: St<'_>, value: bool) -> Res<()> {
    state.engine.set_loop_enabled(value);
    state.remember_settings();
    Ok(())
}

#[tauri::command]
pub async fn set_loop_region(state: St<'_>, region: Option<(f64, f64)>) -> Res<()> {
    if let Some((a, b)) = region {
        if !a.is_finite() || !b.is_finite() {
            return Err("loop region must be two numbers".into());
        }
    }
    state.engine.set_loop_region(region);
    if region.is_some() {
        state.engine.set_loop_enabled(true);
    }
    state.mark_state_dirty();
    Ok(())
}

/// Monitoring fold (SPEC §6). Remembered across launches.
#[tauri::command]
pub async fn set_monitor_mode(state: St<'_>, mode: MonitorMode) -> Res<()> {
    state.engine.set_monitor_mode(mode);
    state.remember_settings();
    // `monitorMode` rides on every frame, but the snapshot carries it too and
    // the badge must not wait up to a frame behind the rest of the UI.
    state.mark_state_dirty();
    Ok(())
}

/* ── A/B ─────────────────────────────────────────────────────────────────── */

#[tauri::command]
pub async fn ab_set_enabled(state: St<'_>, value: bool) -> Res<AppSnapshot> {
    if !value {
        // A blind test without two audible decks is meaningless, and throwing
        // the votes away behind the listener's back is worse than refusing:
        // `blind_abort` is the explicit way to stop a run.
        state.blind_guard("Switching A/B off")?;
    }
    state.engine.set_ab_enabled(value);
    state.ab.lock().enabled = value;
    if !value {
        // Defence in depth: the guard above means an active test cannot reach
        // this, but a future caller that bypasses it must not leave a test
        // running against a single audible deck.
        let aborted = {
            let mut blind = state.blind.lock();
            let running = blind.is_active();
            if running {
                blind.abort();
            }
            running
        };
        if aborted {
            state.warn("Blind test stopped: A/B was switched off");
        }
    }
    loader::recompute_trims(state.inner());
    state.remember_settings();
    Ok(state.snapshot())
}

#[tauri::command]
pub async fn ab_select(state: St<'_>, deck: Deck) -> Res<()> {
    // While a blind test runs, only the test's own slots may move the audible
    // deck; an A/B button would tell the listener which one they are on. This
    // used to return `Ok(())`, which meant a caller that got past the hidden UI
    // could not tell the difference between "done" and "ignored".
    state.blind_guard("Selecting a deck by name")?;
    state.engine.select_deck(deck);
    Ok(())
}

#[tauri::command]
pub async fn ab_toggle_deck(state: St<'_>) -> Res<()> {
    state.blind_guard("Switching decks")?;
    let next = state.engine.active_deck().other();
    state.engine.select_deck(next);
    Ok(())
}

#[tauri::command]
pub async fn ab_assign(state: St<'_>, deck: Deck, id: u64) -> Res<AppSnapshot> {
    loader::load_entry_into_deck(state.inner(), deck, id, false)?;
    // Assigning deck B implies you want to hear the comparison (SPEC §2.8).
    // The rule is `abrules::ab_enabled_after_assign` and nowhere else: the
    // front-end mock backend has to reproduce it exactly, and when it did not,
    // deck B assignment looked broken in the shipped app while every preview
    // verification passed.
    {
        let enable = {
            let mut ab = state.ab.lock();
            let was = ab.enabled;
            ab.enabled = abrules::ab_enabled_after_assign(deck, was);
            ab.enabled && !was
        };
        if enable {
            state.engine.set_ab_enabled(true);
            state.remember_settings();
        }
    }
    loader::recompute_trims(state.inner());
    Ok(state.snapshot())
}

#[tauri::command]
pub async fn ab_set_crossfade_ms(state: St<'_>, value: f32) -> Res<()> {
    if !value.is_finite() {
        return Err("crossfade must be a number".into());
    }
    state.engine.set_crossfade_ms(value);
    // Read back what the engine clamped it to, so the UI shows the truth. The
    // read happens before the lock is taken, not inside the assignment.
    let clamped = state.engine.crossfade_ms();
    state.ab.lock().crossfade_ms = clamped;
    state.remember_settings();
    state.mark_state_dirty();
    Ok(())
}

/// Loudness matching, opt-in and attenuation-only (SPEC §10).
#[tauri::command]
pub async fn set_level_match(state: St<'_>, enabled: bool) -> Res<()> {
    // Changing the trim mid-test changes the very thing under test: the level
    // difference between the two decks is often *what* is being compared.
    state.blind_guard("Level matching")?;
    state.ab.lock().level_match.enabled = enabled;
    // Applies the real trims if both measurements are in, and reports
    // `ready: false` at unity if they are not. Either way the engine glides.
    loader::recompute_trims(state.inner());
    state.remember_settings();
    state.mark_state_dirty();
    Ok(())
}

/* ── A/B time alignment (SPEC §11) ───────────────────────────────────────── */

/// `AlignResult` in `src/lib/types.ts`.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlignResult {
    pub offset_frames: i64,
    pub offset_ms: f64,
    pub confidence: f32,
    pub polarity_inverted: bool,
    pub applied: bool,
}

/// Manual offset. Takes an `f64` because the front end sends a JSON number;
/// rounding here rather than refusing a `1.0` keeps a pointer-rate drag working.
#[tauri::command]
pub async fn set_ab_offset(state: St<'_>, frames: f64) -> Res<()> {
    if !frames.is_finite() {
        return Err("offset must be a number".into());
    }
    // Sliding one deck against the other mid-trial changes what the listener is
    // comparing (and a few ms of pre-echo is audible), so it is refused.
    state.blind_guard("Changing the A/B offset")?;
    // The engine clamps to ±30 s worth of frames.
    state.engine.set_ab_offset_frames(frames.round() as i64);
    state.mark_state_dirty();
    Ok(())
}

#[tauri::command]
pub async fn auto_align_ab(state: St<'_>) -> Res<AlignResult> {
    // Same reason as `set_ab_offset`: this applies an offset when it is
    // confident, which would move the material under a live trial.
    state.blind_guard("Auto-aligning the decks")?;
    let (Some(a), Some(b)) = (state.deck_handle(Deck::A), state.deck_handle(Deck::B)) else {
        return Err("load a track into deck A and deck B before auto-aligning".into());
    };
    let rate = state.engine.engine_rate();
    let (pcm_a, pcm_b) = (Arc::clone(&a.pcm), Arc::clone(&b.pcm));
    // Cross-correlating a minute of audio takes long enough to be felt: it runs
    // on a blocking worker, never on an async runtime thread and certainly never
    // on the audio thread.
    let estimate = tauri::async_runtime::spawn_blocking(move || {
        align::estimate_from_pcm(&pcm_a, &pcm_b, rate)
    })
    .await
    .map_err(|e| format!("the alignment worker did not finish: {e}"))?
    .map_err(|e| e.to_string())?;

    let applied = estimate.is_confident();
    let offset_frames = if applied {
        state.engine.set_ab_offset_frames(estimate.offset_frames);
        state.mark_state_dirty();
        // Report what the engine actually holds: it clamps to ±30 s, and the UI
        // adopts this number optimistically.
        state.engine.ab_offset_frames()
    } else {
        // A wrong automatic offset is worse than none, so nothing is applied and
        // the existing offset is left exactly as the user had it.
        estimate.offset_frames
    };
    Ok(AlignResult {
        offset_frames,
        offset_ms: offset_frames as f64 * 1000.0 / rate.max(1) as f64,
        confidence: estimate.confidence,
        polarity_inverted: estimate.polarity_inverted,
        applied,
    })
}

#[tauri::command]
pub async fn set_deck_invert(state: St<'_>, deck: Deck, invert: bool) -> Res<()> {
    // Inverting one deck and not the other is a change to one side of the
    // comparison, and it is audible on anything but a null test.
    state.blind_guard("Inverting a deck's polarity")?;
    state.engine.set_deck_invert(deck, invert);
    // `invert` is only in the snapshot, not in the 60 Hz frame.
    state.mark_state_dirty();
    Ok(())
}

/* ── blind test (2AFC + ABX, SPEC §7) ────────────────────────────────────── */

#[tauri::command]
pub async fn blind_start(state: St<'_>, trials: usize, mode: BlindMode) -> Res<BlindSnapshot> {
    {
        let decks = state.decks.lock();
        if !decks[Deck::A.index()].is_loaded() || !decks[Deck::B.index()].is_loaded() {
            return Err("load a track into deck A and deck B first".into());
        }
    }
    // Both decks have to be audible for the slots to mean anything.
    let enable = {
        let mut ab = state.ab.lock();
        let was = ab.enabled;
        ab.enabled = true;
        !was
    };
    if enable {
        state.engine.set_ab_enabled(true);
        state.remember_settings();
    }
    let deck = state.blind.lock().start(trials, mode);
    state.engine.select_deck(deck);
    let snapshot = state.blind.lock().snapshot();
    state.emit_state();
    Ok(snapshot)
}

/// `slot` arrives as a string and is validated against the running protocol:
/// an unknown name is an `Err`, never a panic and never a silent no-op.
#[tauri::command]
pub async fn blind_switch(state: St<'_>, slot: String) -> Res<BlindSnapshot> {
    let parsed = Slot::parse(&slot).ok_or_else(|| format!("unknown slot \"{slot}\""))?;
    let (result, snapshot) = {
        let mut blind = state.blind.lock();
        (blind.switch(parsed), blind.snapshot())
    };
    let deck = result?;
    state.engine.select_deck(deck);
    Ok(snapshot)
}

#[tauri::command]
pub async fn blind_vote(state: St<'_>, slot: String) -> Res<BlindSnapshot> {
    let parsed = Slot::parse(&slot).ok_or_else(|| format!("unknown slot \"{slot}\""))?;
    let (result, snapshot) = {
        let mut blind = state.blind.lock();
        (blind.vote(parsed), blind.snapshot())
    };
    match result? {
        Some(deck) => state.engine.select_deck(deck),
        // Finished: hand the listener back to deck A with the identities shown.
        None => state.engine.select_deck(Deck::A),
    }
    state.emit_state();
    Ok(snapshot)
}

#[tauri::command]
pub async fn blind_abort(state: St<'_>) -> Res<BlindSnapshot> {
    let snapshot = {
        let mut blind = state.blind.lock();
        blind.abort();
        blind.snapshot()
    };
    state.engine.select_deck(Deck::A);
    state.emit_state();
    Ok(snapshot)
}

/* ── EQ (SPEC §12) ───────────────────────────────────────────────────────── */

/// The single authoritative setter: the front end owns the band list and always
/// sends the whole config. There are deliberately no per-band commands — two
/// sources of truth for a filter chain is how a UI ends up disagreeing with
/// what you hear.
#[tauri::command]
pub async fn set_eq(state: St<'_>, config: EqConfig) -> Res<()> {
    if config.bands.len() > MAX_BANDS {
        // Silently truncating would leave the UI showing bands that are not
        // running, so this is a refusal rather than a partial success.
        return Err(format!(
            "{} EQ bands requested, {MAX_BANDS} is the maximum",
            config.bands.len()
        ));
    }
    // The engine clamps every parameter and hands back what is really running.
    let applied = state.engine.set_eq(config);
    state.settings.lock().eq = applied;
    state.mark_settings_dirty();
    state.mark_state_dirty();
    Ok(())
}

/// Band-solo audition bandpass. `freqHz: null` means audition off.
#[tauri::command]
pub async fn set_eq_audition(state: St<'_>, freq_hz: Option<f32>, q: f32) -> Res<()> {
    if let Some(f) = freq_hz {
        if !f.is_finite() {
            return Err("audition frequency must be a number".into());
        }
    }
    // No `mark_state_dirty`: this is dragged at pointer rate, and the only thing
    // it changes in the snapshot is `bitTransparent`, which rides on the frame.
    state.engine.set_eq_audition(freq_hz, q);
    Ok(())
}

/// A closed EQ panel must cost zero FFT.
#[tauri::command]
pub async fn set_spectrum_enabled(state: St<'_>, enabled: bool) -> Res<()> {
    state.engine.set_spectrum_enabled(enabled);
    Ok(())
}

/* ── the EQ window (SPEC §12) ────────────────────────────────────────────── */

/// Open the detached EQ window, or focus the one that is already open.
///
/// This is a command rather than a `WebviewWindow.create()` in the renderer on
/// purpose (SPEC §5.1): neither webview holds a window-creation permission, so
/// there is exactly one code path that can bring an EQ window into existence
/// and it cannot be talked into making a second one.
#[tauri::command]
pub async fn eq_window_open(app: AppHandle) -> Res<()> {
    eqwindow::open(&app, true)
}

/// Close it. Playback is untouched; the analyser and any audition bandpass are
/// switched off by the window's own destroy handler.
#[tauri::command]
pub async fn eq_window_close(app: AppHandle) -> Res<()> {
    eqwindow::close(&app);
    Ok(())
}

/// `E` from either window: open if closed, close if open.
#[tauri::command]
pub async fn eq_window_toggle(app: AppHandle) -> Res<()> {
    eqwindow::toggle(&app)
}

/// Always-on-top for the EQ window. Persisted in `settings.json`.
#[tauri::command]
pub async fn eq_window_set_pinned(app: AppHandle, pinned: bool) -> Res<()> {
    eqwindow::set_pinned(&app, pinned);
    Ok(())
}

/// The current `{ open, pinned }`, for a webview that has just loaded and has
/// not seen an `onyx://eq-window` event yet.
#[tauri::command]
pub async fn eq_window_state(app: AppHandle) -> Res<eqwindow::EqWindowState> {
    Ok(eqwindow::snapshot(&app))
}

/* ── waveform / device / cache / misc ────────────────────────────────────── */

#[tauri::command]
pub async fn waveform_get(state: St<'_>, deck: Deck, from: usize) -> Res<WaveformData> {
    let handle = state.deck_handle(deck);
    Ok(match handle {
        // `from` beyond the last bucket is normal, not an error: the lane asks
        // for "everything after what I have drawn" and the decode may not have
        // produced anything since. The core clamps and the reply is an empty
        // slice with an honest `count`.
        Some(handle) => handle.waveform.data(from),
        // Not loaded (yet): an empty lane is the honest answer, not an error.
        None => WaveformData {
            bucket_secs: 0.0,
            count: 0,
            expected: 0,
            min: Vec::new(),
            max: Vec::new(),
            rms: Vec::new(),
        },
    })
}

#[tauri::command]
pub async fn devices_list(state: St<'_>) -> Res<Vec<DeviceInfo>> {
    Ok(state.engine.list_devices())
}

/* ── engine source: host, device, rate, buffer (SPEC §16) ─────────────── */

/// Audio APIs this machine offers. Cheap, and safe to call repeatedly: the
/// core enumerates on demand so a driver installed while Onyx is running
/// shows up on the next call.
///
/// On a machine with no sound at all (CI, a headless build box) this is an
/// empty list, not an error — the panel says "no audio API available" rather
/// than showing a failure the user cannot act on.
#[tauri::command]
pub async fn audio_hosts(state: St<'_>) -> Res<Vec<HostInfo>> {
    Ok(state.engine.list_hosts())
}

/// Output devices on `hostId`, or on the host in use when it is `null`.
#[tauri::command]
pub async fn audio_devices(state: St<'_>, host_id: Option<String>) -> Res<Vec<DeviceInfo>> {
    Ok(match host_id.as_deref() {
        Some(host) => state.engine.list_devices_for_host(Some(host)),
        None => state.engine.list_devices(),
    })
}

/// Everything the "Audio device" section needs in one round trip: what is
/// playing now, plus the lists to choose from.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioSourceState {
    /// `null` only if no stream has ever been opened, which cannot happen
    /// while the app is running — it refuses to start without one.
    pub source: Option<EngineSource>,
    pub hosts: Vec<HostInfo>,
    /// Devices on the host currently in use.
    pub devices: Vec<DeviceInfo>,
}

#[tauri::command]
pub async fn audio_source(state: St<'_>) -> Res<AudioSourceState> {
    Ok(AudioSourceState {
        source: state.engine_source(),
        hosts: state.engine.list_hosts(),
        devices: state.engine.list_devices(),
    })
}

/// One atomic change to the engine source. Every field is "leave it alone"
/// when absent, so the panel can send only what the user touched.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SourceChange {
    pub host_id: Option<String>,
    pub device_name: Option<String>,
    /// Follow the OS default output device. This is how "System default" is
    /// expressed: `deviceName: null` means "unchanged", not "the default".
    pub system_default_device: bool,
    pub sample_rate: Option<u32>,
    pub follow_source_rate: Option<bool>,
    pub buffer_frames: Option<u32>,
}

/// Change host / device / rate / buffer in one stream rebuild (SPEC §16).
///
/// Safe while playing: the core tears the stream down and brings it back with
/// the playhead, the transport state, the loop region and both decks intact,
/// and restores the previous stream if the new one cannot be opened. What is
/// left for this layer is the *other* half of "do not lose state": if the new
/// stream runs at a different rate, both decks hold PCM at the old one, so
/// they are re-decoded in place — playing if they were playing, at the same
/// position — rather than being cleared.
#[tauri::command]
pub async fn audio_source_set(state: St<'_>, change: SourceChange) -> Res<AppSnapshot> {
    let previous_rate = state.engine.engine_rate();
    let request = SourceRequest {
        host_id: change.host_id.clone(),
        device_name: change.device_name.clone(),
        use_system_default_device: change.system_default_device,
        sample_rate: change.sample_rate,
        follow_source_rate: change.follow_source_rate,
        buffer_frames: change.buffer_frames,
    };
    let source = state.engine.set_source(&request).map_err(|e| {
        format!(
            "could not open {}: {e}",
            change
                .device_name
                .clone()
                .unwrap_or_else(|| "that audio device".into())
        )
    })?;
    if source.sample_rate != previous_rate {
        loader::rearm_decks(state.inner());
    }
    state.info(describe_source(&source));
    state.remember_settings();
    state.mark_state_dirty();
    Ok(state.snapshot())
}

/// The one line the toast shows after a source change. Pure, so the wording —
/// which is the whole point of returning what was *granted* rather than what
/// was asked for — can be tested without an audio device.
fn describe_source(source: &EngineSource) -> String {
    let device = source.device_name.as_deref().unwrap_or("no output");
    let default = if source.following_system_default {
        "system default: "
    } else {
        ""
    };
    let rate = format!("{:.1} kHz", source.sample_rate as f32 / 1000.0);
    match (source.buffer_frames, source.latency_ms) {
        (Some(frames), Some(ms)) => {
            format!("Output → {default}{device} · {rate} · {frames} frames ({ms:.1} ms)")
        }
        _ => format!("Output → {default}{device} · {rate}"),
    }
}

/* ── the General MIDI bank (SPEC §18) ─────────────────────────────────── */

/// Which SoundFont MIDI files are rendered through.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SoundFontState {
    /// The user's `.sf2`, or `null` when the bundled bank is in use.
    pub path: Option<String>,
    /// Bank name as the file declares it — the `<bank>` of `MIDI · GM · …`.
    pub name: String,
    pub bundled: bool,
}

/// Load a bank and describe it, refusing a user file that cannot be used.
///
/// The core never fails over the *user's* choice: a missing or corrupt `.sf2`
/// silently becomes the bundled bank with a `fallback_reason`. That is right
/// for playback — a MIDI file still makes a sound — but wrong for a settings
/// command, where the user is entitled to be told their file was rejected
/// instead of watching it be quietly ignored.
fn describe_bank(path: Option<String>) -> Result<SoundFontState, String> {
    let opts = MidiOptions {
        soundfont: path.as_deref().map(std::path::PathBuf::from),
    };
    let (_, bank) = midi::load_bank(&opts).map_err(|e| e.to_string())?;
    if let Some(reason) = bank.fallback_reason {
        return Err(reason);
    }
    Ok(SoundFontState {
        path,
        name: bank.name,
        bundled: opts.soundfont.is_none(),
    })
}

#[tauri::command]
pub async fn soundfont_get(state: St<'_>) -> Res<SoundFontState> {
    let path = state.settings.lock().soundfont.clone();
    // Parsing a 30 MB bank is not something to do on an async runtime thread.
    // A bank that has gone missing since it was chosen reports itself as the
    // bundled one with the reason, rather than as an error the panel cannot
    // render.
    let described =
        tauri::async_runtime::spawn_blocking(move || match describe_bank(path.clone()) {
            Ok(state) => Ok(state),
            Err(reason) => match describe_bank(None) {
                Ok(bundled) => {
                    log::warn!(
                        "the chosen SoundFont is unusable ({reason}); using the bundled bank"
                    );
                    Ok(bundled)
                }
                Err(e) => Err(e),
            },
        })
        .await
        .map_err(|e| format!("the SoundFont worker did not finish: {e}"))?;
    described
}

/// Choose (or clear, with `null`) the user SoundFont. Persisted, and applied
/// to the next track loaded — a bank change does not re-render what is
/// already decoded, but it does invalidate the loudness cache for it.
#[tauri::command]
pub async fn soundfont_set(state: St<'_>, path: Option<String>) -> Res<SoundFontState> {
    let path = path.map(|p| p.trim().to_string()).filter(|p| !p.is_empty());
    let described = {
        let path = path.clone();
        tauri::async_runtime::spawn_blocking(move || describe_bank(path))
            .await
            .map_err(|e| format!("the SoundFont worker did not finish: {e}"))??
    };
    state.settings.lock().soundfont = path;
    state.mark_settings_dirty();
    state.mark_state_dirty();
    state.info(if described.bundled {
        "SoundFont → the bundled General MIDI bank".to_string()
    } else {
        format!("SoundFont → {}", described.name)
    });
    Ok(described)
}

/// Pick a `.sf2` with the native dialog. The webview cannot read the disk
/// (SPEC §5.1), so choosing a file is a Rust command like opening audio is.
#[tauri::command]
pub async fn pick_soundfont(app: AppHandle, state: St<'_>) -> Res<Option<SoundFontState>> {
    let (tx, rx) = std::sync::mpsc::channel();
    app.dialog()
        .file()
        .set_title("Choose a General MIDI SoundFont")
        .add_filter("SoundFont", &["sf2"])
        .pick_file(move |picked| {
            let _ = tx.send(picked);
        });
    let picked = tauri::async_runtime::spawn_blocking(move || rx.recv().ok().flatten())
        .await
        .map_err(|e| format!("file dialog failed: {e}"))?;
    let Some(path) = picked.and_then(|f| f.into_path().ok()) else {
        // Cancelled — not an error, and not a change.
        return Ok(None);
    };
    let path = path.to_string_lossy().to_string();
    let described = {
        let path = path.clone();
        tauri::async_runtime::spawn_blocking(move || describe_bank(Some(path)))
            .await
            .map_err(|e| format!("the SoundFont worker did not finish: {e}"))??
    };
    state.settings.lock().soundfont = Some(path);
    state.mark_settings_dirty();
    state.mark_state_dirty();
    state.info(format!("SoundFont → {}", described.name));
    Ok(Some(described))
}

/* ── appearance (SPEC §14/§15) ────────────────────────────────────────── */

/// Validate an appearance the webview sent.
///
/// §15 asks for unparseable input to be *rejected visibly*, so this returns
/// `Err` rather than quietly substituting a default the way the settings-file
/// reader does — a hand-edited file has nobody to tell, a settings panel does.
/// Pure, and tested: it is the boundary that keeps a font name out of the
/// webview's CSS.
fn validated_appearance(mut appearance: Appearance) -> Result<Appearance, String> {
    appearance.accent = normalise_accent(&appearance.accent)
        .ok_or_else(|| format!("\"{}\" is not a colour — use #rrggbb", appearance.accent))?;
    appearance.ui_font = normalise_font(&appearance.ui_font)
        .ok_or_else(|| format!("\"{}\" is not a usable font name", appearance.ui_font))?;
    appearance.numeric_font = normalise_font(&appearance.numeric_font)
        .ok_or_else(|| format!("\"{}\" is not a usable font name", appearance.numeric_font))?;
    Ok(appearance)
}

/// Theme, accent, fonts and size scale, in one setter for the same reason the
/// EQ has one: two sources of truth for how the app looks is how a panel ends
/// up disagreeing with the window.
///
/// Returns the normalised value (`#C9A227` comes back as `#c9a227`) so the
/// panel shows what is really in force. Both windows see it: the snapshot
/// carries `appearance`, and `onyx://state` is broadcast, so the EQ window
/// re-themes with the main one (§14).
#[tauri::command]
pub async fn set_appearance(state: St<'_>, appearance: Appearance) -> Res<Appearance> {
    let appearance = validated_appearance(appearance)?;
    let changed = {
        let mut settings = state.settings.lock();
        let changed = settings.appearance != appearance;
        settings.appearance = appearance.clone();
        changed
    };
    if changed {
        state.mark_settings_dirty();
        state.mark_state_dirty();
        // The token layer re-themes the webviews; the window *frames* are the
        // window manager's and have to be told (see `apply_native_appearance`).
        if let Some(app) = state.app() {
            crate::apply_native_appearance(&app, appearance.theme);
        }
    }
    Ok(appearance)
}

/* ── the theme document (SPEC §20) ────────────────────────────────────── */

/// Persist a theme document, or clear it with `null`.
///
/// Rust stores text and nothing else. The schema, the token catalogue, the
/// colour grammar, the clamps, the "did you mean" and the contrast audit are
/// the front end's (`src/lib/themedoc.ts`), in one implementation — a second
/// validator here would be a second thing to keep in step, and the first time
/// the two disagreed a valid theme would be refused with no way to tell which
/// half was wrong. What this layer enforces is the *file* contract: bounded
/// size, real text, no control characters (see `settings::normalise_theme_doc`).
///
/// The webview has already applied the document by the time this is called —
/// that is what makes Apply feel instant — so this is the persist half, and its
/// broadcast is what re-skins the *other* windows: the snapshot carries
/// `themeDoc` and `onyx://state` goes to every webview (SPEC §14).
#[tauri::command]
pub async fn set_theme_doc(state: St<'_>, text: Option<String>) -> Res<Option<String>> {
    let cleaned = match text {
        None => None,
        // Blank clears rather than fails: an editor emptied on purpose means
        // "no theme". `theme_doc_is_blank` is the same trim `normalise_theme_doc`
        // uses, so nothing can be blank to one and unstorable to the other.
        Some(raw) if theme_doc_is_blank(&raw) => None,
        Some(raw) => Some(normalise_theme_doc(&raw).ok_or_else(|| {
            format!(
                "that is not a theme document Onyx can store: it must be text, \
                 no control characters, at most {} KB",
                MAX_THEME_DOC_BYTES / 1024
            )
        })?),
    };
    let changed = {
        let mut settings = state.settings.lock();
        let changed = settings.theme_doc != cleaned;
        settings.theme_doc = cleaned.clone();
        changed
    };
    if changed {
        state.mark_settings_dirty();
        state.mark_state_dirty();
    }
    Ok(cleaned)
}

/* ── the window surface (SPEC §14) ────────────────────────────────────── */

/// The colour the webview is painting its own base with, so the *window* can be
/// painted the same colour underneath it.
///
/// Rust cannot work this out for itself and must not try. The designed themes it
/// can read (`surface::designed` parses `tokens.css` at compile time), but a
/// theme document (SPEC §20) can move the base surface to anything, and what a
/// document *means* is decided in one implementation, in the front end. So the
/// window that wears the document reports the resolved colour and this is where
/// it lands: `#rrggbb`, plus the theme it was resolved against, because a
/// document states dark and light separately and a colour reported for one says
/// nothing about the other.
///
/// Rejected visibly rather than substituted (§15): the only thing that may
/// reach a native window from here is a hex colour.
#[tauri::command]
pub async fn set_window_surface(app: AppHandle, color: String, theme: String) -> Res<()> {
    let resolved = surface::Resolved::parse(&theme)
        .ok_or_else(|| format!("\"{theme}\" is not a resolved theme — use dark or light"))?;
    let color = surface::hex_color(&color)
        .ok_or_else(|| format!("\"{color}\" is not a colour — use #rrggbb"))?;
    surface::report(&app, resolved, color);
    Ok(())
}

/// The escape hatch, as a command: the designed themes, the champagne accent,
/// the system fonts, no document. Atomic and always available — it is what the
/// native menu item and `Ctrl/Cmd+Alt+Shift+R` both end up calling.
#[tauri::command]
pub async fn reset_appearance(app: AppHandle, state: St<'_>) -> Res<Appearance> {
    let appearance = reset_appearance_in(state.inner());
    crate::apply_native_appearance(&app, appearance.theme);
    Ok(appearance)
}

/// The reset itself, without the IPC. Shared with [`crate::appmenu::reset`],
/// which cannot `await` a command.
pub(crate) fn reset_appearance_in(state: &Arc<AppState>) -> Appearance {
    let appearance = Appearance::default();
    // The document is going, so the surface colour reported *for* it must go
    // with it — otherwise the reset repaints the window edges in the colour of
    // the theme it just removed, until the webview gets round to reporting the
    // designed one (see `surface::forget`).
    surface::forget();
    let changed = {
        let mut settings = state.settings.lock();
        let changed = settings.appearance != appearance || settings.theme_doc.is_some();
        settings.appearance = appearance.clone();
        settings.theme_doc = None;
        changed
    };
    if changed {
        state.mark_settings_dirty();
        state.mark_state_dirty();
        state.info("Appearance reset");
    }
    appearance
}

/* ── the theme editor window (SPEC §20) ───────────────────────────────── */

/// Open the editor, or bring the one that exists forward.
#[tauri::command]
pub async fn theme_window_open(app: AppHandle) -> Res<()> {
    themewindow::open(&app, true)
}

#[tauri::command]
pub async fn theme_window_close(app: AppHandle) -> Res<()> {
    themewindow::close(&app);
    Ok(())
}

#[tauri::command]
pub async fn theme_window_toggle(app: AppHandle) -> Res<()> {
    themewindow::toggle(&app)
}

/// Is it open? For the settings panel's button, which would otherwise guess.
#[tauri::command]
pub async fn theme_window_state(app: AppHandle) -> Res<bool> {
    Ok(themewindow::is_open(&app))
}

#[tauri::command]
pub async fn device_set(state: St<'_>, name: Option<String>) -> Res<AppSnapshot> {
    let previous_rate = state.engine.engine_rate();
    let rate = state.engine.set_device(name.clone()).map_err(|e| {
        format!(
            "could not open {}: {e}",
            name.clone()
                .unwrap_or_else(|| "the system default output".into())
        )
    })?;
    if rate != previous_rate {
        // The new device runs at a different rate, so both decks hold PCM at
        // the wrong rate now.
        loader::rearm_decks(state.inner());
    }
    state.info(format!(
        "Output → {}",
        name.unwrap_or_else(|| "system default".into())
    ));
    state.remember_settings();
    state.mark_state_dirty();
    Ok(state.snapshot())
}

#[tauri::command]
pub async fn set_follow_source_rate(state: St<'_>, value: bool) -> Res<AppSnapshot> {
    state.engine.set_follow_source_rate(value);
    if value {
        // Go and get the source rate now rather than at the next track change.
        loader::rearm_decks(state.inner());
    }
    state.remember_settings();
    state.mark_state_dirty();
    Ok(state.snapshot())
}

#[tauri::command]
pub async fn meters_get(state: St<'_>) -> Res<MeterSnapshot> {
    Ok(state.engine.meters())
}

#[tauri::command]
pub async fn reset_meters(state: St<'_>) -> Res<()> {
    state.engine.reset_meters();
    Ok(())
}

/// Persistent loudness cache (SPEC §8). Both are answered from memory: the
/// settings panel must never wait on the disk.
#[tauri::command]
pub async fn cache_stats(state: St<'_>) -> Res<CacheStats> {
    Ok(state.cache.stats())
}

#[tauri::command]
pub async fn cache_clear(state: St<'_>) -> Res<CacheStats> {
    Ok(state.cache.clear())
}

/// "Reveal in Finder" / "Show in Explorer" (SPEC §5.4).
///
/// This is the one command that hands a webview-supplied string to the
/// operating system, so the string is not trusted: only a path the user has
/// already put in the playlist may be revealed. Without that check a scripting
/// bug in the front end could ask the OS to select any file on the machine —
/// `~/.ssh/id_rsa`, a mounted share — and on Windows `reveal_item_in_dir`
/// spawns `explorer.exe`, which is not somewhere arbitrary strings should go.
/// The front end only ever passes `entry.path`, so the check costs it nothing.
#[tauri::command]
pub async fn reveal_in_finder(app: AppHandle, state: St<'_>, path: String) -> Res<()> {
    if path.trim().is_empty() {
        return Err("no file to reveal".into());
    }
    let known = state
        .playlist
        .lock()
        .entries
        .iter()
        .any(|entry| entry.path == path);
    if !known {
        log::warn!("refused to reveal \"{path}\": not a playlist entry");
        return Err("that file is not in the playlist".into());
    }
    // The error goes back as the command's `Err`; the caller toasts it once.
    app.opener()
        .reveal_item_in_dir(&path)
        .map_err(|e| format!("could not reveal {path}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /* ── seek boundaries (SPEC §9.6) ──────────────────────────────────── */

    #[test]
    fn a_seek_to_exactly_the_end_is_honoured() {
        // Dragging the playhead to the far right must land *on* the end, not a
        // frame before it and not at zero.
        assert_eq!(clamp_secs(210.0, 210.0), 210.0);
        assert_eq!(clamp_secs(210.000_001, 210.0), 210.0);
        assert_eq!(clamp_secs(1e12, 210.0), 210.0);
    }

    #[test]
    fn a_seek_is_never_negative_and_never_nan() {
        for secs in [-1.0, -1e12, f64::NEG_INFINITY, f64::NAN] {
            assert_eq!(clamp_secs(secs, 210.0), 0.0, "{secs} escaped the clamp");
        }
        assert_eq!(clamp_secs(f64::INFINITY, 210.0), 210.0);
    }

    #[test]
    fn seeking_with_nothing_loaded_lands_at_zero() {
        // `transport_seek` is reachable with both decks empty (a keyboard
        // shortcut on a fresh launch), and 0/0 must not become NaN.
        for duration in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(clamp_secs(30.0, duration), 0.0, "duration {duration}");
        }
    }

    /* ── payload contracts (src/lib/types.ts) ─────────────────────────── */

    #[test]
    fn the_align_payload_matches_the_front_end() {
        let json = serde_json::to_string(&AlignResult {
            offset_frames: -441,
            offset_ms: -10.0,
            confidence: 0.82,
            polarity_inverted: true,
            applied: true,
        })
        .unwrap();
        // `AlignResult` in src/lib/types.ts, field for field.
        assert!(json.contains("\"offsetFrames\":-441"), "{json}");
        assert!(json.contains("\"offsetMs\":-10"), "{json}");
        assert!(json.contains("\"confidence\":0.82"), "{json}");
        assert!(json.contains("\"polarityInverted\":true"), "{json}");
        assert!(json.contains("\"applied\":true"), "{json}");
    }

    #[test]
    fn the_front_ends_eq_json_deserialises_into_the_core_config() {
        // Exactly what `setEq` sends for one band. A casing slip here is a
        // runtime failure with no compiler to catch it.
        let json = r#"{
            "enabled": true,
            "bands": [{
                "id": 3,
                "enabled": true,
                "kind": "highShelf",
                "freqHz": 8000,
                "gainDb": -3.5,
                "q": 0.7,
                "slopeDbOct": 12
            }]
        }"#;
        let config: EqConfig = serde_json::from_str(json).expect("front-end EQ JSON must parse");
        assert!(config.enabled);
        assert_eq!(config.bands.len(), 1);
        assert_eq!(config.bands[0].freq_hz, 8_000.0);
        assert_eq!(config.bands[0].gain_db, -3.5);
        assert_eq!(config.bands[0].slope_db_oct, 12);
    }

    #[test]
    fn the_front_ends_monitor_modes_all_deserialise() {
        // Every string in `MonitorMode` in src/lib/types.ts.
        for (wire, expected) in [
            ("stereo", MonitorMode::Stereo),
            ("mono", MonitorMode::Mono),
            ("left", MonitorMode::Left),
            ("right", MonitorMode::Right),
            ("swap", MonitorMode::Swap),
            ("side", MonitorMode::Side),
            ("flipRight", MonitorMode::FlipRight),
        ] {
            let parsed: MonitorMode =
                serde_json::from_str(&format!("\"{wire}\"")).unwrap_or_else(|e| {
                    panic!("{wire} is in the front-end union but does not parse: {e}")
                });
            assert_eq!(parsed, expected);
        }
    }

    #[test]
    fn the_front_ends_blind_modes_and_decks_deserialise() {
        assert_eq!(
            serde_json::from_str::<BlindMode>("\"abx\"").unwrap(),
            BlindMode::Abx
        );
        assert_eq!(
            serde_json::from_str::<BlindMode>("\"ab\"").unwrap(),
            BlindMode::Ab
        );
        assert_eq!(serde_json::from_str::<Deck>("\"a\"").unwrap(), Deck::A);
        assert_eq!(serde_json::from_str::<Deck>("\"b\"").unwrap(), Deck::B);
        // Anything else is a rejected command, not a default.
        assert!(serde_json::from_str::<BlindMode>("\"ABX\"").is_err());
        assert!(serde_json::from_str::<Deck>("\"c\"").is_err());
    }
}
