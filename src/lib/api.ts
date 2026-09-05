/**
 * Thin typed wrappers around the Tauri IPC contract (SPEC.md §3.1,
 * SPEC.md §6–§12). One wrapper per command, no second way of doing the same
 * call, no retries, no global error handling: every caller decides what a
 * failure means and is expected to surface it (usually as a toast).
 *
 * The surface is deliberately complete: every command in the contract gets a
 * wrapper even when nothing calls it today (`transport_play`, `meters_get`,
 * `playlist_play_index`). That is the point of the module — a second, ad-hoc
 * `invoke("...")` somewhere in a component is how casing drift starts. Helpers
 * with no caller are a different matter and have been removed.
 *
 * Payload keys are camelCase throughout, per SPEC.md §3.1 ("snake_case command
 * names, camelCase payloads"); SPEC spells some argument names in Rust
 * snake_case prose (`freq_hz`), which is the same argument under the contract's
 * casing rule.
 *
 * The mock backend is selected by `__ONYX_MOCK__`, a build-time constant
 * injected by `vite.config.ts`. In a normal build it is the literal `false`, so
 * the `if` below is dead code, the dynamic `import("./mock")` is dropped by
 * rollup and `src/lib/mock.ts` is not part of the bundle at all — verified by
 * grepping the built asset. (It used to be a *static* import guarded by an
 * `import.meta.env` read, which shipped the whole mock engine to production and
 * ran its module-level `bootstrap()` on load.)
 */

import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen as tauriListen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AlignResult,
  Appearance,
  AppSnapshot,
  AudioSourceState,
  BlindMode,
  BlindState,
  CacheStats,
  Deck,
  DeviceInfo,
  EqConfig,
  EqWindowState,
  HostInfo,
  MeterSnapshot,
  MonitorMode,
  SoundFontState,
  SourceChange,
  WaveformData,
} from "./types";

/** Build-time constant. `true` only for `vite build --mode mock` / `dev:mock`. */
export const MOCK: boolean = __ONYX_MOCK__;

type MockModule = typeof import("./mock");
let mockModule: Promise<MockModule> | null = null;

function mock(): Promise<MockModule> {
  if (!mockModule) mockModule = import("./mock");
  return mockModule;
}

function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (MOCK) return mock().then((m) => m.invoke<T>(cmd, args));
  return tauriInvoke<T>(cmd, args);
}

export function errorMessage(err: unknown): string {
  if (typeof err === "string") return err;
  if (err instanceof Error) return err.message;
  if (err && typeof err === "object") {
    const message = (err as { message?: unknown }).message;
    if (typeof message === "string") return message;
  }
  try {
    return JSON.stringify(err) ?? "unknown error";
  } catch {
    return "unknown error";
  }
}

/* ── events ──────────────────────────────────────────────────────────────── */

export async function listenEvent<T>(
  event: string,
  handler: (payload: T) => void,
): Promise<UnlistenFn> {
  if (MOCK) {
    const m = await mock();
    m.ensureRunning();
    return m.listen(event, (payload) => handler(payload as T));
  }
  return tauriListen<T>(event, (e) => handler(e.payload));
}

/* ── app / playlist ──────────────────────────────────────────────────────── */

export const appState = (): Promise<AppSnapshot> => call("app_state");

export const openFiles = (paths: string[], replace: boolean): Promise<AppSnapshot> =>
  call("open_files", { paths, replace });

export const pickAndOpenFiles = (replace: boolean): Promise<AppSnapshot> =>
  call("pick_and_open_files", { replace });

export const playlistPlayIndex = (index: number): Promise<AppSnapshot> =>
  call("playlist_play_index", { index });

export const playlistPlayEntry = (id: number): Promise<AppSnapshot> =>
  call("playlist_play_entry", { id });

export const playlistRemove = (id: number): Promise<AppSnapshot> =>
  call("playlist_remove", { id });

export const playlistClear = (): Promise<AppSnapshot> => call("playlist_clear");

export const playlistMove = (from: number, to: number): Promise<AppSnapshot> =>
  call("playlist_move", { from, to });

export const playlistNext = (): Promise<AppSnapshot> => call("playlist_next");
export const playlistPrev = (): Promise<AppSnapshot> => call("playlist_prev");

/* ── transport ───────────────────────────────────────────────────────────── */

export const transportToggle = (): Promise<void> => call("transport_toggle");
export const transportPlay = (): Promise<void> => call("transport_play");
export const transportPause = (): Promise<void> => call("transport_pause");
export const transportStop = (): Promise<void> => call("transport_stop");
export const transportSeek = (secs: number): Promise<void> => call("transport_seek", { secs });
export const transportNudge = (secs: number): Promise<void> => call("transport_nudge", { secs });

export const setVolume = (value: number): Promise<void> => call("set_volume", { value });
export const setMuted = (value: boolean): Promise<void> => call("set_muted", { value });
export const setLoopEnabled = (value: boolean): Promise<void> => call("set_loop_enabled", { value });
export const setLoopRegion = (region: [number, number] | null): Promise<void> =>
  call("set_loop_region", { region });

/** Monitoring fold — SPEC §6. Returns `()`; truth arrives on the frame stream. */
export const setMonitorMode = (mode: MonitorMode): Promise<void> =>
  call("set_monitor_mode", { mode });

/* ── A/B ─────────────────────────────────────────────────────────────────── */

export const abSetEnabled = (value: boolean): Promise<AppSnapshot> =>
  call("ab_set_enabled", { value });
export const abSelect = (deck: Deck): Promise<void> => call("ab_select", { deck });
export const abToggleDeck = (): Promise<void> => call("ab_toggle_deck");
export const abAssign = (deck: Deck, id: number): Promise<AppSnapshot> =>
  call("ab_assign", { deck, id });
export const abSetCrossfadeMs = (value: number): Promise<void> =>
  call("ab_set_crossfade_ms", { value });

/** Level matching, opt-in — SPEC §10. Replaces v1's `ab_set_level_match`. */
export const setLevelMatch = (enabled: boolean): Promise<void> =>
  call("set_level_match", { enabled });

/* ── A/B time alignment (SPEC §11) ────────────────────────────────────── */

export const setAbOffset = (frames: number): Promise<void> =>
  call("set_ab_offset", { frames: Math.round(frames) });

export const autoAlignAb = (): Promise<AlignResult> => call("auto_align_ab");

export const setDeckInvert = (deck: Deck, invert: boolean): Promise<void> =>
  call("set_deck_invert", { deck, invert });

/* ── blind test (2AFC + ABX, SPEC §7) ─────────────────────────────────── */

export const blindStart = (trials: number, mode: BlindMode): Promise<BlindState> =>
  call("blind_start", { trials, mode });
/** `slot` is validated by the backend against `BlindState.slots`. */
export const blindSwitch = (slot: string): Promise<BlindState> => call("blind_switch", { slot });
export const blindVote = (slot: string): Promise<BlindState> => call("blind_vote", { slot });
export const blindAbort = (): Promise<BlindState> => call("blind_abort");

/* ── EQ (SPEC §12) ────────────────────────────────────────────────────── */

/**
 * The one authoritative setter: the front end owns the band list and always
 * sends the whole config. There are deliberately no per-band commands.
 */
export const setEq = (config: EqConfig): Promise<void> => call("set_eq", { config });

/** Band-solo audition bandpass. `null` frequency means audition off. */
export const setEqAudition = (freqHz: number | null, q: number): Promise<void> =>
  call("set_eq_audition", { freqHz, q });

/** A closed EQ window must cost zero FFT. */
export const setSpectrumEnabled = (enabled: boolean): Promise<void> =>
  call("set_spectrum_enabled", { enabled });

/* ── the EQ window (SPEC §12) ────────────────────────────────────────────── */

/*
 * The EQ lives in its own `WebviewWindow`, and Rust owns it: no webview holds
 * `core:webview:allow-create-webview-window`, so these five commands are the
 * only way a window comes into existence, moves on top or goes away. That is
 * what makes "focus the existing one instead of spawning a second" a property
 * of the system rather than a convention two front ends have to remember.
 */

/** Open it, or bring the one that already exists forward. */
export const eqWindowOpen = (): Promise<void> => call("eq_window_open");

/** Close it. Playback is untouched; Rust stops the analyser and any sweep. */
export const eqWindowClose = (): Promise<void> => call("eq_window_close");

/** `E`, from either window. */
export const eqWindowToggle = (): Promise<void> => call("eq_window_toggle");

/** Always-on-top, persisted in `settings.json`. */
export const eqWindowSetPinned = (pinned: boolean): Promise<void> =>
  call("eq_window_set_pinned", { pinned });

/** For a webview that has just loaded and has not seen the event yet. */
export const eqWindowState = (): Promise<EqWindowState> => call("eq_window_state");

/* ── waveform / device / cache / misc ────────────────────────────────────── */

export const waveformGet = (deck: Deck, from: number): Promise<WaveformData> =>
  call("waveform_get", { deck, from });

export const devicesList = (): Promise<DeviceInfo[]> => call("devices_list");
export const deviceSet = (name: string | null): Promise<AppSnapshot> => call("device_set", { name });
export const setFollowSourceRate = (value: boolean): Promise<AppSnapshot> =>
  call("set_follow_source_rate", { value });

/* ── engine source: host, device, rate, buffer (SPEC §16) ─────────────
 *
 * `devices_list` above is the v2 call and still exists (it lists the host in
 * use); these four are the whole §16 surface. Enumeration is *not* an error
 * when it comes back empty — a machine with no audio API at all is a real
 * state the panel has to render, so the wrappers pass the empty list through
 * rather than inventing a failure.
 */

/** Audio APIs this machine offers. Empty on a box with no sound. */
export const audioHosts = (): Promise<HostInfo[]> => call("audio_hosts");

/** Output devices on `hostId`, or on the host in use when it is `null`. */
export const audioDevices = (hostId: string | null): Promise<DeviceInfo[]> =>
  call("audio_devices", { hostId });

/** What is playing now, plus the lists to choose from, in one round trip. */
export const audioSource = (): Promise<AudioSourceState> => call("audio_source");

/**
 * Change host / device / rate / buffer in one stream rebuild. Safe while
 * playing; returns the snapshot, whose `device` block describes what was
 * *granted* — a device that refused 96 kHz shows up there and nowhere else.
 */
export const audioSourceSet = (change: SourceChange): Promise<AppSnapshot> =>
  call("audio_source_set", { change });

/* ── the General MIDI bank (SPEC §18) ─────────────────────────────────── */

export const soundfontGet = (): Promise<SoundFontState> => call("soundfont_get");

/** `null` goes back to the bundled bank. A bank that cannot be read rejects. */
export const soundfontSet = (path: string | null): Promise<SoundFontState> =>
  call("soundfont_set", { path });

/** Native `.sf2` picker; resolves to `null` when the dialog was cancelled. */
export const pickSoundfont = (): Promise<SoundFontState | null> => call("pick_soundfont");

/* ── appearance (SPEC §14/§15) ────────────────────────────────────────── */

/**
 * Validate, persist and broadcast the appearance. Rejects an unparseable
 * accent or font name instead of silently substituting a default (§15), and
 * returns the normalised value — `#C9A227` comes back as `#c9a227` — so the
 * panel can show what is really in force.
 */
export const setAppearance = (appearance: Appearance): Promise<Appearance> =>
  call("set_appearance", { appearance });

/* ── the theme document (SPEC §20) ────────────────────────────────────── */

/**
 * Persist a theme document, or clear it with `null`.
 *
 * Text, not a parsed theme: Rust stores the document the user pasted, comments
 * and all, and refuses only what no settings file should hold (control
 * characters, more than 256 kB). What a document *means* is decided once, in
 * `lib/themedoc.ts`, before this is ever called — the webview has already
 * applied it, so this is the persist half, and its broadcast is what re-skins
 * the other windows.
 */
export const setThemeDoc = (text: string | null): Promise<string | null> =>
  call("set_theme_doc", { text });

/**
 * Tell Rust what this window is painting its base with, so the *native window*
 * can be painted the same colour underneath the webview (SPEC §14).
 *
 * An untold window keeps the system's own background colour, which is never one
 * of Onyx's themes: on macOS it shows as a light rim around the rounded corners
 * of a dark window and as a grey flash before the first frame. Rust reads the two
 * designed themes out of `tokens.css` itself; what it cannot know is what a theme
 * document (SPEC §20) resolved to, which is why the window that wears the
 * document reports it. `theme` is the *resolved* theme — `dark` or `light`, never
 * `system` — because a document states the two separately and a colour reported
 * for one says nothing about the other.
 */
export const setWindowSurface = (color: string, theme: "dark" | "light"): Promise<void> =>
  call("set_window_surface", { color, theme });

/**
 * The escape hatch: designed themes, champagne accent, system fonts, no
 * document. Reachable from the native Appearance menu and from
 * `Ctrl/Cmd+Alt+Shift+R` in any window, including one a theme has made
 * invisible.
 */
export const resetAppearance = (): Promise<Appearance> => call("reset_appearance");

/* The editor is a third window, created by Rust for the same reason the EQ
   window is: no webview may create windows, so there can only ever be one. */
export const themeWindowOpen = (): Promise<void> => call("theme_window_open");
export const themeWindowClose = (): Promise<void> => call("theme_window_close");
export const themeWindowToggle = (): Promise<void> => call("theme_window_toggle");
export const themeWindowState = (): Promise<boolean> => call("theme_window_state");

export const metersGet = (): Promise<MeterSnapshot> => call("meters_get");
export const resetMeters = (): Promise<void> => call("reset_meters");
export const revealInFinder = (path: string): Promise<void> => call("reveal_in_finder", { path });

/** Persistent loudness cache — SPEC §8. */
export const cacheStats = (): Promise<CacheStats> => call("cache_stats");
export const cacheClear = (): Promise<CacheStats> => call("cache_clear");
