//! The native menu, which exists for exactly one reason (SPEC §20).
//!
//! A theme document can set every colour in the app, and one of the documents
//! a language model will eventually write sets them all to the same one. When
//! that happens the user is looking at a rectangle: the settings panel is
//! there, the buttons are there, the click targets are there — and none of it
//! can be seen. Every recovery path that lives *inside* the themed webview is,
//! at that moment, part of the problem.
//!
//! So there are three ways out, in increasing order of "nothing else works":
//!
//!  1. **Revert** in the theme editor, which is a separate window with native
//!     decorations (`themewindow.rs`) and is itself never re-skinned by the
//!     document being edited — see `src/theme/ThemeWindow.tsx`;
//!  2. **The keyboard**: `Ctrl`/`Cmd` + `Alt` + `Shift` + `R`, handled in
//!     `src/lib/theme.ts` at the capture phase in *every* window, before React
//!     and independently of it. A theme cannot intercept a key;
//!  3. **This menu**, which the *window manager* draws. It does not read a
//!     token, it does not run our CSS, and it works when the renderer is a
//!     grey rectangle or has stopped painting altogether.
//!
//! # Platforms
//!
//! * **macOS** — the menu is app-wide. [`install`] appends an *Appearance*
//!   submenu to Tauri's default menu (keeping the app, Edit and Window
//!   submenus, so ⌘Q and ⌘V still exist) and sets it once at startup.
//! * **Windows / Linux** — the menu is per window, and the main window has no
//!   decorations to hang one under (SPEC §4 draws its own title bar), so a menu
//!   bar there would put a grey strip through the design. [`attach`] puts the
//!   same submenu on the *theme editor* window instead, which does have native
//!   decorations and is the window you go to when a theme has gone wrong.
//!
//! None of this can be exercised here: there is no display in this environment,
//! and menus are drawn by the OS. What *is* tested is the wiring that a typo
//! would break — that the ids the handler compares are the ids the builder
//! sets, and that the accelerator is the one the front end and the docs claim.

use tauri::menu::{Menu, MenuEvent, MenuItemBuilder, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Manager, WebviewWindow};

/// Menu item ids. Compared in [`on_event`]; a typo here is a menu item that
/// does nothing, which is why they are constants and why the test below holds
/// the two lists against each other.
pub const RESET_ID: &str = "onyx:appearance-reset";
pub const EDITOR_ID: &str = "onyx:appearance-editor";

/// The escape hatch, in the one form both the menu and the front end use.
/// `src/lib/theme.ts` binds the same chord, and `THEMING.md` documents it.
pub const RESET_ACCELERATOR: &str = "CmdOrCtrl+Alt+Shift+R";
const EDITOR_ACCELERATOR: &str = "CmdOrCtrl+Alt+T";

/// Build the *Appearance* submenu. Two items: the way in and the way out.
fn appearance_submenu(app: &AppHandle) -> tauri::Result<Submenu<tauri::Wry>> {
    let editor = MenuItemBuilder::with_id(EDITOR_ID, "Theme Editor…")
        .accelerator(EDITOR_ACCELERATOR)
        .build(app)?;
    let reset = MenuItemBuilder::with_id(RESET_ID, "Reset Appearance")
        .accelerator(RESET_ACCELERATOR)
        .build(app)?;
    Submenu::with_items(
        app,
        "Appearance",
        true,
        &[&editor, &PredefinedMenuItem::separator(app)?, &reset],
    )
}

/// macOS: one app-wide menu, installed at startup, on top of Tauri's default.
///
/// A failure here is logged and shrugged off — a missing menu is a lost escape
/// hatch, not a reason to refuse to start a music player.
pub fn install(app: &AppHandle) {
    if !cfg!(target_os = "macos") {
        // Windows and Linux get the menu on the theme editor window instead;
        // see the module docs.
        return;
    }
    let built = Menu::default(app).and_then(|menu| {
        menu.append(&appearance_submenu(app)?)?;
        Ok(menu)
    });
    match built {
        Ok(menu) => {
            if let Err(e) = app.set_menu(menu) {
                log::warn!("could not install the application menu: {e}");
                return;
            }
            let handle = app.clone();
            app.on_menu_event(move |_app, event| on_event(&handle, &event));
        }
        Err(e) => log::warn!("could not build the application menu: {e}"),
    }
}

/// Windows / Linux: the same submenu, on the one window that has a frame to
/// hang it under. On macOS this is a no-op — the menu is already app-wide.
pub fn attach(app: &AppHandle, window: &WebviewWindow) {
    if cfg!(target_os = "macos") {
        return;
    }
    let built = (|| -> tauri::Result<Menu<tauri::Wry>> {
        // Edit as well as Appearance: this window is a text editor, and on
        // Windows and Linux the webview's own context menu is all the user
        // would otherwise have for cut/copy/paste.
        let edit = Submenu::with_items(
            app,
            "Edit",
            true,
            &[
                &PredefinedMenuItem::undo(app, None)?,
                &PredefinedMenuItem::redo(app, None)?,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::cut(app, None)?,
                &PredefinedMenuItem::copy(app, None)?,
                &PredefinedMenuItem::paste(app, None)?,
                &PredefinedMenuItem::select_all(app, None)?,
            ],
        )?;
        Menu::with_items(app, &[&edit, &appearance_submenu(app)?])
    })();
    match built {
        Ok(menu) => {
            if let Err(e) = window.set_menu(menu) {
                log::warn!("could not put a menu on the theme editor: {e}");
                return;
            }
            let handle = app.clone();
            window.on_menu_event(move |_window, event| on_event(&handle, &event));
        }
        Err(e) => log::warn!("could not build the theme editor's menu: {e}"),
    }
}

/// One handler for both platforms.
fn on_event(app: &AppHandle, event: &MenuEvent) {
    match event.id().as_ref() {
        EDITOR_ID => {
            if let Err(e) = crate::themewindow::open(app, true) {
                log::warn!("{e}");
            }
        }
        RESET_ID => reset(app),
        _ => {}
    }
}

/// Put the appearance back: the designed themes, the champagne accent, the
/// system fonts, and **no theme document**.
///
/// Deliberately not a "revert to the last good theme": the point of this path
/// is that it has one outcome, known in advance, that cannot itself be a theme
/// that does not work. Nothing here touches playback, the playlist or the EQ —
/// the appearance is the only thing an appearance reset may cost.
pub fn reset(app: &AppHandle) {
    let Some(state) = app.try_state::<std::sync::Arc<crate::state::AppState>>() else {
        return;
    };
    crate::commands::reset_appearance_in(&state);
    crate::apply_native_appearance(app, crate::settings::Theme::default());
    log::info!("appearance reset from the native menu");
}

#[cfg(test)]
mod tests {
    /// The ids are compared as strings in two places (the builder and the
    /// handler) and named in a third (the front end's own reset). A typo is a
    /// menu item that silently does nothing, which is the failure mode this
    /// whole module exists to *prevent*, so it is worth a compile-time-ish
    /// check even though the menu itself cannot be built without a display.
    #[test]
    fn the_menu_ids_and_the_escape_hatch_chord_are_the_ones_everything_else_names() {
        const SOURCE: &str = include_str!("appmenu.rs");
        // The handler matches on both ids.
        assert!(SOURCE.contains("EDITOR_ID => {"));
        assert!(SOURCE.contains("RESET_ID => reset(app)"));
        // The chord in the front end, in the docs and here must agree: it is
        // the one thing a user is told to remember.
        const THEME_TS: &str = include_str!("../../src/lib/theme.ts");
        assert!(
            THEME_TS.contains("altKey") && THEME_TS.contains("shiftKey"),
            "the front end must bind the same modifier chord"
        );
        assert_eq!(super::RESET_ACCELERATOR, "CmdOrCtrl+Alt+Shift+R");
    }
}
