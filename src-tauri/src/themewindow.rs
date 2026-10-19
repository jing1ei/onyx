//! The theme editor lives in its own window (SPEC §20).
//!
//! Same shape as [`crate::eqwindow`], and for the same reason: it is a real
//! `WebviewWindow` with its own document (`theme.html`), so the document being
//! edited can sit on a second monitor beside the app it re-skins, and so the
//! editor keeps working while the *main* window is being repainted by whatever
//! was just pasted.
//!
//! That last point is not decoration. The whole feature is "paste a theme and
//! see what happens", and what happens is sometimes "the main window turns into
//! a flat grey rectangle". A separate window with **native decorations** means
//! the editor still has a title bar the window manager drew, a close button the
//! theme cannot touch, and — through [`crate::appmenu`] — a native *Reset
//! appearance* item that does not depend on the renderer being legible.
//!
//! # Who owns what
//!
//! * **whether the window exists** — Rust. No webview holds
//!   `core:webview:allow-create-webview-window`; `theme_window_*` are commands.
//! * **the document itself** — `settings.json` (`themeDoc`), written through
//!   `set_theme_doc` and broadcast to every webview on the snapshot, which is
//!   how applying a theme in the editor re-skins the main and EQ windows
//!   without any of the three knowing the others exist.
//! * **what a document *means*** — the front end, in one implementation
//!   (`src/lib/themedoc.ts`). Rust never parses a theme.
//!
//! Unlike the EQ window this one is **not** restored at launch and **not**
//! pinned: it is a thing you open, paste into, and close. Reopening a text
//! editor over the player every morning would be an odd thing for a music app
//! to do.

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};

use crate::state::MAIN_WINDOW;

/// Window label. Matches `capabilities/theme.json` and `src/lib/themeio.ts`.
pub const THEME_WINDOW: &str = "theme";
/// The third HTML entry point (`theme.html`, built by `vite.config.ts`).
pub const THEME_ENTRY: &str = "theme.html";

/// First-run size. A code editor is tall: a theme document is ~420 lines and
/// the errors list sits under it.
const DEFAULT_W: f64 = 720.0;
const DEFAULT_H: f64 = 840.0;
/// Below this the textarea stops being a place anyone would edit JSON.
const MIN_W: f64 = 520.0;
const MIN_H: f64 = 420.0;
const CASCADE: f64 = 28.0;

pub fn is_open(app: &AppHandle) -> bool {
    app.get_webview_window(THEME_WINDOW).is_some()
}

/// Open the editor, or bring the existing one forward. Never a second one.
pub fn open(app: &AppHandle, focus: bool) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(THEME_WINDOW) {
        let _ = window.unminimize();
        let _ = window.show();
        if focus {
            let _ = window.set_focus();
        }
        return Ok(());
    }

    let mut builder =
        WebviewWindowBuilder::new(app, THEME_WINDOW, WebviewUrl::App(THEME_ENTRY.into()))
            .title("Onyx — Theme")
            .inner_size(DEFAULT_W, DEFAULT_H)
            .min_inner_size(MIN_W, MIN_H)
            .resizable(true)
            .maximizable(true)
            .minimizable(true)
            .accept_first_mouse(true)
            // Native decorations, deliberately — see the module docs. This is
            // the window you use to undo a theme that ate the other windows.
            .decorations(true)
            // The window manager's own background, under the webview. The
            // *designed* surface, never the document's: this window does not
            // wear the theme it is editing, so a document that paints
            // everything one colour must not reach its edges either
            // (`crate::surface::pick`).
            .background_color(crate::surface::for_new_window(app, THEME_WINDOW))
            .focused(focus);
    if let Some((x, y)) = first_run_position(app) {
        builder = builder.position(x, y);
    }
    let window = builder
        .build()
        .map_err(|e| format!("could not open the theme editor: {e}"))?;

    // A new window is born with the OS theme; on Windows the frame is
    // per-window (SPEC §14).
    if let Some(state) = app.try_state::<std::sync::Arc<crate::state::AppState>>() {
        let theme = state.settings.lock().appearance.theme;
        crate::apply_native_appearance(app, theme);
    }
    // On Windows and Linux the menu is per-window, and this is the window that
    // must always be able to undo a theme. On macOS the app menu already
    // carries it and this is a no-op.
    crate::appmenu::attach(app, &window);
    watch(&window);
    log::info!("theme editor opened");
    Ok(())
}

pub fn close(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(THEME_WINDOW) {
        if let Err(e) = window.close() {
            log::warn!("could not close the theme editor: {e}");
        }
    }
}

pub fn toggle(app: &AppHandle) -> Result<(), String> {
    if is_open(app) {
        close(app);
        Ok(())
    } else {
        open(app, true)
    }
}

fn watch(window: &WebviewWindow) {
    window.on_window_event(move |event| {
        if matches!(event, WindowEvent::Destroyed) {
            // Nothing to undo: the theme in force lives in `settings.json` and
            // is applied by every webview independently. Closing the editor
            // deliberately does *not* revert — a theme you applied is a theme
            // you chose, and losing it by closing a window would be the
            // opposite of "the engine must not break".
            log::info!("theme editor closed");
        }
    });
}

/// Closing the main window closes this one too, so the app can quit: Tauri
/// exits when the last window goes, and a lone theme editor is not an app.
pub fn watch_main(app: &AppHandle) {
    let Some(main) = app.get_webview_window(MAIN_WINDOW) else {
        return;
    };
    let handle = app.clone();
    main.on_window_event(move |event| {
        if matches!(event, WindowEvent::CloseRequested { .. }) {
            close(&handle);
        }
    });
}

/// Cascaded off the main window's top-left, never off the top of the screen.
/// After the first run `tauri-plugin-window-state` owns the geometry.
fn first_run_position(app: &AppHandle) -> Option<(f64, f64)> {
    let main = app.get_webview_window(MAIN_WINDOW)?;
    let scale = main.scale_factor().ok()?;
    let pos = main.outer_position().ok()?.to_logical::<f64>(scale);
    let size = main.inner_size().ok()?.to_logical::<f64>(scale);
    // To the right of the main window if it fits on its own edge, so the app
    // and the document that is restyling it are both visible at once.
    let x = pos.x + size.width - DEFAULT_W - CASCADE;
    Some((x.max(pos.x + CASCADE), pos.y + CASCADE))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Same contract check as the EQ window's: the label, the entry point, the
    /// capability file and the Vite input list all name each other, and none of
    /// that is checked by the compiler. A mismatch is a blank window or a
    /// webview with no permissions — both only visible by launching the app.
    #[test]
    fn the_theme_window_label_and_entry_point_line_up_with_everything_that_names_them() {
        const CAPABILITY: &str = include_str!("../capabilities/theme.json");
        const VITE_CONFIG: &str = include_str!("../../vite.config.ts");
        const THEME_HTML: &str = include_str!("../../theme.html");

        let capability: serde_json::Value =
            serde_json::from_str(CAPABILITY).expect("capabilities/theme.json must be valid JSON");
        assert_eq!(
            capability["windows"].as_array(),
            Some(&vec![serde_json::json!(THEME_WINDOW)]),
            "the theme capability must apply to the theme window and nothing else"
        );

        // Least privilege (SPEC §5.1): the same three event verbs the EQ window
        // gets, and nothing else. In particular no window-creation permission,
        // so the editor cannot conjure a fourth window, and no clipboard
        // permission — copying is `navigator.clipboard` in the webview, which
        // needs nothing from Tauri.
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
            "the theme window's permissions widened"
        );

        assert!(
            VITE_CONFIG.contains(THEME_ENTRY),
            "vite.config.ts does not build {THEME_ENTRY}"
        );
        assert!(
            THEME_HTML.contains("src/theme/main.tsx"),
            "theme.html does not load the theme editor entry point"
        );
    }

    #[test]
    fn the_first_run_size_is_a_usable_code_editor() {
        // Taller than it is wide: a theme document is a long list of short
        // lines, and the problems list lives under the textarea.
        const _: () = assert!(DEFAULT_H > DEFAULT_W);
        const _: () = assert!(DEFAULT_W > MIN_W && DEFAULT_H > MIN_H);
        const _: () = assert!(MIN_W >= 480.0 && MIN_H >= 400.0);
    }
}
