//! Application state: the single source of truth the front end mirrors.
//!
//! Locking rules (deliberately strict, because the audio callback must never
//! wait on us and a deadlock in a player is unforgivable):
//!
//! * every field that needs interior mutability sits behind its own
//!   `parking_lot::Mutex`,
//! * **no code path ever holds two of those locks at the same time**, so no
//!   lock-order cycle can exist. Where a value from one lock feeds a decision
//!   about another, the first guard is dropped before the second is taken —
//!   look for the `let (…) = { … };` blocks, they are that, not style,
//! * a lock is never held across a call into [`AudioEngine`] that can block
//!   (`request_rate` / `set_device` re-open the output device),
//! * a lock is never held across a Tauri `emit`: [`AppState::emit_state`]
//!   builds the whole snapshot first and only then emits it. `emit` runs
//!   listeners and serialises on the calling thread, so holding the playlist
//!   mutex across it would put an unbounded amount of work inside the critical
//!   section that the audio-adjacent load path also waits on.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use onyx_core::deck::{DeckSlot, DeckState};
use onyx_core::decode::{DecodeHandle, DecodeOptions};
use onyx_core::engine::AudioEngine;
use onyx_core::midi::MidiOptions;
use onyx_core::{Deck, EngineSource, EqConfig, LoudnessAnalysis, TransportState, LUFS_SILENCE};
use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::archive::ExtractedArchive;
use crate::blind::{BlindSnapshot, BlindTest};
use crate::cache::LoudnessCache;
use crate::playlist::{openable_extensions, Playlist, PlaylistEntry, ProbePool};
use crate::settings::{Appearance, Settings, SettingsStore};

/// Event names (SPEC §3.2).
pub const FRAME_EVENT: &str = "onyx://frame";
pub const STATE_EVENT: &str = "onyx://state";
pub const TOAST_EVENT: &str = "onyx://toast";
/// Label of the one and only window.
pub const MAIN_WINDOW: &str = "main";

/// Widest level-match attenuation, in dB.
///
/// This is not a safety limit — the trims only ever attenuate (SPEC §10), so
/// no amount of it can clip. It only stops a nonsense measurement from muting a
/// deck outright. 24 dB comfortably covers the real case it has to survive: a
/// quiet 1990s master at −20 LUFS against a modern one at −6.
pub const MAX_MATCH_TRIM_DB: f32 = 24.0;

/// Level-match trim applied to each deck, in dB. Always `<= 0`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct TrimPair {
    pub a: f32,
    pub b: f32,
}

impl TrimPair {
    pub fn get(&self, deck: Deck) -> f32 {
        match deck {
            Deck::A => self.a,
            Deck::B => self.b,
        }
    }
}

/// Loudness matching state (SPEC §10).
///
/// Opt-in, **off by default**, and attenuation-only. `ready == false` means
/// "you asked for matching but we do not have both measurements yet", and while
/// that is the case both trims are exactly unity — the engine early-outs, so
/// the signal path stays bit-transparent instead of pretending to be matched.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LevelMatch {
    pub enabled: bool,
    pub ready: bool,
    pub trim: TrimPair,
}

impl LevelMatch {
    /// Work out the trims from the two integrated-loudness readings.
    ///
    /// `a` / `b` are `None` when that deck is empty or has not been measured
    /// yet (neither the decode nor the loudness cache of §8 could say). Pure on
    /// purpose: this is the rule the whole feature rests on, and it is the one
    /// part of the A/B path that can be tested without an audio device.
    pub fn compute(enabled: bool, a: Option<f32>, b: Option<f32>) -> LevelMatch {
        if !enabled {
            // Not "0.0 dB converted to a gain of 1.0": unity, and the engine
            // skips the gain stage entirely.
            return LevelMatch::default();
        }
        let (Some(a), Some(b)) = (a, b) else {
            // Honest "pending" rather than a silent claim of a match.
            return LevelMatch {
                enabled: true,
                ready: false,
                trim: TrimPair::default(),
            };
        };
        if !a.is_finite() || !b.is_finite() || a <= LUFS_SILENCE || b <= LUFS_SILENCE {
            // A silent (or unmeasurable) deck cannot be matched to anything and
            // 24 dB of gain on silence is still silence. The measurement *did*
            // land, so this is `ready`, at unity.
            return LevelMatch {
                enabled: true,
                ready: true,
                trim: TrimPair::default(),
            };
        }
        // Attenuate only: bring the louder deck down to the quieter one. Never
        // boost — boosting risks true-peak overs on material that is already
        // hot, which is exactly the material this tool is used on.
        let target = a.min(b);
        let trim = TrimPair {
            a: (target - a).clamp(-MAX_MATCH_TRIM_DB, 0.0),
            b: (target - b).clamp(-MAX_MATCH_TRIM_DB, 0.0),
        };
        LevelMatch {
            enabled: true,
            ready: true,
            trim,
        }
    }

    fn snapshot(&self) -> LevelMatchState {
        LevelMatchState {
            enabled: self.enabled,
            ready: self.ready,
            trim_db_a: self.trim.a,
            trim_db_b: self.trim.b,
        }
    }
}

/// Host-side A/B configuration. The engine owns the audible truth; this is the
/// part the UI has to read back.
#[derive(Clone, Copy, Debug, Default)]
pub struct AbConfig {
    pub enabled: bool,
    pub level_match: LevelMatch,
    pub crossfade_ms: f32,
}

/// `LevelMatchState` in `src/lib/types.ts`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LevelMatchState {
    pub enabled: bool,
    pub ready: bool,
    pub trim_db_a: f32,
    pub trim_db_b: f32,
}

/// `AbState` in `src/lib/types.ts`.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AbSnapshot {
    pub enabled: bool,
    pub level_match: LevelMatchState,
    pub crossfade_ms: f32,
    /// Deck B relative to deck A, at the engine sample rate (SPEC §11).
    pub ab_offset_frames: i64,
}

/// The output side of the world, as the Settings panel reads it back
/// (SPEC §16).
///
/// `current` / `followSourceRate` / `engineSampleRate` are the v2 fields and
/// keep their meaning. The rest describe what the *stream* is, which is not
/// always what was asked for: a device that would not take 96 kHz, or a
/// backend that chose its own buffer size, shows up here and nowhere else.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSnapshot {
    pub current: Option<String>,
    pub follow_source_rate: bool,
    pub engine_sample_rate: u32,
    /// Host the stream is running on, matching `HostInfo.id`.
    pub host_id: Option<String>,
    /// True when the device is whatever the OS calls default, so the UI can
    /// say "System default (Studio Monitors)".
    pub following_system_default: bool,
    /// Granted buffer size in frames, `null` when the backend chose its own.
    pub buffer_frames: Option<u32>,
    /// Latency of that buffer at the running rate, in ms — the number the
    /// user actually cares about.
    pub latency_ms: Option<f32>,
}

/// `DeckState` plus the polarity flag of SPEC §11.
///
/// `invert` lives on the engine, not in `onyx_core::deck::DeckState`, so it is
/// flattened in here instead of being duplicated into the core type. The
/// front end reads it as `DeckState.invert`.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeckSnapshot {
    #[serde(flatten)]
    pub state: DeckState,
    pub invert: bool,
}

/// Everything the UI needs in one message (SPEC §3.3, §6–§12).
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSnapshot {
    pub playlist: Vec<PlaylistEntry>,
    pub deck_a: DeckSnapshot,
    pub deck_b: DeckSnapshot,
    pub transport: TransportState,
    pub ab: AbSnapshot,
    pub blind: BlindSnapshot,
    pub eq: EqConfig,
    pub device: DeviceSnapshot,
    /// Every extension a drop or the file dialog may accept — audio plus
    /// `.zip` (SPEC §19).
    pub supported_extensions: Vec<String>,
    /// Theme, accent, fonts and size scale (SPEC §14/§15).
    pub appearance: Appearance,
    /// The pasted theme document, verbatim, or `null` for the designed themes
    /// (SPEC §20). It rides on the snapshot for the same reason `appearance`
    /// does: it is broadcast to every webview, so a theme applied in the editor
    /// window re-skins the main and EQ windows without any of them knowing the
    /// others exist.
    pub theme_doc: Option<String>,
    /// The user's `.sf2`, or `null` for the bundled GM bank (SPEC §18).
    pub soundfont: Option<String>,
}

/// Per-deck view of the decode in flight, pushed at 60 Hz.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameDeck {
    pub decoded_fraction: f32,
    pub waveform_buckets: usize,
    pub analysis_ready: bool,
}

/// The band-solo audition bandpass, if one is engaged (SPEC §12).
///
/// It rides on the frame stream rather than on the snapshot because it is
/// dragged at pointer rate — and because it now has to cross a *window*
/// boundary: the sweep happens in the EQ window, and the main window's
/// "band solo" badge has to light up for it. A module-level ref in one
/// webview cannot be seen from the other, so the engine's own state is the
/// only thing both windows can agree on.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditionFrame {
    pub freq_hz: f32,
    pub q: f32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FramePayload {
    pub transport: TransportState,
    pub meters: onyx_core::MeterSnapshot,
    pub deck_a: FrameDeck,
    pub deck_b: FrameDeck,
    pub audition: Option<AuditionFrame>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ToastKind {
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToastPayload {
    pub kind: ToastKind,
    pub message: String,
}

pub struct AppState {
    pub editor_previews: Mutex<std::collections::HashMap<std::path::PathBuf, std::sync::Arc<crate::editor::Preview>>>,
    pub engine: Arc<AudioEngine>,
    pub playlist: Mutex<Playlist>,
    /// Index 0 = deck A, 1 = deck B (see [`Deck::index`]).
    pub decks: Mutex<[DeckSlot; 2]>,
    pub ab: Mutex<AbConfig>,
    pub blind: Mutex<BlindTest>,
    /// Persisted settings (SPEC §12) — the in-memory copy of `settings.json`.
    pub settings: Mutex<Settings>,
    /// Loudness cache (SPEC §8).
    pub cache: LoudnessCache,
    /// Background metadata probing.
    pub probes: OnceLock<ProbePool>,
    /// Temp directories holding the contents of opened `.zip` archives
    /// (SPEC §19). Dropping one deletes its tree, so the lifetime rule is
    /// simply "these live as long as the rows that came out of them": the
    /// open path adopts them, `playlist_clear` and a replacing open drop
    /// them, and exit drops them.
    archives: Mutex<Vec<ExtractedArchive>>,
    store: SettingsStore,
    app: OnceLock<AppHandle>,
    /// Something changed that the UI has to see; the frame thread pushes an
    /// `onyx://state` at up to 10 Hz.
    state_dirty: AtomicBool,
    settings_dirty: AtomicBool,
    /// Toasts raised before the webview could listen for them. See
    /// [`AppState::warn_at_startup`].
    startup_notices: Mutex<Vec<String>>,
    /// Set once the queue has been handed to the UI, after which a late notice
    /// has to be toasted directly or it would never be seen.
    startup_flushed: AtomicBool,
}

impl AppState {
    pub fn new(
        engine: Arc<AudioEngine>,
        settings: Settings,
        store: SettingsStore,
        cache: LoudnessCache,
    ) -> AppState {
        let ab = AbConfig {
            enabled: settings.ab_enabled,
            level_match: LevelMatch {
                enabled: settings.level_match,
                // Nothing is loaded yet, so nothing is measured yet.
                ready: false,
                trim: TrimPair::default(),
            },
            crossfade_ms: settings.crossfade_ms,
        };
        AppState {
            editor_previews: Mutex::new(std::collections::HashMap::new()),
            engine,
            playlist: Mutex::new(Playlist::default()),
            decks: Mutex::new([DeckSlot::default(), DeckSlot::default()]),
            ab: Mutex::new(ab),
            blind: Mutex::new(BlindTest::new()),
            settings: Mutex::new(settings),
            cache,
            probes: OnceLock::new(),
            archives: Mutex::new(Vec::new()),
            store,
            app: OnceLock::new(),
            state_dirty: AtomicBool::new(false),
            settings_dirty: AtomicBool::new(false),
            startup_notices: Mutex::new(Vec::new()),
            startup_flushed: AtomicBool::new(false),
        }
    }

    /// Push the persisted settings into the engine. Called once at startup.
    pub fn apply_settings(&self) {
        let settings = self.settings.lock().clone();
        self.engine.set_volume(settings.volume);
        self.engine.set_muted(settings.muted);
        self.engine
            .set_follow_source_rate(settings.follow_source_rate);
        self.engine.set_loop_enabled(settings.loop_enabled);
        self.engine.set_ab_enabled(settings.ab_enabled);
        self.engine.set_crossfade_ms(settings.crossfade_ms);
        // SPEC §6 / §12: the monitor fold and the EQ curve are remembered.
        self.engine.set_monitor_mode(settings.monitor_mode);
        let applied = self.engine.set_eq(settings.eq.clone());
        // The engine clamps against the real device rate, so store what is
        // actually running rather than what the file asked for.
        self.settings.lock().eq = applied;
    }

    pub fn set_app(&self, app: AppHandle) {
        let _ = self.app.set(app);
    }

    /// The handle, once `setup` has run. `None` in unit tests and during the
    /// window-less part of startup.
    pub fn app(&self) -> Option<AppHandle> {
        self.app.get().cloned()
    }

    pub fn budget_bytes(&self) -> usize {
        self.settings.lock().budget_bytes()
    }

    /// Decode options built from the settings: today, the user's SoundFont
    /// (SPEC §18). Read afresh every time rather than cached, so picking a
    /// new bank applies to the next track without any invalidation logic.
    pub fn decode_options(&self) -> DecodeOptions {
        DecodeOptions {
            midi: MidiOptions {
                soundfont: self.settings.lock().soundfont_path(),
            },
        }
    }

    /// The rate a decode of a `source_rate` Hz file would be stored at, which
    /// is what the loudness cache is keyed on (SPEC §8).
    ///
    /// With "follow source sample rate" on (SPEC §9.6) the device is re-opened
    /// at the source rate, so that is where the decode lands; with it off the
    /// file is resampled to the rate the engine already runs at. This is a
    /// *prediction* — the device can refuse, and `load` then decodes at
    /// whatever it got — so it is only ever used for `lookup`. `store` uses the
    /// rate the decode really ran at (`DecodeHandle::stored_rate`), so a wrong
    /// prediction costs a re-measurement and can never produce a false hit.
    pub fn expected_decode_rate(&self, source_rate: u32) -> u32 {
        if source_rate != 0 && self.engine.follow_source_rate() {
            source_rate
        } else {
            self.engine.engine_rate()
        }
    }

    // -- archives (SPEC §19) ----------------------------------------------

    /// Keep these temp directories alive until the playlist is cleared.
    pub fn adopt_archives(&self, archives: Vec<ExtractedArchive>) {
        if archives.is_empty() {
            return;
        }
        self.archives.lock().extend(archives);
    }

    /// Delete every extracted archive. Called when the playlist is emptied
    /// (clear, or a replacing open) and at exit. Returns how many went, which
    /// is what the tests assert on.
    ///
    /// The `Vec` is moved out *before* the directories are removed, so the
    /// `remove_dir_all` calls — real IO — happen with no lock held.
    pub fn clear_archives(&self) -> usize {
        let taken = std::mem::take(&mut *self.archives.lock());
        taken.len()
    }

    // -- events -------------------------------------------------------------

    /// Ask for an `onyx://state` push on the next frame tick (max ~10 Hz).
    pub fn mark_state_dirty(&self) {
        self.state_dirty.store(true, Ordering::Release);
    }

    pub fn take_state_dirty(&self) -> bool {
        self.state_dirty.swap(false, Ordering::AcqRel)
    }

    pub fn mark_settings_dirty(&self) {
        self.settings_dirty.store(true, Ordering::Release);
    }

    /// Build and emit a snapshot right now. No lock is held across the `emit`.
    pub fn emit_state(&self) {
        self.state_dirty.store(false, Ordering::Release);
        if let Some(app) = self.app.get() {
            let snap = self.snapshot();
            if let Err(e) = app.emit(STATE_EVENT, snap) {
                // Same story as the toast path: the webview is tearing down.
                log::debug!("could not emit {STATE_EVENT} ({e}); the webview is going away");
            }
        }
    }

    /// Show a toast *and* record it in the log.
    ///
    /// Every toast is a message we chose to put in front of the user, so it is
    /// exactly what a bug report needs. The log level follows the toast kind
    /// rather than being fixed at `info`: an error toast means the user's action
    /// failed, and a log reader filtering at `warn` must still see it. Call
    /// sites therefore do not need a second `log::` line of their own.
    pub fn toast(&self, kind: ToastKind, message: impl Into<String>) {
        let payload = ToastPayload {
            kind,
            message: message.into(),
        };
        match payload.kind {
            ToastKind::Error => log::error!("toast: {}", payload.message),
            ToastKind::Warn => log::warn!("toast: {}", payload.message),
            ToastKind::Info => log::info!("toast: {}", payload.message),
        }
        self.emit_toast(payload);
    }

    fn emit_toast(&self, payload: ToastPayload) {
        if let Some(app) = self.app.get() {
            if let Err(e) = app.emit(TOAST_EVENT, payload) {
                // The only way this fails is a webview that is going away, so
                // the log line is all that is left to do.
                log::debug!("could not emit {TOAST_EVENT} ({e}); the webview is going away");
            }
        }
    }

    /// Record a warning raised during `setup()`, before the webview exists.
    ///
    /// Toasting straight from startup logs the line but the user never sees it:
    /// `frame.ts` only installs its `onyx://toast` listener once React mounts,
    /// and an event emitted before that is dropped, not queued. So the startup
    /// complaints ("the remembered output device is gone", "no writable config
    /// dir") were log-only in practice. Buffer them here instead and let the
    /// first `app_state` call — which the frontend makes *after* subscribing —
    /// flush them.
    pub fn warn_at_startup(&self, message: impl Into<String>) {
        let message = message.into();
        log::warn!("{message}");
        let mut queue = self.startup_notices.lock();
        if self.startup_flushed.load(Ordering::Acquire) {
            // The webview is already listening and the queue has been drained:
            // a checker that finished late (see `soundfont_complaint`) says it
            // now rather than into the void.
            drop(queue);
            self.emit_toast(ToastPayload {
                kind: ToastKind::Warn,
                message,
            });
        } else {
            queue.push(message);
        }
    }

    /// Emit anything [`AppState::warn_at_startup`] buffered. Idempotent: the
    /// queue is drained, so a reload does not repeat old news.
    pub fn flush_startup_notices(&self) {
        let pending = {
            let mut queue = self.startup_notices.lock();
            self.startup_flushed.store(true, Ordering::Release);
            std::mem::take(&mut *queue)
        };
        for message in pending {
            // Already logged by `warn_at_startup`; this is the UI half.
            self.emit_toast(ToastPayload {
                kind: ToastKind::Warn,
                message,
            });
        }
    }

    pub fn info(&self, message: impl Into<String>) {
        self.toast(ToastKind::Info, message);
    }

    pub fn warn(&self, message: impl Into<String>) {
        self.toast(ToastKind::Warn, message);
    }

    pub fn error(&self, message: impl Into<String>) {
        self.toast(ToastKind::Error, message);
    }

    // -- persistence --------------------------------------------------------

    /// Hand the settings and the loudness cache to their debounced writers if
    /// anything changed. Called from the frame thread every couple of seconds,
    /// so dragging a fader does not hammer the disk — and never from a command,
    /// the audio callback or a decode thread.
    pub fn persist_tick(&self) {
        if self.settings_dirty.swap(false, Ordering::AcqRel) {
            let settings = self.settings.lock().clone();
            self.store.save(&settings);
        }
        self.cache.tick();
    }

    /// Write everything out and wait for it. Called once, at exit.
    pub fn persist_flush(&self) {
        self.remember_settings();
        self.settings_dirty.store(false, Ordering::Release);
        let settings = self.settings.lock().clone();
        self.store.save(&settings);
        self.store.flush();
        self.cache.flush();
    }

    /// Mirror the live engine/AB state into the settings.
    ///
    /// Only flags the file as dirty when a value actually moved: this runs on
    /// the frame thread every couple of seconds, and an unconditional flag would
    /// rewrite `settings.json` for ever even with the app sitting idle.
    pub fn remember_settings(&self) {
        // Everything is read from the engine and the A/B config *before* the
        // settings lock is taken: two locks are never held at once.
        let volume = self.engine.volume();
        let muted = self.engine.muted();
        let follow_source_rate = self.engine.follow_source_rate();
        let loop_enabled = self.engine.loop_enabled();
        let device = self.engine.current_device();
        let monitor_mode = self.engine.monitor_mode();
        let eq = self.engine.eq_config();
        // SPEC §16: the whole engine source is remembered, not just the
        // device. `config()` is what was *asked* for, which is the right thing
        // to persist — a device that refused 96 kHz today may grant it
        // tomorrow, and re-asking is free.
        let config = self.engine.config();
        let source = self.engine.current_source();
        let ab = *self.ab.lock();
        let changed = {
            let mut settings = self.settings.lock();
            let next = Settings {
                volume,
                muted,
                follow_source_rate,
                loop_enabled,
                // A device chosen by following the OS default is *not* pinned
                // by name: storing the name would turn "system default" into
                // "this exact interface" the next time it happened to be the
                // default one.
                device: match &source {
                    Some(s) if s.following_system_default => None,
                    _ => device,
                },
                host_id: config.host_id.clone(),
                sample_rate: (!follow_source_rate).then_some(config.fallback_rate),
                buffer_frames: config.buffer_frames,
                monitor_mode,
                eq,
                ab_enabled: ab.enabled,
                level_match: ab.level_match.enabled,
                crossfade_ms: ab.crossfade_ms,
                ..settings.clone()
            };
            let changed = *settings != next;
            *settings = next;
            changed
        };
        if changed {
            self.mark_settings_dirty();
        }
    }

    // -- snapshots ----------------------------------------------------------

    pub fn deck_handle(&self, deck: Deck) -> Option<DecodeHandle> {
        self.decks.lock()[deck.index()].handle.clone()
    }

    pub fn deck_loaded(&self, deck: Deck) -> bool {
        self.decks.lock()[deck.index()].is_loaded()
    }

    // -- blind-test integrity ------------------------------------------------

    /// Refuse an action that would invalidate a running blind test.
    ///
    /// The front end has its own guards (`blindLocked` in `src/lib/store.ts`),
    /// but those are a UX affordance: they explain *why* a button does nothing.
    /// They are not a boundary. Every path into the backend — the IPC surface,
    /// the OS "open with" handler, a file dropped on the window, the playlist
    /// auto-advance at the end of a track — has to be checked here as well,
    /// because a deck that is swapped, cleared or re-trimmed mid-trial turns the
    /// listener's remaining votes into noise *without anything looking wrong*.
    ///
    /// `what` is a capitalised subject ("Loading a track"), so the message reads
    /// the same in a toast as it does in the log.
    pub fn blind_guard(&self, what: &str) -> Result<(), String> {
        match blind_refusal(self.blind.lock().is_active(), what) {
            Some(message) => {
                // `debug`, not `warn`: this is the guard doing its job, and the
                // caller turns the `Err` into a toast the user can act on.
                log::debug!("blind-test guard refused: {what}");
                Err(message)
            }
            None => Ok(()),
        }
    }

    /// `(entry id, measured loudness)` for both decks, in one lock.
    pub fn deck_analysis(&self) -> [(Option<u64>, Option<LoudnessAnalysis>); 2] {
        let decks = self.decks.lock();
        [
            (decks[0].entry_id, decks[0].analysis()),
            (decks[1].entry_id, decks[1].analysis()),
        ]
    }

    pub fn ab_snapshot(&self) -> AbSnapshot {
        let ab = *self.ab.lock();
        AbSnapshot {
            enabled: ab.enabled,
            level_match: ab.level_match.snapshot(),
            crossfade_ms: ab.crossfade_ms,
            ab_offset_frames: self.engine.ab_offset_frames(),
        }
    }

    pub fn device_snapshot(&self) -> DeviceSnapshot {
        let source = self.engine.current_source();
        DeviceSnapshot {
            current: self.engine.current_device(),
            follow_source_rate: self.engine.follow_source_rate(),
            engine_sample_rate: self.engine.engine_rate(),
            host_id: source.as_ref().map(|s| s.host_id.clone()),
            following_system_default: source
                .as_ref()
                .map(|s| s.following_system_default)
                .unwrap_or(true),
            buffer_frames: source.as_ref().and_then(|s| s.buffer_frames),
            latency_ms: source.as_ref().and_then(|s| s.latency_ms),
        }
    }

    /// The engine source as SPEC §16 describes it, for the Settings panel.
    pub fn engine_source(&self) -> Option<EngineSource> {
        self.engine.current_source()
    }

    /// The transport line, cheap enough to build 60 times a second.
    pub fn transport(&self) -> TransportState {
        let shared = self.engine.shared();
        let active = shared.active_deck();
        let engine_rate = shared.engine_rate();
        let (durations, fraction, transparent_deck, trim) = {
            let decks = self.decks.lock();
            let slot = &decks[active.index()];
            (
                [decks[0].duration_secs(), decks[1].duration_secs()],
                slot.handle
                    .as_ref()
                    .map(|h| h.pcm.progress())
                    .unwrap_or(0.0),
                slot.handle
                    .as_ref()
                    .map(|h| h.bit_transparent && h.stored_rate == engine_rate),
                slot.trim_db,
            )
        };
        let volume = self.engine.volume();
        let muted = self.engine.muted();
        let monitor_mode = self.engine.monitor_mode();
        // Every lock is taken and released *before* the payload is built: a
        // `self.ab.lock()` inside the struct literal below would stay alive
        // until the end of the whole expression, i.e. across the engine reads
        // that follow it. Cheap here, but it is the pattern that turns into a
        // deadlock the first time one of those reads grows a lock of its own.
        let ab_enabled = self.ab.lock().enabled;
        let bit_transparent = transparent_deck.unwrap_or(false)
            // "Bit transparent" is a promise about the whole chain, not just
            // the decoder (SPEC §2 rule 5, §6/§10/§12): no sample-rate
            // conversion, EQ transparent, no audition bandpass, no monitor
            // fold, no polarity flip, unity gain, not muted, no level-match
            // trim. Every one of these is a thing a user can leave switched on
            // by accident, which is precisely why the badge has to be honest.
            && self.engine.eq_config().is_transparent()
            && self.engine.eq_audition().is_none()
            && monitor_mode == onyx_core::MonitorMode::Stereo
            && !self.engine.deck_inverted(active)
            && !muted
            && (volume - 1.0).abs() < 1.0e-4
            && trim.abs() < 1.0e-4;
        TransportState {
            playing: shared.is_playing(),
            position_secs: shared.position_secs(),
            duration_secs: durations[0].max(durations[1]),
            volume,
            muted,
            loop_enabled: self.engine.loop_enabled(),
            loop_region: self.engine.loop_region(),
            active_deck: active,
            ab_enabled,
            engine_sample_rate: engine_rate,
            buffering: shared.is_buffering(),
            decoded_fraction: fraction,
            bit_transparent,
            output_underruns: shared.underruns(),
            monitor_mode,
        }
    }

    /// The audition bandpass as the frame stream reports it. Read from the
    /// engine, so both windows see the same thing.
    pub fn audition_frame(&self) -> Option<AuditionFrame> {
        self.engine
            .eq_audition()
            .map(|(freq_hz, q)| AuditionFrame { freq_hz, q })
    }

    pub fn frame_decks(&self) -> (FrameDeck, FrameDeck) {
        let decks = self.decks.lock();
        let one = |slot: &DeckSlot| match slot.handle.as_ref() {
            None => FrameDeck {
                decoded_fraction: 0.0,
                waveform_buckets: 0,
                analysis_ready: false,
            },
            Some(h) => FrameDeck {
                decoded_fraction: h.pcm.progress(),
                waveform_buckets: h.waveform.len(),
                analysis_ready: h.status.analysis().is_some(),
            },
        };
        (one(&decks[0]), one(&decks[1]))
    }

    pub fn snapshot(&self) -> AppSnapshot {
        // Each lock is taken, read and released on its own line, in this order:
        // playlist → decks → ab → blind. Nothing below holds a guard while
        // taking the next one, and `emit_state` only emits once this returns, so
        // no lock is ever held across a Tauri `emit`.
        let playlist = self.playlist.lock().entries.clone();
        let (deck_a, deck_b) = {
            let decks = self.decks.lock();
            (decks[0].state(), decks[1].state())
        };
        let transport = self.transport();
        let ab = self.ab_snapshot();
        let blind = self.blind.lock().snapshot();
        let (appearance, theme_doc, soundfont) = {
            let settings = self.settings.lock();
            (
                settings.appearance.clone(),
                settings.theme_doc.clone(),
                settings.soundfont.clone(),
            )
        };
        AppSnapshot {
            playlist,
            deck_a: DeckSnapshot {
                state: deck_a,
                invert: self.engine.deck_inverted(Deck::A),
            },
            deck_b: DeckSnapshot {
                state: deck_b,
                invert: self.engine.deck_inverted(Deck::B),
            },
            transport,
            ab,
            blind,
            eq: self.engine.eq_config(),
            device: self.device_snapshot(),
            supported_extensions: openable_extensions()
                .iter()
                .map(|s| s.to_string())
                .collect(),
            appearance,
            theme_doc,
            soundfont,
        }
    }
}

/// The blind-test rule on its own, so it can be tested without an audio device
/// (building an `AppState` needs a real output stream, which CI does not have).
///
/// `None` = allowed. `Some(message)` = refused, with a message that names the
/// action and says how to get out of the way.
fn blind_refusal(test_active: bool, what: &str) -> Option<String> {
    if !test_active {
        return None;
    }
    Some(format!(
        "{what} is not allowed while a blind test is running — finish or abort the test first"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /* ── blind-test integrity guard ───────────────────────────────────── */

    #[test]
    fn the_blind_guard_only_refuses_while_a_test_is_running() {
        assert_eq!(blind_refusal(false, "Loading a track"), None);
        let refusal = blind_refusal(true, "Loading a track").expect("must refuse");
        // The subject the caller passed, so the toast names the action.
        assert!(refusal.starts_with("Loading a track"), "{refusal}");
        // ...and a way out, otherwise the user is just stuck.
        assert!(refusal.contains("blind test"), "{refusal}");
        assert!(refusal.contains("abort"), "{refusal}");
    }

    /* ── level matching (SPEC §10) ────────────────────────────────────── */

    #[test]
    fn level_matching_is_unity_while_disabled() {
        let lm = LevelMatch::compute(false, Some(-6.0), Some(-20.0));
        assert!(!lm.enabled && !lm.ready);
        assert_eq!(lm.trim, TrimPair { a: 0.0, b: 0.0 });
        // Even a 14 dB difference must not move a single sample: the whole
        // point of the v2 change is that nothing is touched unless asked.
        assert_eq!(lm.trim.get(Deck::A), 0.0);
        assert_eq!(lm.trim.get(Deck::B), 0.0);
    }

    #[test]
    fn level_matching_only_ever_attenuates() {
        // B is 4 dB louder, so B comes down and A stays at unity.
        let lm = LevelMatch::compute(true, Some(-18.0), Some(-14.0));
        assert!(lm.enabled && lm.ready);
        assert_eq!(lm.trim.a, 0.0, "the quieter deck must never be boosted");
        assert!((lm.trim.b + 4.0).abs() < 1.0e-6);

        // ...and the other way round.
        let lm = LevelMatch::compute(true, Some(-14.0), Some(-18.0));
        assert!((lm.trim.a + 4.0).abs() < 1.0e-6);
        assert_eq!(lm.trim.b, 0.0);

        // Never positive, whichever way the pair sits.
        for (a, b) in [(-6.0, -30.0), (-30.0, -6.0), (-9.0, -9.0), (-1.0, -60.0)] {
            let lm = LevelMatch::compute(true, Some(a), Some(b));
            assert!(lm.trim.a <= 0.0 && lm.trim.b <= 0.0, "boosted {a}/{b}");
            // One deck is always the reference, so one trim is always unity.
            assert!(lm.trim.a == 0.0 || lm.trim.b == 0.0);
        }
    }

    #[test]
    fn an_equal_pair_is_matched_at_unity() {
        let lm = LevelMatch::compute(true, Some(-14.0), Some(-14.0));
        assert!(lm.ready);
        assert_eq!(lm.trim, TrimPair { a: 0.0, b: 0.0 });
    }

    #[test]
    fn a_pending_measurement_is_reported_as_pending_not_as_matched() {
        // SPEC §10: "do not silently behave as if matched".
        for (a, b) in [(None, Some(-14.0)), (Some(-14.0), None), (None, None)] {
            let lm = LevelMatch::compute(true, a, b);
            assert!(lm.enabled, "the toggle is still on");
            assert!(!lm.ready, "{a:?}/{b:?} claimed to be matched");
            assert_eq!(lm.trim, TrimPair::default(), "trims must be unity");
        }
    }

    #[test]
    fn silence_and_nonsense_are_left_at_unity() {
        for (a, b) in [
            (LUFS_SILENCE, -14.0),
            (-14.0, LUFS_SILENCE),
            (-90.0, -14.0),
            (f32::NAN, -14.0),
            (-14.0, f32::INFINITY),
        ] {
            let lm = LevelMatch::compute(true, Some(a), Some(b));
            assert_eq!(lm.trim, TrimPair::default(), "{a}/{b} produced a trim");
            // The measurement did land, so this is not "pending".
            assert!(lm.ready, "{a}/{b} should be ready-at-unity");
        }
    }

    #[test]
    fn the_attenuation_is_clamped_but_wide_enough_for_real_masters() {
        // A 1990s master against a modern one: 14 dB, must be matched in full.
        let lm = LevelMatch::compute(true, Some(-20.0), Some(-6.0));
        assert!((lm.trim.b + 14.0).abs() < 1.0e-6);
        // Absurd differences are clamped instead of muting the deck.
        let lm = LevelMatch::compute(true, Some(-69.0), Some(-1.0));
        assert_eq!(lm.trim.b, -MAX_MATCH_TRIM_DB);
        assert_eq!(lm.trim.a, 0.0);
    }

    /* ── serialisation contract (src/lib/types.ts) ────────────────────── */

    #[test]
    fn the_level_match_payload_matches_the_front_end() {
        let json =
            serde_json::to_string(&LevelMatch::compute(true, Some(-14.0), Some(-11.0)).snapshot())
                .unwrap();
        // `LevelMatchState` in src/lib/types.ts, field for field.
        assert!(json.contains("\"enabled\":true"), "{json}");
        assert!(json.contains("\"ready\":true"), "{json}");
        assert!(json.contains("\"trimDbA\":0"), "{json}");
        assert!(json.contains("\"trimDbB\":-3"), "{json}");
    }

    #[test]
    fn the_deck_payload_carries_invert_alongside_the_core_fields() {
        let snap = DeckSnapshot {
            state: DeckState::default(),
            invert: true,
        };
        let json = serde_json::to_string(&snap).unwrap();
        // `#[serde(flatten)]` must not nest the core state under a key.
        assert!(json.contains("\"loaded\":false"), "{json}");
        assert!(json.contains("\"invert\":true"), "{json}");
        assert!(json.contains("\"waveformBuckets\":0"), "{json}");
        assert!(!json.contains("\"state\""), "flatten failed: {json}");
    }

    #[test]
    fn the_ab_payload_matches_the_front_end() {
        let snap = AbSnapshot {
            enabled: true,
            level_match: LevelMatch::default().snapshot(),
            crossfade_ms: 8.0,
            ab_offset_frames: -1234,
        };
        let json = serde_json::to_string(&snap).unwrap();
        assert!(json.contains("\"levelMatch\":{"), "{json}");
        assert!(json.contains("\"crossfadeMs\":8"), "{json}");
        assert!(json.contains("\"abOffsetFrames\":-1234"), "{json}");
    }
}
