//! The EQ lives in its own window (SPEC §12).
//!
//! It is a real `WebviewWindow` with its own document (`eq.html`), not a
//! `position: fixed` panel pretending to float: an engineer drags it onto a
//! second monitor, works the curve while the main window keeps playing, and
//! closes it without touching playback — the way a plugin editor behaves.
//!
//! # Who owns what
//!
//! Rust is the single authority for everything about this window:
//!
//! * **whether it exists** — the webview cannot create windows (the renderer
//!   holds no `core:webview:allow-create-webview-window`; see
//!   `capabilities/eq.json`). `eq_window_open` is a `#[tauri::command]`, so
//!   "open the EQ" is one call from either window and the answer is always the
//!   same window;
//! * **whether it is pinned** on top, and **whether it was open at exit** —
//!   both live in `settings.json` (SPEC §12 asks for panel visibility to
//!   persist), and are pushed to every webview as [`EQ_WINDOW_EVENT`] so the
//!   main window's EQ button reflects reality rather than its own guess;
//! * **the EQ config itself** — unchanged: the engine owns it, the snapshot
//!   carries it, and the one editor (this window) sends the whole thing back
//!   through `set_eq`. There is deliberately no second band list anywhere.
//!
//! Geometry is *not* owned here: `tauri-plugin-window-state` already remembers
//! size and position per label, including this one, so the window comes back
//! where the user left it. The size below is only the first-run default.
//!
//! # The two things that must not outlive the window
//!
//! The analyser (`set_spectrum_enabled`) and the band-solo audition bandpass
//! belong to this window's lifetime. A closing window does not reliably run
//! JavaScript teardown — the webview is destroyed, `useEffect` cleanups and
//! `beforeunload` are not a contract — so both are switched off from
//! [`on_destroyed`] as well. That is not a belt-and-braces nicety: without it,
//! closing the window mid-sweep would leave the engineer monitoring a narrow
//! bandpass with nothing on screen to explain it, and the FFT would keep
//! running for a panel nobody can see.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::Serialize;
use tauri::{
    AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
};

use crate::state::{AppState, MAIN_WINDOW};

/// Window label. Matches `capabilities/eq.json` and `src/lib/eqwindow.ts`.
pub const EQ_WINDOW: &str = "eq";
/// `{ open, pinned }`, broadcast to every webview whenever either changes.
pub const EQ_WINDOW_EVENT: &str = "onyx://eq-window";
/// The second HTML entry point (`eq.html`, built by `vite.config.ts`).
pub const EQ_ENTRY: &str = "eq.html";

/// First-run size. A curve editor is wide and shallow: the frequency axis is
/// four decades and the band list needs ~300 px beside it.
const DEFAULT_W: f64 = 940.0;
const DEFAULT_H: f64 = 560.0;
/// Below this the graph stops being usable as an editor.
const MIN_W: f64 = 620.0;
const MIN_H: f64 = 380.0;
/// First-run offset from the main window's top-left, so the new window is
/// obviously a child of this app and not stacked exactly on top of it.
const CASCADE: f64 = 34.0;

/// `EqWindowState` in `src/lib/types.ts`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EqWindowState {
    pub open: bool,
    pub pinned: bool,
}

/// Set while the app is on its way out, so a window torn down by the shutdown
/// is not mistaken for one the *user* closed.
///
/// Without this, quitting with the EQ open wrote `eqWindowOpen: false` on the
/// way past and the window did not come back on the next launch — the one bug
/// this flag exists for.
static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);

pub fn begin_shutdown() {
    SHUTTING_DOWN.store(true, Ordering::Release);
}

fn shutting_down() -> bool {
    SHUTTING_DOWN.load(Ordering::Acquire)
}

/* ── state ───────────────────────────────────────────────────────────────── */

fn app_state(app: &AppHandle) -> Option<Arc<AppState>> {
    app.try_state::<Arc<AppState>>()
        .map(|s| Arc::clone(s.inner()))
}

pub fn is_open(app: &AppHandle) -> bool {
    app.get_webview_window(EQ_WINDOW).is_some()
}

pub fn pinned(app: &AppHandle) -> bool {
    app_state(app).is_none_or(|s| s.settings.lock().eq_window_pinned)
}

/// What both webviews are told. `open` is asked of the window system rather
/// than of a flag, so it cannot drift from what is on screen.
pub fn snapshot(app: &AppHandle) -> EqWindowState {
    EqWindowState {
        open: is_open(app),
        pinned: pinned(app),
    }
}

/// Broadcast to *every* window: the main window's EQ button and the EQ
/// window's pin control both read from this, so neither has to guess.
pub fn announce(app: &AppHandle) {
    let state = snapshot(app);
    if let Err(e) = app.emit(EQ_WINDOW_EVENT, state) {
        log::debug!("could not emit {EQ_WINDOW_EVENT} ({e}); a webview is going away");
    }
}

/// Remember, in `settings.json`, whether the EQ window should come back.
fn remember_open(app: &AppHandle, open: bool) {
    let Some(state) = app_state(app) else { return };
    let changed = {
        let mut settings = state.settings.lock();
        let changed = settings.eq_window_open != open;
        settings.eq_window_open = open;
        changed
    };
    if changed {
        state.mark_settings_dirty();
    }
}

/* ── open / close ────────────────────────────────────────────────────────── */

/// Open the EQ window, or bring the existing one forward.
///
/// `focus` is false only for the restore at launch: a tool window that steals
/// the keyboard from the main window while the app is still starting is a
/// worse first impression than one that comes back quietly.
pub fn open(app: &AppHandle, focus: bool) -> Result<(), String> {
    // Never a second one. Every caller — the `E` key in either window, the EQ
    // button, the restore at launch — funnels through here.
    if let Some(window) = app.get_webview_window(EQ_WINDOW) {
        let _ = window.unminimize();
        let _ = window.show();
        if focus {
            let _ = window.set_focus();
        }
        remember_open(app, true);
        announce(app);
        return Ok(());
    }

    let pinned = pinned(app);
    let mut builder = WebviewWindowBuilder::new(app, EQ_WINDOW, WebviewUrl::App(EQ_ENTRY.into()))
        .title("Onyx — Equaliser")
        .inner_size(DEFAULT_W, DEFAULT_H)
        .min_inner_size(MIN_W, MIN_H)
        .resizable(true)
        .maximizable(false)
        .minimizable(true)
        // A tool window that sits over the thing it edits, like every plugin
        // editor. It is a preference, not a law — see `set_pinned`.
        .always_on_top(pinned)
        // Reaching into an unfocused editor and grabbing a node should work on
        // the first click, as it does in the main window.
        .accept_first_mouse(true)
        // Native decorations, deliberately, unlike the main window: this is a
        // secondary window the user must be able to move and close with the
        // system's own affordances even if the webview is wedged, and drawing
        // our own chrome here would mean handing the renderer
        // `core:window:allow-start-dragging` and `…:allow-close` for nothing.
        .decorations(true)
        // The colour the window manager paints under the webview. Given at
        // creation, not only after it, because the window is on screen with
        // nothing but its own background for the frames before the webview's
        // first paint — and an untold window's background is the system's grey,
        // which is the light rim of `crate::surface`.
        .background_color(crate::surface::for_new_window(app, EQ_WINDOW))
        .focused(focus);
    if let Some((x, y)) = first_run_position(app) {
        builder = builder.position(x, y);
    }

    let window = builder
        .build()
        .map_err(|e| format!("could not open the EQ window: {e}"))?;
    // A new window is born with the OS theme; the user's choice lives in
    // `settings.json`. On Windows the frame is per-window, so a light-themed
    // session would otherwise open a dark-framed editor (SPEC §14). This also
    // re-states the surface colour given to the builder above, which costs one
    // message and covers the case where the theme moved between the two.
    if let Some(state) = app_state(app) {
        let theme = state.settings.lock().appearance.theme;
        crate::apply_native_appearance(app, theme);
    }
    watch(app, &window);
    remember_open(app, true);
    announce(app);
    log::info!("EQ window opened");
    Ok(())
}

/// Close it if it is there. Not an error if it is not: `E` is a toggle and the
/// user may have closed the window with its own close button a moment ago.
pub fn close(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(EQ_WINDOW) {
        if let Err(e) = window.close() {
            log::warn!("could not close the EQ window: {e}");
        }
    }
    // `close()` is asynchronous (it asks the window system); the destroy
    // handler below does the rest.
}

pub fn toggle(app: &AppHandle) -> Result<(), String> {
    if is_open(app) {
        close(app);
        Ok(())
    } else {
        open(app, true)
    }
}

/// Always-on-top is a preference: a second monitor makes it pointless, and
/// some engineers hate it. Persisted, applied live.
pub fn set_pinned(app: &AppHandle, value: bool) {
    if let Some(state) = app_state(app) {
        let changed = {
            let mut settings = state.settings.lock();
            let changed = settings.eq_window_pinned != value;
            settings.eq_window_pinned = value;
            changed
        };
        if changed {
            state.mark_settings_dirty();
        }
    }
    if let Some(window) = app.get_webview_window(EQ_WINDOW) {
        if let Err(e) = window.set_always_on_top(value) {
            log::warn!("could not change the EQ window's always-on-top: {e}");
        }
    }
    announce(app);
}

/// Reopen the EQ window if it was open when the app was last quit (SPEC §12
/// asks for panel visibility to persist). Called from `setup`.
pub fn restore(app: &AppHandle) {
    let wanted = app_state(app).is_some_and(|s| s.settings.lock().eq_window_open);
    if !wanted {
        return;
    }
    if let Err(e) = open(app, false) {
        // Not fatal: the player still plays, the user can press `E`.
        log::warn!("could not restore the EQ window: {e}");
        remember_open(app, false);
    }
}

/* ── lifecycle ───────────────────────────────────────────────────────────── */

fn watch(app: &AppHandle, window: &WebviewWindow) {
    let handle = app.clone();
    window.on_window_event(move |event| {
        // Only `Destroyed`: `CloseRequested` fires while the window is still
        // there, so a snapshot taken from it would report `open: true` for a
        // window that is on its way out.
        if matches!(event, WindowEvent::Destroyed) {
            on_destroyed(&handle);
        }
    });
}

/// The EQ window is gone. Put the engine back the way a closed panel implies.
fn on_destroyed(app: &AppHandle) {
    if let Some(state) = app_state(app) {
        // SPEC §12: a closed panel costs zero FFT, and no audition bandpass
        // may survive the window that started it. Both are idempotent.
        state.engine.set_spectrum_enabled(false);
        state.engine.set_eq_audition(None, 1.0);
    }
    if shutting_down() {
        // Quitting: leave `eqWindowOpen` alone so the window comes back next
        // launch. Nothing is listening for the event either.
        return;
    }
    remember_open(app, false);
    announce(app);
    log::info!("EQ window closed");
}

/// Closing the main window quits Onyx.
///
/// Tauri exits when the *last* window closes, so without this, closing the main
/// window while the EQ was open left the app running as a lone EQ editor with
/// no transport, no playlist and — on Windows and Linux, where the main window
/// draws its own chrome — no obvious way back.
pub fn watch_main(app: &AppHandle) {
    let Some(main) = app.get_webview_window(MAIN_WINDOW) else {
        return;
    };
    let handle = app.clone();
    main.on_window_event(move |event| {
        if matches!(event, WindowEvent::CloseRequested { .. }) {
            begin_shutdown();
            close(&handle);
        }
    });
}

/// First-run placement: cascaded off the main window's top-left, clamped so it
/// never lands with its title bar off the top of the screen. Once the user has
/// moved it, `tauri-plugin-window-state` takes over and this is not consulted
/// again.
fn first_run_position(app: &AppHandle) -> Option<(f64, f64)> {
    let main = app.get_webview_window(MAIN_WINDOW)?;
    let scale = main.scale_factor().ok()?;
    let pos = main.outer_position().ok()?.to_logical::<f64>(scale);
    let size = main.inner_size().ok()?.to_logical::<f64>(scale);
    // Bottom-ish and centred on the main window: the curve editor wants to sit
    // where the drawer used to, over the playlist rather than over the
    // waveform the user is reading.
    let x = pos.x + (size.width - DEFAULT_W) / 2.0;
    let y = pos.y + (size.height - DEFAULT_H).max(CASCADE) - CASCADE;
    Some((x.max(pos.x + CASCADE), y.max(pos.y + CASCADE)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The label, the entry point and the capability file have to agree, and
    /// none of that is checked by the compiler: a mismatch is a blank window
    /// (wrong URL) or a webview with no `listen` permission (wrong label) —
    /// both only visible by launching the app, which CI cannot do.
    #[test]
    fn the_eq_window_label_and_entry_point_line_up_with_everything_that_names_them() {
        const CAPABILITY: &str = include_str!("../capabilities/eq.json");
        const VITE_CONFIG: &str = include_str!("../../vite.config.ts");
        // The document itself must exist, or the window opens blank.
        const EQ_HTML: &str = include_str!("../../eq.html");

        let capability: serde_json::Value =
            serde_json::from_str(CAPABILITY).expect("capabilities/eq.json must be valid JSON");
        let windows = capability["windows"]
            .as_array()
            .expect("the capability must name its windows");
        assert_eq!(
            windows,
            &vec![serde_json::json!(EQ_WINDOW)],
            "the EQ capability must apply to the EQ window and nothing else"
        );

        // Least privilege (SPEC §5.1): the EQ webview gets the three event
        // verbs and nothing more. It cannot create windows, so opening the EQ
        // window has to stay a #[tauri::command] — which is the point.
        let permissions: Vec<&str> = capability["permissions"]
            .as_array()
            .expect("a permission list")
            .iter()
            .map(|p| p.as_str().expect("string permissions"))
            .collect();
        assert_eq!(
            permissions,
            vec![
                "core:event:allow-listen",
                "core:event:allow-unlisten",
                "core:event:allow-emit",
            ],
            "the EQ window's permissions widened"
        );

        // Vite has to emit the document this window points at.
        assert!(
            VITE_CONFIG.contains(EQ_ENTRY),
            "vite.config.ts does not build {EQ_ENTRY}"
        );
        assert!(
            EQ_HTML.contains("src/eq/main.tsx"),
            "eq.html does not load the EQ entry point"
        );
    }

    #[test]
    fn the_window_payload_matches_the_front_end() {
        let json = serde_json::to_string(&EqWindowState {
            open: true,
            pinned: false,
        })
        .unwrap();
        // `EqWindowState` in src/lib/types.ts, field for field.
        assert_eq!(json, r#"{"open":true,"pinned":false}"#);
    }

    /// These are compile-time constants, so this is a compile-time assertion:
    /// a bad edit fails to build rather than failing a test run.
    #[test]
    fn the_first_run_size_is_a_usable_curve_editor() {
        // Wider than it is tall (four decades of frequency), and the minimum
        // still leaves room for the graph beside the band list.
        const _: () = assert!(DEFAULT_W > DEFAULT_H);
        const _: () = assert!(MIN_W >= 620.0 && MIN_H >= 360.0);
        const _: () = assert!(DEFAULT_W > MIN_W && DEFAULT_H > MIN_H);
    }
}
