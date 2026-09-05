//! `settings.json` — everything that has to survive a relaunch except the
//! loudness cache (SPEC §12, plus the v1 preferences).
//!
//! One file, one writer. v1 had a second, hand-rolled `prefs.json` path with a
//! non-atomic `fs::write` and a swallowed error; keeping two persistence
//! mechanisms alive is exactly the duplication SPEC §9.2 asks to remove, so
//! the v1 preferences moved in here and both now go through the debounced
//! atomic writer of [`crate::persist`].
//!
//! The EQ *window* is here, though, and used not to be. While the EQ was an
//! in-app drawer its visibility was pure webview state and lived in
//! `localStorage`; now it is a real `WebviewWindow` that only Rust can create
//! (see [`crate::eqwindow`]), so "was it open?" is a fact about the app, not
//! about a document, and it is restored at launch from here — which is also
//! what SPEC §12 asked for in the first place. `eq_window_pinned` rides along:
//! always-on-top is a per-user preference about a tool window, and there is
//! exactly one place a preference is kept.
//!
//! Window *geometry* is deliberately not here: `tauri-plugin-window-state`
//! already remembers size and position for every label, including `eq`.
//!
//! # Schema evolution (v3)
//!
//! The file is versioned and **forward-only compatible by construction**: every
//! field is `#[serde(default)]`, so a v2 file written by an older build parses
//! into a v3 `Settings` with the new fields at their defaults and *every value
//! the user had still set*. [`Settings::migrate`] then runs the one-way fixups
//! and stamps the current [`SCHEMA`]. Losing a user's EQ curve because a build
//! added a theme setting would be unforgivable, so there is a test for exactly
//! that (`a_v2_file_keeps_every_setting_it_had`).
//!
//! Anything a hand-edited or downgraded file can put in the file is treated as
//! untrusted: colours, font names and numeric ranges are validated in
//! [`Settings::sanitised`] rather than trusted and passed to the webview, where
//! a font name is interpolated into CSS.

use std::path::{Path, PathBuf};

use onyx_core::{EqConfig, MonitorMode};
use serde::{Deserialize, Serialize};

use crate::persist::{read_optional, AtomicWriter, DEBOUNCE};

pub const FILE_NAME: &str = "settings.json";
/// Bump when a field changes meaning; older files are *migrated*, never wiped.
///
/// * 1 → 2: the v1 `prefs.json` was folded in here (EQ, monitor fold, A/B).
/// * 2 → 3: SPEC §14–§16/§18 — appearance (theme, accent, fonts, scale),
///   the audio host / fixed rate / buffer selection, and the user SoundFont.
/// * 3 → 4: SPEC §20 — the theme document (`themeDoc`), the pasted skin. A v3
///   file has none, which reads back as `None` = "the designed themes", and
///   that is exactly the right answer.
pub const SCHEMA: u32 = 4;

/// Which of the two designed themes to use (SPEC §14).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Theme {
    #[default]
    Dark,
    Light,
    /// Follow the OS, live.
    System,
}

/// Root UI scale (SPEC §15).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SizeScale {
    Compact,
    #[default]
    Normal,
    Large,
}

/// The champagne accent of the existing dark identity (SPEC §4).
pub const DEFAULT_ACCENT: &str = "#c9a227";
/// "Whatever the platform's UI font is" — the only font guaranteed offline.
pub const DEFAULT_UI_FONT: &str = "system";
/// The numeric read-outs must stay monospaced and column-stable (§15).
pub const DEFAULT_NUMERIC_FONT: &str = "system-mono";
/// Longest font token accepted. A family name is a word or three.
const MAX_FONT_LEN: usize = 48;
/// Longest theme document accepted, in bytes.
///
/// The generated default is about 20 kB; 256 kB is ten of them and still small
/// enough that reading it, holding two copies of it and shipping it to two
/// webviews on every snapshot costs nothing anyone can feel. The front end's
/// reader (`src/lib/jsonc.ts`) refuses the same size, so the boundary is the
/// same on both sides of the IPC seam.
pub const MAX_THEME_DOC_BYTES: usize = 256 * 1024;

/// Everything §14/§15 lets a user change about how Onyx looks.
///
/// The values are opaque *tokens* to this layer: the front end owns the
/// curated lists and maps a token to a real font stack. What this layer owns is
/// that whatever reaches the webview cannot be anything but a token — see
/// [`normalise_font`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Appearance {
    pub theme: Theme,
    /// `#rrggbb`, lower case. Hover/active/dim variants are derived from it in
    /// the front end so a custom accent cannot break hand-tuned states.
    pub accent: String,
    pub ui_font: String,
    pub numeric_font: String,
    pub size_scale: SizeScale,
}

impl Default for Appearance {
    fn default() -> Self {
        Appearance {
            theme: Theme::Dark,
            accent: DEFAULT_ACCENT.to_string(),
            ui_font: DEFAULT_UI_FONT.to_string(),
            numeric_font: DEFAULT_NUMERIC_FONT.to_string(),
            size_scale: SizeScale::Normal,
        }
    }
}

impl Appearance {
    fn sanitised(mut self) -> Appearance {
        let defaults = Appearance::default();
        self.accent = normalise_accent(&self.accent).unwrap_or_else(|| {
            log::warn!(
                "\"{}\" is not a colour; using the default accent {DEFAULT_ACCENT}",
                self.accent
            );
            defaults.accent.clone()
        });
        self.ui_font = normalise_font(&self.ui_font).unwrap_or_else(|| {
            log::warn!(
                "\"{}\" is not a usable font name; using the system UI font",
                self.ui_font
            );
            defaults.ui_font.clone()
        });
        self.numeric_font = normalise_font(&self.numeric_font).unwrap_or_else(|| {
            log::warn!(
                "\"{}\" is not a usable font name; using the system monospace font",
                self.numeric_font
            );
            defaults.numeric_font.clone()
        });
        self
    }
}

/// Parse `#rgb` / `#rrggbb` (with or without the `#`) into `#rrggbb`.
///
/// `None` for anything else — §15 requires unparseable input to be *rejected
/// visibly*, so the caller decides between "tell the user" (a command) and
/// "fall back and log" (a file that was edited by hand).
pub fn normalise_accent(raw: &str) -> Option<String> {
    let hex = raw.trim().trim_start_matches('#');
    if !hex.is_ascii() || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let expanded = match hex.len() {
        3 => hex.chars().flat_map(|c| [c, c]).collect::<String>(),
        6 => hex.to_string(),
        _ => return None,
    };
    Some(format!("#{}", expanded.to_ascii_lowercase()))
}

/// Validate a font token.
///
/// This value ends up in a CSS custom property in a webview. A settings file is
/// an ordinary file on disk that a user (or anything running as them) can edit,
/// so `"Inter; } * { display: none }"` is reachable input — CSS injection into
/// our own document, with no HTML involved. Only letters, digits, spaces and
/// `-_.` survive, which is every real family name and nothing that can close a
/// declaration.
pub fn normalise_font(raw: &str) -> Option<String> {
    let name = raw.trim();
    if name.is_empty() || name.len() > MAX_FONT_LEN {
        return None;
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.'))
    {
        return None;
    }
    Some(name.to_string())
}

/// Validate a theme document's *text* (SPEC §20).
///
/// This layer deliberately does **not** understand themes. The schema, the
/// token catalogue, the colour grammar and the contrast audit all live in the
/// front end (`src/lib/themedoc.ts`), in one implementation, because two
/// validators that must agree are a bug with a schedule. What Rust owns is that
/// the *file* cannot be a weapon: a bounded number of bytes, real text, and no
/// control characters — a NUL or an escape sequence in a settings file is
/// either corruption or an attempt to confuse a terminal that later prints it.
///
/// Anything the front end cannot parse is caught there, once, on load, and the
/// app falls back to the designed themes with a notice (`theme.ts`'s
/// `adoptDocText`). That fallback is proven by `check:theme`'s "garbage in the
/// saved theme" case and by `a_corrupt_theme_document_is_kept_but_bounded`.
///
/// The mock backend the browser preview and the screenshot harness run against
/// (`src/lib/mock.ts`) reimplements this function, and the two are held to one
/// checked-in fixture — see `theme_doc_contract.json` and the test at the foot
/// of this file. Mock/engine drift is what hid a shipped bug once already
/// (`scripts/check-ab-parity.mjs`), so this one is pinned from both sides.
pub fn normalise_theme_doc(raw: &str) -> Option<String> {
    let text = theme_doc_trim(raw);
    if text.is_empty() || text.len() > MAX_THEME_DOC_BYTES {
        return None;
    }
    // Tabs and newlines are how a document is laid out; nothing else below
    // 0x20 has any business in one.
    if text
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return None;
    }
    Some(text.to_string())
}

/// Surrounding whitespace, trimmed the one way both implementations agree on.
///
/// Rust's `str::trim` is the Unicode `White_Space` property and JavaScript's
/// `String.trim()` is not the same set: U+0085 is whitespace to Rust and not to
/// JS, U+FEFF is whitespace to JS and not to Rust. Either disagreement would
/// make one backend store a document the other clears. ASCII is the ground both
/// stand on, and everything outside it is left in the text where the control
/// check below can have an opinion about it.
fn theme_doc_trim(raw: &str) -> &str {
    raw.trim_matches(|c: char| c.is_ascii_whitespace())
}

/// Is this text "no document"? An empty box in the editor clears the theme
/// rather than failing to store nothing. Same trim as [`normalise_theme_doc`],
/// so a string cannot be blank to one and unstorable to the other.
pub fn theme_doc_is_blank(raw: &str) -> bool {
    theme_doc_trim(raw).is_empty()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// Schema of the file we were *read* from. A file with no `schema` key at
    /// all predates versioning, so it reads back as 0 rather than as "current"
    /// — [`Settings::migrate`] needs to know what it is looking at.
    #[serde(default = "unversioned")]
    pub schema: u32,
    // -- v1 preferences ----------------------------------------------------
    pub volume: f32,
    pub muted: bool,
    pub follow_source_rate: bool,
    pub ab_enabled: bool,
    pub crossfade_ms: f32,
    pub loop_enabled: bool,
    pub device: Option<String>,
    /// Per-deck decoded-audio ceiling in MiB (`decode::DEFAULT_DECK_BUDGET_BYTES`).
    pub deck_budget_mb: usize,
    // -- v2 (SPEC §6, §10, §12) --------------------------------------------
    pub eq: EqConfig,
    /// Was the detached EQ window open at exit? (SPEC §12: panel visibility
    /// persists.) Restored by [`crate::eqwindow::restore`].
    pub eq_window_open: bool,
    /// Always-on-top for the EQ window. On for a tool window by default, the
    /// way a plugin editor floats over its host.
    pub eq_window_pinned: bool,
    pub monitor_mode: MonitorMode,
    /// Level matching is opt-in and **off** by default (SPEC §10).
    pub level_match: bool,
    // -- v3 (SPEC §14, §15, §16, §18) -----------------------------------
    /// Theme, accent, fonts and size scale.
    pub appearance: Appearance,
    /// cpal host / audio API id (`coreaudio`, `wasapi`, `asio`, `alsa`), or
    /// `None` for the platform default (SPEC §16).
    pub host_id: Option<String>,
    /// Fixed output rate when `follow_source_rate` is off. `None` means "the
    /// engine's fallback", and it is always `None` while following the source.
    pub sample_rate: Option<u32>,
    /// Preferred buffer size in frames, or `None` to let the backend choose.
    pub buffer_frames: Option<u32>,
    /// A user-supplied General MIDI `.sf2` (SPEC §18). `None` = the bundled
    /// bank. A path that has stopped working falls back to the bundled bank
    /// with a message rather than being silently dropped from the file.
    pub soundfont: Option<String>,
    // -- v4 (SPEC §20) -----------------------------------------------------
    /// The theme document, verbatim, as the user pasted it — comments and all.
    ///
    /// Text, not a parsed structure, and that is the design: the front end owns
    /// the schema (§20), the document is round-tripped back into the editor for
    /// the *next* edit, and a build that adds a token must be able to read a
    /// document written by an older one. `None` = the two designed themes.
    pub theme_doc: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            schema: SCHEMA,
            volume: 1.0,
            muted: false,
            follow_source_rate: true,
            ab_enabled: false,
            crossfade_ms: 8.0,
            loop_enabled: false,
            device: None,
            deck_budget_mb: onyx_core::decode::DEFAULT_DECK_BUDGET_BYTES / (1024 * 1024),
            eq: EqConfig::default(),
            eq_window_open: false,
            eq_window_pinned: true,
            monitor_mode: MonitorMode::Stereo,
            level_match: false,
            appearance: Appearance::default(),
            host_id: None,
            sample_rate: None,
            buffer_frames: None,
            soundfont: None,
            theme_doc: None,
        }
    }
}

/// Rates we are willing to *ask* a device for. The engine still validates
/// against what the device reports; this only rejects nonsense from the file.
const KNOWN_RATES: &[u32] = &[
    44_100, 48_000, 88_200, 96_000, 176_400, 192_000, 352_800, 384_000,
];

/// Serde default for a file written before the schema key existed.
fn unversioned() -> u32 {
    0
}

impl Settings {
    /// One-way fixups for files written by older builds.
    ///
    /// Called before [`Settings::sanitised`]. Nothing here may *drop* a value:
    /// a v2 file has no `appearance`, no `hostId` and no `soundfont`, and serde
    /// has already given those their defaults, which is exactly right. The only
    /// thing to repair is state that used to be expressible two ways.
    fn migrate(&mut self) {
        let from = self.schema;
        if from == 0 {
            // A file with no `schema` at all is a v1 `prefs.json`-era file.
            log::debug!("settings file has no schema; treating it as v1");
        }
        if from < 3 {
            // v2 stored only `follow_source_rate`; there was no fixed-rate
            // field, so a v2 file that was *not* following the source has no
            // opinion about the rate. `None` = "use the engine fallback",
            // which is what v2 did.
            if self.follow_source_rate {
                self.sample_rate = None;
            }
        }
        if from != SCHEMA {
            log::info!("settings migrated from schema {from} to {SCHEMA}");
        }
        self.schema = SCHEMA;
    }

    /// Clamp everything a hand-edited (or corrupted-but-parseable) file could
    /// put out of range. The engine clamps too; doing it here means the value we
    /// report back to the UI is the value that is really in force.
    pub fn sanitised(mut self) -> Settings {
        self.migrate();
        self.volume = if self.volume.is_finite() {
            self.volume.clamp(0.0, 1.0)
        } else {
            1.0
        };
        self.crossfade_ms = if self.crossfade_ms.is_finite() {
            self.crossfade_ms.clamp(0.0, 200.0)
        } else {
            8.0
        };
        self.deck_budget_mb = self.deck_budget_mb.clamp(64, 8 * 1024);
        // 48 kHz is a lower bound only for the *stored* config; the engine
        // re-sanitises against the real device rate on every `set_eq`.
        self.eq = self.eq.sanitised(48_000.0);
        self.appearance = self.appearance.sanitised();
        self.host_id = self.host_id.and_then(|id| {
            let id = id.trim().to_ascii_lowercase();
            let ok = !id.is_empty()
                && id.len() <= 32
                && id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
            ok.then_some(id)
        });
        self.sample_rate = self.sample_rate.filter(|r| {
            let known = KNOWN_RATES.contains(r);
            if !known {
                log::warn!("ignoring the stored output rate {r} Hz: not a rate we can request");
            }
            known
        });
        self.buffer_frames = self.buffer_frames.filter(|f| {
            let sane = (16..=16_384).contains(f);
            if !sane {
                log::warn!("ignoring the stored buffer size of {f} frames");
            }
            sane
        });
        self.soundfont = self.soundfont.filter(|p| !p.trim().is_empty());
        // SPEC §20. A document that fails this is *not* a reason to reset the
        // rest of the file, and it is not a reason to keep a 4 MB string in
        // memory either: it is dropped, loudly, and the app comes up in the
        // designed themes.
        self.theme_doc = self.theme_doc.and_then(|text| {
            let ok = normalise_theme_doc(&text);
            if ok.is_none() {
                log::warn!(
                    "the saved theme document is not usable text ({} bytes); \
                     using the built-in appearance",
                    text.len()
                );
            }
            ok
        });
        self
    }

    pub fn budget_bytes(&self) -> usize {
        self.deck_budget_mb
            .saturating_mul(1024 * 1024)
            .max(64 * 1024 * 1024)
    }

    /// The user SoundFont as a path, if one is configured.
    ///
    /// Existence is *not* checked here: a bank on an unmounted volume must
    /// stay in the file (the user did choose it), and the decoder reports the
    /// fallback to the bundled bank at render time (SPEC §18).
    pub fn soundfont_path(&self) -> Option<PathBuf> {
        self.soundfont.as_ref().map(PathBuf::from)
    }
}

/// The settings file plus its debounced writer.
pub struct SettingsStore {
    writer: AtomicWriter,
}

impl SettingsStore {
    /// Read `dir/settings.json`. A corrupt file falls back to defaults with a
    /// warning: never a crash, and never a silent reset (SPEC §12).
    pub fn open(dir: Option<&Path>) -> (Settings, SettingsStore) {
        let path = dir.map(|d| d.join(FILE_NAME));
        let mut settings = Settings::default();
        if let Some(path) = path.as_deref() {
            match read_optional(path) {
                Ok(None) => {}
                Ok(Some(text)) => match serde_json::from_str::<Settings>(&text) {
                    Ok(parsed) => {
                        settings = parsed;
                        log::debug!("settings restored from {}", path.display());
                    }
                    Err(e) => {
                        log::warn!(
                            "{FILE_NAME} is corrupt ({e}); starting from the default settings. \
                             The old file is left in place so the EQ curve is not lost silently."
                        );
                        log::debug!("corrupt settings file at {}", path.display());
                    }
                },
                Err(e) => {
                    log::warn!(
                        "could not read {FILE_NAME} ({e}); starting from the default settings"
                    );
                    log::debug!("unreadable settings file at {}", path.display());
                }
            }
        }
        (
            settings.sanitised(),
            SettingsStore {
                writer: AtomicWriter::spawn(path, "settings", DEBOUNCE),
            },
        )
    }

    /// Queue a save. Never blocks, never writes on the calling thread.
    pub fn save(&self, settings: &Settings) {
        if self.writer.path().is_none() {
            return;
        }
        match serde_json::to_string_pretty(settings) {
            Ok(text) => self.writer.queue(text),
            Err(e) => log::error!(
                "could not serialise the settings ({e}); this session's changes will not be \
                 remembered"
            ),
        }
    }

    /// Write anything outstanding. Called once, at exit.
    pub fn flush(&self) {
        self.writer.flush_blocking();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use onyx_core::{EqBand, FilterKind};

    fn scratch(tag: &str) -> std::path::PathBuf {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("onyx-settings-{}-{tag}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn level_matching_is_off_by_default() {
        // SPEC §10: a mastering engineer must be able to trust that nothing
        // touches the gain unless they asked for it.
        assert!(!Settings::default().level_match);
        assert_eq!(Settings::default().monitor_mode, MonitorMode::Stereo);
        assert!(Settings::default().eq.bands.is_empty());
    }

    #[test]
    fn the_eq_window_starts_closed_and_pinned() {
        // SPEC §12: "the whole panel is hidden by default". Pinned, because a
        // tool window that disappears behind the thing it edits is a bug
        // report; the user can unpin it and that choice is remembered too.
        assert!(!Settings::default().eq_window_open);
        assert!(Settings::default().eq_window_pinned);
    }

    #[test]
    fn the_eq_window_state_survives_a_relaunch() {
        // The whole point of moving this out of localStorage: the window is
        // created by Rust at launch, so Rust has to know it was open.
        let dir = scratch("eqwindow");
        {
            let (_, store) = SettingsStore::open(Some(&dir));
            store.save(&Settings {
                eq_window_open: true,
                eq_window_pinned: false,
                ..Settings::default()
            });
            store.flush();
        }
        let (loaded, _) = SettingsStore::open(Some(&dir));
        assert!(loaded.eq_window_open);
        assert!(!loaded.eq_window_pinned);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_v2_state_survives_a_round_trip() {
        let dir = scratch("roundtrip");
        let settings = Settings {
            level_match: true,
            monitor_mode: MonitorMode::Side,
            eq: EqConfig {
                enabled: true,
                bands: vec![EqBand::new(7, FilterKind::HighShelf, 8_000.0, -3.5, 0.7)],
            },
            volume: 0.5,
            ..Settings::default()
        };
        {
            let (_, store) = SettingsStore::open(Some(&dir));
            store.save(&settings);
            store.flush();
        }
        let (loaded, _) = SettingsStore::open(Some(&dir));
        assert!(loaded.level_match);
        assert_eq!(loaded.monitor_mode, MonitorMode::Side);
        assert_eq!(loaded.eq.bands.len(), 1);
        assert_eq!(loaded.eq.bands[0].kind, FilterKind::HighShelf);
        assert_eq!(loaded.eq.bands[0].id, 7);
        assert_eq!(loaded.volume, 0.5);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_file_falls_back_to_defaults_without_panicking() {
        let dir = scratch("corrupt");
        std::fs::write(dir.join(FILE_NAME), b"{\"volume\": ").unwrap();
        let (loaded, _) = SettingsStore::open(Some(&dir));
        assert_eq!(loaded, Settings::default());
        // The broken file is left alone rather than overwritten on read.
        assert!(dir.join(FILE_NAME).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_partial_file_keeps_the_fields_it_has() {
        let dir = scratch("partial");
        std::fs::write(dir.join(FILE_NAME), b"{\"levelMatch\":true,\"muted\":true}").unwrap();
        let (loaded, _) = SettingsStore::open(Some(&dir));
        assert!(loaded.level_match);
        assert!(loaded.muted);
        assert_eq!(loaded.volume, 1.0, "missing fields take the default");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nonsense_values_are_clamped_rather_than_applied() {
        let dir = scratch("clamp");
        std::fs::write(
            dir.join(FILE_NAME),
            b"{\"volume\":9.5,\"crossfadeMs\":-40,\"deckBudgetMb\":1}",
        )
        .unwrap();
        let (loaded, _) = SettingsStore::open(Some(&dir));
        assert_eq!(loaded.volume, 1.0);
        assert_eq!(loaded.crossfade_ms, 0.0);
        assert_eq!(loaded.deck_budget_mb, 64);
        assert!(loaded.budget_bytes() >= 64 * 1024 * 1024);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_nan_volume_cannot_silence_the_app() {
        let s = Settings {
            volume: f32::NAN,
            crossfade_ms: f32::INFINITY,
            ..Settings::default()
        };
        let s = s.sanitised();
        assert_eq!(s.volume, 1.0);
        assert_eq!(s.crossfade_ms, 8.0);
    }

    #[test]
    fn settings_with_no_directory_are_a_no_op() {
        let (settings, store) = SettingsStore::open(None);
        assert_eq!(settings, Settings::default());
        store.save(&settings);
        store.flush();
    }

    // -- v3 -----------------------------------------------------------------

    /// The migration promise: adding v3 fields must not cost a user anything
    /// they had set in v2.
    #[test]
    fn a_v2_file_keeps_every_setting_it_had() {
        let dir = scratch("v2-migration");
        // Exactly what the previous build wrote: schema 2, no appearance, no
        // host, no soundfont, and a hand-tuned EQ curve.
        std::fs::write(
            dir.join(FILE_NAME),
            br#"{
              "schema": 2,
              "volume": 0.42,
              "muted": true,
              "followSourceRate": false,
              "abEnabled": true,
              "crossfadeMs": 24.0,
              "loopEnabled": true,
              "device": "Prism Lyra",
              "deckBudgetMb": 512,
              "eq": {"enabled": true, "bands": [
                {"id": 3, "kind": "bell", "freqHz": 1200.0, "gainDb": -2.5, "q": 1.4,
                 "enabled": true, "slopeDbOct": 12}
              ]},
              "eqWindowOpen": true,
              "eqWindowPinned": false,
              "monitorMode": "side",
              "levelMatch": true
            }"#,
        )
        .unwrap();
        let (s, _) = SettingsStore::open(Some(&dir));

        // Every v2 value survived, byte for byte.
        assert_eq!(s.volume, 0.42);
        assert!(s.muted);
        assert!(!s.follow_source_rate);
        assert!(s.ab_enabled);
        assert_eq!(s.crossfade_ms, 24.0);
        assert!(s.loop_enabled);
        assert_eq!(s.device.as_deref(), Some("Prism Lyra"));
        assert_eq!(s.deck_budget_mb, 512);
        assert!(s.eq.enabled);
        assert_eq!(s.eq.bands.len(), 1);
        assert_eq!(s.eq.bands[0].id, 3);
        assert_eq!(s.eq.bands[0].gain_db, -2.5);
        assert!(s.eq_window_open);
        assert!(!s.eq_window_pinned);
        assert_eq!(s.monitor_mode, MonitorMode::Side);
        assert!(s.level_match);

        // And the v3 fields arrived at their defaults, not at nothing.
        assert_eq!(s.schema, SCHEMA);
        assert_eq!(s.appearance, Appearance::default());
        assert_eq!(s.host_id, None);
        assert_eq!(s.sample_rate, None);
        assert_eq!(s.buffer_frames, None);
        assert_eq!(s.soundfont, None);
        assert_eq!(
            s.theme_doc, None,
            "no theme document is the designed themes"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // -- v4: the theme document (SPEC §20) ----------------------------------

    #[test]
    fn a_v3_file_gains_a_theme_document_slot_without_losing_anything() {
        // The migration promise again, one schema on: a user who set an accent
        // and a size scale in v3 must find them where they left them.
        let dir = scratch("v4-migration");
        // `br##…##`, not `br#…#`: the accent below contains `"#`, which would
        // otherwise close the raw string.
        std::fs::write(
            dir.join(FILE_NAME),
            br##"{
              "schema": 3,
              "volume": 0.6,
              "appearance": {"theme":"light","accent":"#b0652a","uiFont":"neutral",
                             "numericFont":"menlo","sizeScale":"large"},
              "eqWindowOpen": true
            }"##,
        )
        .unwrap();
        let (s, _) = SettingsStore::open(Some(&dir));
        assert_eq!(s.schema, SCHEMA);
        assert_eq!(s.volume, 0.6);
        assert_eq!(s.appearance.theme, Theme::Light);
        assert_eq!(s.appearance.accent, "#b0652a");
        assert_eq!(s.appearance.size_scale, SizeScale::Large);
        assert!(s.eq_window_open);
        assert_eq!(s.theme_doc, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_theme_document_survives_a_relaunch_verbatim() {
        // Comments and trailing commas included: the document is round-tripped
        // back into the editor, and a reformatted one would lose the user's
        // notes to themselves (SPEC §20).
        let dir = scratch("themedoc");
        let text = "// Cold Graphite\n{\n  \"onyx\": \"theme\",\n  \"dark\": { \"ink-900\": \"#0b0e11\", },\n}\n";
        {
            let (_, store) = SettingsStore::open(Some(&dir));
            store.save(&Settings {
                theme_doc: Some(text.to_string()),
                ..Settings::default()
            });
            store.flush();
        }
        let (loaded, _) = SettingsStore::open(Some(&dir));
        assert_eq!(loaded.theme_doc.as_deref(), Some(text.trim()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The escape hatch, from Rust's side: a settings file with garbage where
    /// the theme should be must still produce a usable app.
    #[test]
    fn a_corrupt_theme_document_is_kept_but_bounded() {
        // 1. Not JSON at all. Rust does not parse themes, so this *is* stored —
        //    and the front end drops it with a notice on load (proved in
        //    `scripts/check-theme.mjs`, "the escape hatch"). What matters here
        //    is that nothing else in the file is harmed.
        let dir = scratch("themedoc-garbage");
        std::fs::write(
            dir.join(FILE_NAME),
            br#"{"schema":4,"volume":0.3,"themeDoc":"}}} not a theme {{{"}"#,
        )
        .unwrap();
        let (s, _) = SettingsStore::open(Some(&dir));
        assert_eq!(s.volume, 0.3, "a bad theme must not cost the volume");
        assert_eq!(s.theme_doc.as_deref(), Some("}}} not a theme {{{"));

        // 2. A control character, a NUL and a megabyte are not documents.
        assert_eq!(normalise_theme_doc("{\"onyx\":\"theme\"}\u{0}"), None);
        assert_eq!(normalise_theme_doc("{\u{1b}[2J}"), None);
        assert_eq!(
            normalise_theme_doc(&"x".repeat(MAX_THEME_DOC_BYTES + 1)),
            None
        );
        assert_eq!(normalise_theme_doc("   "), None);
        assert_eq!(normalise_theme_doc("\t"), None);
        // Real documents survive, whitespace-trimmed and otherwise untouched.
        assert_eq!(
            normalise_theme_doc("  {\n\t\"onyx\": \"theme\"\n}  ").as_deref(),
            Some("{\n\t\"onyx\": \"theme\"\n}")
        );

        // 3. …and one that reaches `sanitised` with a NUL in it is dropped
        //    rather than handed to a webview.
        let s = Settings {
            theme_doc: Some("{\"onyx\":\"theme\"}\u{0}".into()),
            volume: 0.9,
            ..Settings::default()
        }
        .sanitised();
        assert_eq!(s.theme_doc, None);
        assert_eq!(s.volume, 0.9);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unversioned_file_is_treated_as_v1_and_still_migrates() {
        let dir = scratch("v1-migration");
        std::fs::write(
            dir.join(FILE_NAME),
            br#"{"volume":0.25,"loopEnabled":true}"#,
        )
        .unwrap();
        let (s, _) = SettingsStore::open(Some(&dir));
        assert_eq!(s.volume, 0.25);
        assert!(s.loop_enabled);
        assert_eq!(s.schema, SCHEMA);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_v3_state_survives_a_round_trip() {
        let dir = scratch("v3-roundtrip");
        let settings = Settings {
            appearance: Appearance {
                theme: Theme::System,
                accent: "#3ba7ff".into(),
                ui_font: "Inter".into(),
                numeric_font: "JetBrains Mono".into(),
                size_scale: SizeScale::Large,
            },
            host_id: Some("coreaudio".into()),
            follow_source_rate: false,
            sample_rate: Some(96_000),
            buffer_frames: Some(256),
            soundfont: Some("/banks/Arachno.sf2".into()),
            ..Settings::default()
        };
        {
            let (_, store) = SettingsStore::open(Some(&dir));
            store.save(&settings);
            store.flush();
        }
        let (loaded, _) = SettingsStore::open(Some(&dir));
        assert_eq!(loaded, settings.sanitised());
        assert_eq!(loaded.appearance.theme, Theme::System);
        assert_eq!(loaded.appearance.accent, "#3ba7ff");
        assert_eq!(loaded.sample_rate, Some(96_000));
        assert_eq!(loaded.buffer_frames, Some(256));
        assert_eq!(
            loaded.soundfont_path(),
            Some(std::path::PathBuf::from("/banks/Arachno.sf2"))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_settings_file_is_camel_case_like_the_rest_of_the_contract() {
        let json = serde_json::to_string(&Settings::default()).unwrap();
        for key in [
            "followSourceRate",
            "hostId",
            "sampleRate",
            "bufferFrames",
            "sizeScale",
            "uiFont",
            "numericFont",
            "themeDoc",
        ] {
            assert!(json.contains(key), "{key} missing from {json}");
        }
        assert!(!json.contains("host_id"));
    }

    #[test]
    fn a_font_name_cannot_smuggle_css_into_the_webview() {
        // The settings file is an ordinary file on disk. A font token is
        // interpolated into a CSS custom property, so anything that could
        // close a declaration has to die at the boundary.
        assert_eq!(normalise_font("Inter").as_deref(), Some("Inter"));
        assert_eq!(
            normalise_font("  IBM Plex Mono "),
            Some("IBM Plex Mono".into())
        );
        assert_eq!(normalise_font("Inter; } * { display: none }"), None);
        assert_eq!(normalise_font("url(http://x/y)"), None);
        assert_eq!(normalise_font("</style><script>"), None);
        assert_eq!(normalise_font(""), None);
        assert_eq!(normalise_font(&"A".repeat(MAX_FONT_LEN + 1)), None);

        let s = Settings {
            appearance: Appearance {
                ui_font: "Inter; } html { display:none }".into(),
                ..Appearance::default()
            },
            ..Settings::default()
        }
        .sanitised();
        assert_eq!(s.appearance.ui_font, DEFAULT_UI_FONT);
    }

    #[test]
    fn an_accent_is_normalised_or_rejected() {
        assert_eq!(normalise_accent("#C9A227").as_deref(), Some("#c9a227"));
        assert_eq!(normalise_accent("c9a227").as_deref(), Some("#c9a227"));
        assert_eq!(normalise_accent("#f0a").as_deref(), Some("#ff00aa"));
        assert_eq!(normalise_accent("red"), None);
        assert_eq!(normalise_accent("var(--x)"), None);
        assert_eq!(normalise_accent("#12345"), None);
        assert_eq!(normalise_accent(""), None);

        let s = Settings {
            appearance: Appearance {
                accent: "rgb(255,0,0)".into(),
                ..Appearance::default()
            },
            ..Settings::default()
        }
        .sanitised();
        assert_eq!(s.appearance.accent, DEFAULT_ACCENT);
    }

    #[test]
    fn an_impossible_engine_selection_is_dropped_not_obeyed() {
        let s = Settings {
            host_id: Some("  CoreAudio  ".into()),
            sample_rate: Some(1),
            buffer_frames: Some(1_000_000),
            soundfont: Some("   ".into()),
            ..Settings::default()
        }
        .sanitised();
        assert_eq!(s.host_id.as_deref(), Some("coreaudio"));
        assert_eq!(s.sample_rate, None, "1 Hz is not a rate");
        assert_eq!(s.buffer_frames, None, "a million frames is not a buffer");
        assert_eq!(s.soundfont, None, "a blank path is no path");
    }

    /* ── mock ↔ engine parity (SPEC §20) ─────────────────────────────── */

    /// Build one case's input the way `scripts/check-theme.mjs` builds it.
    fn fixture_input(case: &serde_json::Value) -> Option<String> {
        if let Some(build) = case.get("build") {
            let prefix = build["prefix"].as_str().unwrap_or("");
            let unit = build["unit"].as_str().expect("build.unit");
            let times = build["times"].as_u64().expect("build.times") as usize;
            return Some(format!("{prefix}{}", unit.repeat(times)));
        }
        match &case["text"] {
            serde_json::Value::Null => None,
            serde_json::Value::String(s) => Some(s.clone()),
            other => panic!("`text` must be a string or null, got {other}"),
        }
    }

    /// The storage contract, from the engine's side.
    ///
    /// The same fixture drives `src/lib/mock.ts` in `npm run check:theme`. Two
    /// backends that disagree about which documents exist is how a preview
    /// "proves" a theme that the real app refuses — the failure mode
    /// `check-ab-parity.mjs` was written for, one subsystem later.
    #[test]
    fn the_theme_doc_storage_contract_holds() {
        const FIXTURE: &str = include_str!("../tests/fixtures/theme_doc_contract.json");
        let fixture: serde_json::Value =
            serde_json::from_str(FIXTURE).expect("theme_doc_contract.json must be valid JSON");

        assert_eq!(
            fixture["maxBytes"].as_u64(),
            Some(MAX_THEME_DOC_BYTES as u64),
            "the fixture and settings.rs disagree about the size limit"
        );

        let cases = fixture["cases"].as_array().expect("a case list");
        assert!(cases.len() >= 12, "the contract lost cases");
        for case in cases {
            let name = case["name"].as_str().expect("a name");
            let input = fixture_input(case);

            // What the command does: `null` and blank clear, everything else
            // must survive `normalise_theme_doc` or be refused out loud.
            let stored: Result<Option<String>, ()> = match input.as_deref() {
                None => Ok(None),
                Some(raw) if theme_doc_is_blank(raw) => Ok(None),
                Some(raw) => normalise_theme_doc(raw).map(Some).ok_or(()),
            };

            if case.get("rejected").and_then(|v| v.as_bool()) == Some(true) {
                assert!(stored.is_err(), "{name}: should have been refused");
                continue;
            }
            let want = match case["stored"].as_str() {
                Some("=input") => input.clone(),
                Some(text) => Some(text.to_string()),
                None => None,
            };
            assert_eq!(stored, Ok(want), "{name}");
        }
    }
}
