//! The window *surface* — the one colour the window manager paints, not us.
//!
//! # The bug this module exists for
//!
//! An `NSWindow` is created with a background colour of its own, and unless it
//! is told otherwise that colour is the system's `windowBackgroundColor`: light
//! grey in the light appearance, mid-grey in the dark one. It is never
//! obsidian. Before this module, nothing in `tauri.conf.json` or
//! `tauri.macos.conf.json` set `backgroundColor` and no window builder passed a
//! colour, and `tao` only touches `setBackgroundColor:` when the window is
//! transparent or a colour was given — so every Onyx window kept the system grey
//! underneath the webview.
//!
//! That colour is invisible where the webview covers it, and the webview covers
//! nearly everything. Where it is *not* covered is the outside edge: macOS
//! clips a decorated window to rounded corners and antialiases that curve
//! against the window's own background, and the window is on screen with its
//! background and nothing else for the moment between "created" and "the
//! webview's first frame". Both read as a light rim around a dark app — which
//! is exactly the report this module answers.
//!
//! # What it does *not* claim to fix
//!
//! macOS draws its own 1 px stroke around a decorated window in the dark
//! appearance. That stroke belongs to AppKit, is shared with Finder and every
//! other native window, and no window background colour removes it; only
//! dropping native decorations would, at the price of the traffic lights and
//! native edge-resize. See the note in README's *Known limitations*.
//!
//! # What the pinned versions actually support
//!
//! Read out of the locked sources, not assumed — `tauri 2.11.5`,
//! `tauri-runtime-wry 2.11.4`, `tao 0.35.3`, `wry 0.55.1`:
//!
//! * **At creation.** `WebviewWindowBuilder::background_color` and the
//!   `backgroundColor` key in `tauri.conf.json` both reach
//!   `tao`'s macOS window constructor, which calls `setBackgroundColor:` when a
//!   colour was given (`platform_impl/macos/window.rs`).
//! * **After creation — yes.** `WebviewWindow::set_background_color` is not a
//!   creation-only setter: it dispatches `WindowMessage::SetBackgroundColor` to
//!   the event loop, which calls the same `NSWindow` setter. So [`apply`] is a
//!   real runtime repaint on macOS and Windows, not a hopeful one. What is *not*
//!   implemented on macOS is the second half of that call, the **webview**
//!   layer: `wry`'s `set_background_color` has a body only under
//!   `target_os = "ios"` and returns `Ok(())` elsewhere. Nothing here depends on
//!   it — the webview paints its own background in CSS — but it is why this
//!   module never treats "the call succeeded" as "the pixels changed".
//! * **The main window is the one this module cannot reach at creation.** It is
//!   declared in `tauri.conf.json` and built before the `setup` hook runs, so no
//!   Rust value can reach *its* constructor; the config states the default
//!   theme's surface as a literal (SPEC §14 defaults to dark) and a test below
//!   pins that literal to `tokens.css`. The resolved colour is pushed in
//!   `setup`, which is before `app.run()` turns the run loop and therefore — on
//!   the reading of AppKit that has not been verified on a Mac here — before the
//!   first frame is presented.
//!
//! # Why the colour is not a constant
//!
//! The app has two designed themes plus `system` (SPEC §14) and, since v4, a
//! theme document that can move any token including the base surface
//! (SPEC §20). A window painted `#0a0a0c` would put a *dark* rim around a
//! light or custom theme, which is the same bug in a different colour. So the
//! surface is resolved, in this order:
//!
//! 1. **what the webview reports.** `src/lib/surface.ts` reads the colour the
//!    main window is actually painting (`body { background: var(--ink-900) }`,
//!    after any theme document) and sends it through
//!    [`crate::commands::set_window_surface`], tagged with the theme it was
//!    resolved against. Rust never parses a theme — that stays one
//!    implementation, in the front end (see [`crate::commands::set_theme_doc`]).
//! 2. **the designed value for the resolved theme**, read out of
//!    `src/styles/tokens.css` at compile time by [`designed`]. This is what a
//!    window born before any webview has spoken gets — and, for the dark theme,
//!    the value the main window's config literal is held to — and what a report
//!    tagged with the *other* theme falls back to.
//!
//! The theme editor window is deliberately excluded from (1): it never wears
//! the document it is editing (`ignoreThemeDoc()` in `src/theme/main.tsx`), so
//! its surface has to be the designed one or the window you recover a bad theme
//! from would get a rim of that very theme.

use std::sync::OnceLock;

use parking_lot::Mutex;
use tauri::window::Color;
use tauri::{AppHandle, Manager};

use crate::settings;
use crate::themewindow::THEME_WINDOW;

/// The token every window's base surface is painted with: `tokens.css` ends
/// `body { background: … }` on it, and both `--bg-app` and `--bg-eq` are
/// gradients *over* it. Named here because [`designed`] reads it out of the
/// stylesheet, and `src/lib/surface.ts` names the same token on the other side.
pub(crate) const SURFACE_TOKEN: &str = "--ink-900";

/// The stylesheet is the source of truth for the designed themes, exactly as it
/// is for the front end's token catalogue (`src/lib/tokens.ts` imports it with
/// `?raw` rather than restating it). Included, not re-typed, so a designer
/// nudging obsidian moves the window surface with it.
const TOKENS_CSS: &str = include_str!("../../src/styles/tokens.css");

/// Last resort, used only if the stylesheet stops declaring [`SURFACE_TOKEN`]
/// in a form this module can read. A test asserts these are what the sheet
/// actually says, so they cannot quietly become the values in force.
const FALLBACK_DARK: Color = Color(0x0a, 0x0a, 0x0c, 0xff);
const FALLBACK_LIGHT: Color = Color(0xef, 0xeb, 0xe1, 0xff);

/// A theme after `system` has been resolved. There is no third case: a window
/// surface is a colour, so "follow the OS" has to become dark or light before
/// it can be painted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Resolved {
    Dark,
    Light,
}

impl Resolved {
    /// The two strings `src/lib/appearance.ts` calls a `ResolvedTheme`; they
    /// arrive on the IPC boundary and are written to `<html data-theme>`.
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "dark" => Some(Resolved::Dark),
            "light" => Some(Resolved::Light),
            _ => None,
        }
    }
}

/// What the main window last reported it was painting, and the theme it
/// resolved that against.
///
/// A static rather than a field on `AppState` for the same reason
/// `eqwindow::SHUTTING_DOWN` is one: it is one cosmetic value for the whole
/// process, read from window-creation paths that may run before the managed
/// state exists.
///
/// The tag is what keeps a theme switch honest. A document states dark *and*
/// light surfaces, so a colour reported for light says nothing about dark; a
/// report whose tag is not the theme in force is ignored rather than painted.
static REPORTED: Mutex<Option<(Resolved, Color)>> = Mutex::new(None);

/// The colour the *webview* says it is painting. Called by
/// [`crate::commands::set_window_surface`].
pub(crate) fn report(app: &AppHandle, theme: Resolved, color: Color) {
    let changed = {
        let mut reported = REPORTED.lock();
        let changed = *reported != Some((theme, color));
        *reported = Some((theme, color));
        changed
    };
    if changed {
        log::debug!(
            "the webview paints {:?} in the {theme:?} theme; retargeting the window surfaces",
            hex_of(color)
        );
        apply(app, settings_theme(app));
    }
}

/// Forget it. Called by the appearance reset (`reset_appearance_in`), which
/// clears the theme document: without this, the reset would keep repainting the
/// window surface in the colour of the theme it just removed until the webview
/// got round to reporting the designed one.
pub(crate) fn forget() {
    *REPORTED.lock() = None;
}

/* ── the designed themes ─────────────────────────────────────────────────── */

/// The base surface of a designed theme, as `src/styles/tokens.css` declares
/// it. Parsed once.
pub(crate) fn designed(theme: Resolved) -> Color {
    static CACHE: OnceLock<(Color, Color)> = OnceLock::new();
    let (dark, light) = CACHE.get_or_init(|| {
        let read = |theme, fallback| {
            surface_in(TOKENS_CSS, theme).unwrap_or_else(|| {
                // Not a failure: a player must still open a window. It is a bug
                // in this parser or in the sheet, and the test below is what
                // normally catches it.
                log::warn!(
                    "could not read {SURFACE_TOKEN} for the {theme:?} theme out of tokens.css; \
                     using the built-in value"
                );
                fallback
            })
        };
        (
            read(Resolved::Dark, FALLBACK_DARK),
            read(Resolved::Light, FALLBACK_LIGHT),
        )
    });
    match theme {
        Resolved::Dark => *dark,
        Resolved::Light => *light,
    }
}

/// Read `--ink-900` out of the block that states it for `theme`.
///
/// A declaration scanner, not a CSS parser: the sheet is ours, it is one
/// declaration per line, and the alternative — a stylesheet parser in the
/// application layer — would be a lot of code to read one colour.
fn surface_in(css: &str, theme: Resolved) -> Option<Color> {
    let selector = match theme {
        Resolved::Dark => ":root[data-theme=\"dark\"] {",
        Resolved::Light => ":root[data-theme=\"light\"] {",
    };
    let start = css.find(selector)? + selector.len();
    let block = &css[start..];
    let block = &block[..block.find("\n}")?];
    let declaration = format!("{SURFACE_TOKEN}:");
    let line = block
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with(&declaration))?;
    hex_color(line[declaration.len()..].trim().trim_end_matches(';'))
}

/// `#rgb` / `#rrggbb` → an opaque [`Color`].
///
/// The hex grammar is [`settings::normalise_accent`]'s, which is the one the
/// accent already crosses this boundary through — one parser, one set of
/// accepted forms. Opaque on purpose: a window surface is what everything else
/// is composited over, so an alpha on it has nothing to blend with.
pub(crate) fn hex_color(raw: &str) -> Option<Color> {
    let hex = settings::normalise_accent(raw)?;
    let byte = |at: usize| u8::from_str_radix(hex.get(at..at + 2)?, 16).ok();
    Some(Color(byte(1)?, byte(3)?, byte(5)?, 0xff))
}

/// For logs and tests: `Color` has no `Display`.
fn hex_of(color: Color) -> String {
    format!("#{:02x}{:02x}{:02x}", color.0, color.1, color.2)
}

/* ── resolving ───────────────────────────────────────────────────────────── */

/// The colour one window's surface must be — the whole decision, with no
/// window system in it so it can be tested.
fn pick(label: &str, theme: Resolved, reported: Option<(Resolved, Color)>) -> Color {
    if label == THEME_WINDOW {
        // The editor never wears the document (see the module docs).
        return designed(theme);
    }
    match reported {
        Some((tagged, color)) if tagged == theme => color,
        _ => designed(theme),
    }
}

/// `dark | light | system` → the theme actually in force.
///
/// `system` is asked of the window manager (`window.theme()`, which is
/// `NSApp.effectiveAppearance` on macOS and the registry on Windows) rather
/// than assumed: the designed identity is dark, so assuming dark would put an
/// obsidian rim around a light-appearance Mac running `system`.
fn resolve(app: &AppHandle, theme: settings::Theme) -> Resolved {
    match theme {
        settings::Theme::Dark => Resolved::Dark,
        settings::Theme::Light => Resolved::Light,
        settings::Theme::System => app
            .webview_windows()
            .values()
            .find_map(|window| window.theme().ok())
            .map(|native| match native {
                tauri::Theme::Light => Resolved::Light,
                // `tauri::Theme` is `#[non_exhaustive]`; anything that is not
                // light is dark as far as a surface colour goes.
                _ => Resolved::Dark,
            })
            // No window to ask (the first one is still being built): the
            // reported tag is the webview's own `prefers-color-scheme`, which
            // is the same question answered by the other side.
            .or_else(|| (*REPORTED.lock()).map(|(tagged, _)| tagged))
            .unwrap_or(Resolved::Dark),
    }
}

/// The persisted theme, or the default if the state is not up yet.
fn settings_theme(app: &AppHandle) -> settings::Theme {
    app.try_state::<std::sync::Arc<crate::state::AppState>>()
        .map(|state| state.settings.lock().appearance.theme)
        .unwrap_or_default()
}

/// The colour a window that is *about to be created* must be born with.
///
/// The builders take it at creation (`WebviewWindowBuilder::background_color`)
/// because that is the only way a new window's very first frame is the theme's
/// colour rather than the system's grey; [`apply`] then keeps it in step.
///
/// Only the EQ and editor windows can use this: the main window is the
/// config's, built before any Rust runs, and wears
/// `app.windows[0].backgroundColor` until [`apply`] reaches it in `setup` (see
/// the module docs and the test
/// `the_main_window_is_born_with_the_default_themes_surface`).
pub(crate) fn for_new_window(app: &AppHandle, label: &str) -> Color {
    let theme = resolve(app, settings_theme(app));
    pick(label, theme, *REPORTED.lock())
}

/// Push the surface colour at every window that exists.
///
/// Called from [`crate::apply_native_appearance`], so every path that already
/// retargets the native window *theme* — startup, `set_appearance`, the
/// appearance reset, a new window, an OS appearance change — moves the surface
/// with it.
pub(crate) fn apply(app: &AppHandle, theme: settings::Theme) {
    let theme = resolve(app, theme);
    let reported = *REPORTED.lock();
    for (label, window) in app.webview_windows() {
        let color = pick(&label, theme, reported);
        if let Err(e) = window.set_background_color(Some(color)) {
            // Cosmetic, and unsupported on mobile: never a failure.
            log::debug!("could not set the window surface on `{label}`: {e}");
        }
    }
}

/// Follow the *OS* appearance, for the `system` theme (SPEC §14).
///
/// The webview hears this too — `theme.ts` watches
/// `prefers-color-scheme` and reports the new surface — but the window manager
/// tells Rust first, and the frames between the two are exactly the ones with a
/// grey rim in them. The window theme needs no retargeting here: `system` is
/// `None`, so AppKit has already moved the frame itself.
pub(crate) fn watch_os_appearance(app: &AppHandle) {
    let Some(main) = app.get_webview_window(crate::state::MAIN_WINDOW) else {
        return;
    };
    let handle = app.clone();
    main.on_window_event(move |event| {
        if let tauri::WindowEvent::ThemeChanged(theme) = event {
            log::info!("the OS appearance changed to {theme:?}");
            apply(&handle, settings_theme(&handle));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP_CSS: &str = include_str!("../../src/styles/app.css");
    const EQ_CSS: &str = include_str!("../../src/styles/eq.css");
    const EDITOR_CSS: &str = include_str!("../../src/styles/theme-editor.css");
    const SURFACE_TS: &str = include_str!("../../src/lib/surface.ts");
    const MAIN_TSX: &str = include_str!("../../src/main.tsx");
    const EDITOR_TSX: &str = include_str!("../../src/theme/main.tsx");
    const EQ_TSX: &str = include_str!("../../src/eq/main.tsx");
    const BASE_CONF: &str = include_str!("../tauri.conf.json");
    const MACOS_CONF: &str = include_str!("../tauri.macos.conf.json");

    /* ── the theme link (the point of the module) ─────────────────────────── */

    /// The window surface **is** the resolved theme's background, and the only
    /// place that value exists is `tokens.css`. Read it here rather than
    /// restating it: a designer who moves obsidian moves the window with it,
    /// and if the declaration stops being readable this fails instead of
    /// silently falling back to a colour that is no longer the theme's.
    #[test]
    fn the_window_surface_is_the_resolved_themes_own_background() {
        let dark = surface_in(TOKENS_CSS, Resolved::Dark).expect("tokens.css must state the dark ");
        let light = surface_in(TOKENS_CSS, Resolved::Light).expect("…and the light one");
        assert_eq!(designed(Resolved::Dark), dark);
        assert_eq!(designed(Resolved::Light), light);
        // The fallbacks are documented as "what the sheet says"; if they are
        // not, one of the two is wrong.
        assert_eq!(dark, FALLBACK_DARK, "the dark surface drifted: {dark:?}");
        assert_eq!(
            light, FALLBACK_LIGHT,
            "the light surface drifted: {light:?}"
        );
        // And the two themes are genuinely different surfaces — a light theme
        // wearing the dark window colour is this bug wearing another colour.
        assert_ne!(dark, light);
        let sum = |c: Color| u32::from(c.0) + u32::from(c.1) + u32::from(c.2);
        assert!(sum(dark) < sum(light), "obsidian must be the darker one");
        assert_eq!(dark.3, 0xff, "a window surface is opaque");
        assert_eq!(hex_of(dark), "#0a0a0c");
        assert_eq!(hex_of(light), "#efebe1");
    }

    /// …and [`SURFACE_TOKEN`] really is the token the webviews paint their
    /// outermost pixels with. If a stylesheet is repointed at another token the
    /// window surface has to move with it, and this is what says so — the
    /// window would otherwise keep painting a colour no window wears any more.
    #[test]
    fn every_windows_base_surface_bottoms_out_in_the_token_this_module_reads() {
        let var = format!("var({SURFACE_TOKEN})");
        // `body` is the base of all three documents.
        assert!(
            TOKENS_CSS.contains(&format!("background: {var};")),
            "tokens.css must paint `body` with {SURFACE_TOKEN}"
        );
        // The main window and the EQ window layer gradients over it; both
        // gradient stacks end on the token, which is what makes the edge pixels
        // that colour (`shots/01-main-dark.png` measures rgb(10,10,12)).
        for (which, css) in [("--bg-app", TOKENS_CSS), ("--bg-eq", TOKENS_CSS)] {
            let occurrences = css.matches(&format!("{which}:")).count();
            assert_eq!(occurrences, 2, "{which} must be stated once per theme");
        }
        for block in TOKENS_CSS.split("--bg-app:").skip(1) {
            let value = block.split_once(';').expect("a declaration").0;
            assert!(
                value.trim_end().ends_with(&var),
                "--bg-app must end on {SURFACE_TOKEN}: {value}"
            );
        }
        for block in TOKENS_CSS.split("--bg-eq:").skip(1) {
            let value = block.split_once(';').expect("a declaration").0;
            assert!(
                value.trim_end().ends_with(&var),
                "--bg-eq must end on {SURFACE_TOKEN}: {value}"
            );
        }
        assert!(APP_CSS.contains("background: var(--bg-app);"));
        assert!(EQ_CSS.contains("background: var(--bg-eq);"));
        // The editor paints the token directly, with no gradient over it.
        assert!(EDITOR_CSS.contains(&format!("background: {var};")));
    }

    /// The other side of the seam. The front end reports what it paints, and it
    /// has to be reading the same token this module parses, in a window that is
    /// wearing the document — which is the main one, not the editor.
    #[test]
    fn the_front_end_reports_the_same_token_from_the_window_that_wears_the_theme() {
        let token = SURFACE_TOKEN.trim_start_matches('-');
        assert!(
            SURFACE_TS.contains(&format!("SURFACE_TOKEN = \"{token}\"")),
            "src/lib/surface.ts must name the same token as {SURFACE_TOKEN}"
        );
        // Reported on every applied appearance — a theme change, an OS
        // appearance change and an applied document all end in `onThemeChange`.
        assert!(
            MAIN_TSX.contains("onThemeChange") && MAIN_TSX.contains("setWindowSurface"),
            "src/main.tsx must report the surface on every theme change"
        );
        // And *only* from there. The editor never wears the document, so a
        // report from its webview would hand every window the colour of the
        // theme being edited — `pick`'s rule for THEME_WINDOW would be undone
        // from the other side of the boundary. The EQ window wears the document
        // but is not always open, so it has nothing to add that the main
        // window's report does not already carry.
        for (which, source) in [
            ("src/theme/main.tsx", EDITOR_TSX),
            ("src/eq/main.tsx", EQ_TSX),
        ] {
            assert!(
                !source.contains("setWindowSurface"),
                "{which} must not report a window surface; only the main window does"
            );
        }
    }

    /// The main window cannot be given a colour by this module: it is declared
    /// in `tauri.conf.json` and exists before `setup` runs, so the only
    /// creation-time surface it can have is the literal in the config. That
    /// literal is therefore a second statement of the dark theme's background,
    /// and this is what keeps the two from drifting — if obsidian moves in
    /// `tokens.css` and not in both configs, the window a user launches into is
    /// born the wrong colour for the length of one `setup`, which is exactly the
    /// flash this whole module exists to remove.
    ///
    /// Dark rather than resolved because a config is static and SPEC §14's
    /// default theme is `dark`; a `light` or `system`-in-light session is
    /// corrected by [`apply`] from `setup`.
    #[test]
    fn the_main_window_is_born_with_the_default_themes_surface() {
        assert_eq!(
            settings::Theme::default(),
            settings::Theme::Dark,
            "the config's static surface is the *default* theme's; if the default moved, \
             so must the literal in both configs"
        );
        for (which, conf) in [("tauri.conf.json", BASE_CONF), ("macOS", MACOS_CONF)] {
            let parsed: serde_json::Value = serde_json::from_str(conf).expect("valid JSON");
            let window = &parsed["app"]["windows"][0];
            let stated = window["backgroundColor"].as_str().unwrap_or_else(|| {
                panic!(
                    "{which} must state a window backgroundColor, or the main window is born grey"
                )
            });
            assert_eq!(
                hex_color(stated),
                Some(designed(Resolved::Dark)),
                "{which} states {stated} but the dark theme's surface is {}",
                hex_of(designed(Resolved::Dark))
            );
        }
    }

    /* ── the decision ────────────────────────────────────────────────────── */

    #[test]
    fn a_reported_colour_wins_for_the_theme_it_was_reported_for_and_no_other() {
        let pasted = Color(0x30, 0x10, 0x40, 0xff);
        let report = Some((Resolved::Dark, pasted));
        // The document is in force in the dark theme: wear it.
        assert_eq!(pick("main", Resolved::Dark, report), pasted);
        assert_eq!(pick("eq", Resolved::Dark, report), pasted);
        // A document states both themes, so a colour reported for dark says
        // nothing about light: fall back rather than paint the wrong one.
        assert_eq!(
            pick("main", Resolved::Light, report),
            designed(Resolved::Light)
        );
        // Nothing reported yet (the first frames of a launch).
        assert_eq!(pick("main", Resolved::Dark, None), designed(Resolved::Dark));
        assert_eq!(
            pick("main", Resolved::Light, None),
            designed(Resolved::Light)
        );
    }

    /// The editor is the window you undo a theme *from* (SPEC §20), so it wears
    /// the designed surface even while the document is in force everywhere
    /// else — the same rule its webview follows with `ignoreThemeDoc()`.
    #[test]
    fn the_theme_editor_window_never_wears_the_document() {
        let pasted = Color(0x30, 0x10, 0x40, 0xff);
        let report = Some((Resolved::Dark, pasted));
        assert_eq!(
            pick(THEME_WINDOW, Resolved::Dark, report),
            designed(Resolved::Dark)
        );
        assert_ne!(pick(THEME_WINDOW, Resolved::Dark, report), pasted);
    }

    /* ── the boundary ────────────────────────────────────────────────────── */

    #[test]
    fn only_a_hex_colour_crosses_the_ipc_boundary() {
        assert_eq!(hex_color("#0A0A0C"), Some(Color(0x0a, 0x0a, 0x0c, 0xff)));
        assert_eq!(hex_color("0a0a0c"), Some(Color(0x0a, 0x0a, 0x0c, 0xff)));
        assert_eq!(hex_color("#abc"), Some(Color(0xaa, 0xbb, 0xcc, 0xff)));
        for junk in [
            "",
            "obsidian",
            "rgb(10, 10, 12)",
            "#0a0a0",
            "#0a0a0cff",
            "var(--ink-900)",
            "#0a0a0c; } * { display: none }",
        ] {
            assert_eq!(hex_color(junk), None, "{junk} must not be a surface");
        }
        assert_eq!(Resolved::parse("dark"), Some(Resolved::Dark));
        assert_eq!(Resolved::parse(" light "), Some(Resolved::Light));
        // `system` is not a *resolved* theme: it has no colour of its own, and
        // accepting it here would mean guessing which one the OS is in.
        assert_eq!(Resolved::parse("system"), None);
        assert_eq!(Resolved::parse(""), None);
    }
}
