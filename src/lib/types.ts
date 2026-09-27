/**
 * Onyx IPC payload shapes — mirrors SPEC.md §3.3 and SPEC.md §6–§12 verbatim.
 * Field names must match the Rust `#[serde(rename_all = "camelCase")]` output
 * byte-for-byte; do not "improve" them.
 *
 * This file is the single source of truth for IPC shapes on the front end: no
 * component may declare its own local copy of one of these interfaces.
 */

export type Deck = "a" | "b";

/**
 * Appearance is owned by `lib/theme.ts` (it validates it and applies it), but
 * it is also an IPC payload — `set_appearance` takes one and every snapshot
 * carries one. Re-exported here so no component has to know which of the two
 * modules to import it from, and so there is still exactly one definition.
 */
export type { Appearance, SizeScale, ThemeSetting } from "./theme";
import type { Appearance } from "./theme";

export interface TrackInfo {
  path: string;
  fileName: string;
  durationSecs: number;
  sampleRate: number;
  channels: number;
  bitsPerSample: number | null;
  codec: string;
  /** container as sniffed from the file's *content* (SPEC §17) */
  container: string;
  bitrateKbps: number | null;
  isLossless: boolean;
  sizeBytes: number;
  title: string | null;
  artist: string | null;
  album: string | null;
  /** SoundFont a MIDI file is rendered through (SPEC §18); null otherwise */
  synthBank: string | null;
  /** loudness-cache salt for rendered sources; the UI never reads it */
  renderKey?: string | null;
}

export interface LoudnessAnalysis {
  integratedLufs: number;
  lra: number;
  truePeakDb: number;
  samplePeakDb: number;
}

export interface PlaylistEntry {
  id: number;
  path: string;
  fileName: string;
  title: string | null;
  artist: string | null;
  durationSecs: number;
  sampleRate: number;
  channels: number;
  codec: string;
  bitsPerSample: number | null;
  isLossless: boolean;
  analysis: LoudnessAnalysis | null;
  /** which deck it currently occupies */
  deck: Deck | null;
  /**
   * Name of the `.zip` this row was extracted from (SPEC §19), so a row
   * living in a temp directory can still say where it really came from.
   */
  archive: string | null;
  /** bank a MIDI row is rendered through (SPEC §18); null for audio files */
  synthBank: string | null;
  /** file disappeared / failed to probe */
  missing: boolean;
}

export interface DeckState {
  loaded: boolean;
  entryId: number | null;
  info: TrackInfo | null;
  durationSecs: number;
  decodedFraction: number;
  decoded: boolean;
  truncated: boolean;
  analysis: LoudnessAnalysis | null;
  trimDb: number;
  bitTransparent: boolean;
  error: string | null;
  waveformBuckets: number;
  /**
   * Polarity invert (SPEC §11, `set_deck_invert`). Optional because the
   * spec defines the command but not a read-back field; a missing value is
   * treated as `false` rather than assuming the engine reports it.
   */
  invert?: boolean;
}

/**
 * Monitoring fold on the master bus, applied *after* the meter tap
 * (SPEC.md §6) — the loudness read-outs always describe the programme.
 * `Mid` deliberately does not exist: mid is `(L+R)/2`, i.e. exactly `mono`.
 */
export type MonitorMode = "stereo" | "mono" | "left" | "right" | "swap" | "side" | "flipRight";

export interface TransportState {
  playing: boolean;
  positionSecs: number;
  durationSecs: number;
  volume: number;
  muted: boolean;
  loopEnabled: boolean;
  loopRegion: [number, number] | null;
  activeDeck: Deck;
  abEnabled: boolean;
  engineSampleRate: number;
  buffering: boolean;
  decodedFraction: number;
  bitTransparent: boolean;
  outputUnderruns: number;
  monitorMode: MonitorMode;
}

export interface MeterSnapshot {
  peakDb: [number, number];
  peakHoldDb: [number, number];
  rmsDb: [number, number];
  truePeakDb: [number, number];
  lufsMomentary: number;
  lufsShort: number;
  lufsIntegrated: number;
  lra: number;
  correlation: number;
  spectrum: number[];
  clipCount: number;
}

/* ── EQ (SPEC.md §12 — replaces the fixed 8-band strip) ───────────────── */

export type FilterKind =
  | "bell"
  | "lowShelf"
  | "highShelf"
  | "highPass"
  | "lowPass"
  | "notch"
  | "bandPass";

export interface EqBand {
  id: number;
  enabled: boolean;
  kind: FilterKind;
  /** 20 .. 20 000, clamped by the engine to < nyquist * 0.49 */
  freqHz: number;
  /** −30 .. +30; ignored by HP / LP / notch / band-pass */
  gainDb: number;
  /** 0.1 .. 40 */
  q: number;
  /** HP / LP only: 12 | 24 | 48, cascaded biquads */
  slopeDbOct: number;
}

export interface EqConfig {
  enabled: boolean;
  bands: EqBand[];
}

/** Hard cap shared with the engine's fixed-size inline array. */
export const MAX_EQ_BANDS = 16;

export const FILTER_KINDS: FilterKind[] = [
  "bell",
  "lowShelf",
  "highShelf",
  "highPass",
  "lowPass",
  "notch",
  "bandPass",
];

export const FILTER_LABEL: Record<FilterKind, string> = {
  bell: "BELL",
  lowShelf: "LOW SHELF",
  highShelf: "HIGH SHELF",
  highPass: "HIGH PASS",
  lowPass: "LOW PASS",
  notch: "NOTCH",
  bandPass: "BAND PASS",
};

export const FILTER_SHORT: Record<FilterKind, string> = {
  bell: "BELL",
  lowShelf: "L-SHLF",
  highShelf: "H-SHLF",
  highPass: "HPF",
  lowPass: "LPF",
  notch: "NOTCH",
  bandPass: "BPF",
};

/** Kinds whose gain does nothing — the node stays on the 0 dB line. */
export const GAINLESS_KINDS: ReadonlySet<FilterKind> = new Set<FilterKind>([
  "highPass",
  "lowPass",
  "notch",
  "bandPass",
]);

export const SLOPE_CHOICES = [12, 24, 48];

export interface WaveformData {
  bucketSecs: number;
  count: number;
  expected: number;
  min: number[];
  max: number[];
  rms: number[];
}

/* ── engine source: host, device, rate, buffer (SPEC §16) ───────────── */

/** An audio API (`coreaudio`, `wasapi`, `asio`, `alsa`, …). */
export interface HostInfo {
  id: string;
  name: string;
  isDefault: boolean;
  /** compiled in but uninitialisable here — no driver, no server running */
  available: boolean;
  /** `0` with `available: true` means the API works but nothing is plugged in */
  deviceCount: number;
}

/** Buffer sizes a device accepts, and the ones worth offering. */
export interface BufferRange {
  min: number;
  max: number;
  /** powers of two inside `[min, max]`, endpoints included */
  options: number[];
}

export interface DeviceInfo {
  name: string;
  isDefault: boolean;
  /** rates advertised for 2-channel float output */
  sampleRates: number[];
  hostId: string;
  defaultSampleRate: number | null;
  /** `null` when the backend only offers its own size — then offer no control */
  bufferFrames: BufferRange | null;
  maxChannels: number;
}

/** The stream as it actually is — what was *granted*, not what was asked for. */
export interface EngineSource {
  hostId: string;
  /** `null` means no stream could be opened at all */
  deviceName: string | null;
  followingSystemDefault: boolean;
  sampleRate: number;
  bufferFrames: number | null;
  latencyMs: number | null;
  followSourceRate: boolean;
}

/** Everything the "Audio device" section needs, in one round trip. */
export interface AudioSourceState {
  source: EngineSource | null;
  hosts: HostInfo[];
  /** devices on the host currently in use */
  devices: DeviceInfo[];
}

/**
 * One atomic change to the engine source. Every field absent means "leave it
 * alone", so the panel sends only what the user touched — and `deviceName:
 * null` therefore means *unchanged*, not "the system default": that is what
 * `systemDefaultDevice` is for.
 */
export interface SourceChange {
  hostId?: string;
  deviceName?: string;
  systemDefaultDevice?: boolean;
  sampleRate?: number;
  followSourceRate?: boolean;
  bufferFrames?: number;
}

/** Which SoundFont MIDI is rendered through (SPEC §18). */
export interface SoundFontState {
  /** the user's `.sf2`, or `null` for the bundled GM bank */
  path: string | null;
  /** bank name as the file declares it — the `<bank>` of `MIDI · GM · …` */
  name: string;
  bundled: boolean;
}

/* ── blind testing (SPEC.md §7) ───────────────────────────────────────── */

export type BlindMode = "ab" | "abx";

export interface BlindTrialResult {
  trial: number;
  /** slot the subject picked */
  chose: string;
  /** slot that was actually right */
  correctSlot: string;
  correct: boolean;
}

export interface BlindState {
  active: boolean;
  mode: BlindMode;
  /** 1-based while active */
  trial: number;
  trials: number;
  /** `ab`: ["x","y"] — `abx`: ["a","b","x"] */
  slots: string[];
  /** audible slot */
  currentSlot: string;
  votes: BlindTrialResult[];
  score: number;
  finished: boolean;
  /** One-tailed exact binomial P(K >= score | p = 0.5). null until finished. */
  pValue: number | null;
  /** ab mode. Both mappings are null while active; populated on reveal. */
  mapping: { x: Deck; y: Deck } | null;
  /** abx mode: which deck X was on the final trial */
  abxMapping: { x: Deck } | null;
}

/** Persistent loudness cache (SPEC.md §8). */
export interface CacheStats {
  entries: number;
  bytes: number;
  path: string;
}

/* ── A/B (SPEC.md §10, §11) ───────────────────────────────────────────── */

export interface LevelMatchState {
  enabled: boolean;
  /** false = measurement pending, trims are unity */
  ready: boolean;
  /** <= 0 */
  trimDbA: number;
  /** <= 0 */
  trimDbB: number;
}

export interface AbState {
  enabled: boolean;
  levelMatch: LevelMatchState;
  crossfadeMs: number;
  /** deck B relative to deck A, at the engine sample rate */
  abOffsetFrames: number;
}

export interface AlignResult {
  offsetFrames: number;
  offsetMs: number;
  /** 0..1, normalised correlation peak */
  confidence: number;
  /** best match was at negative correlation */
  polarityInverted: boolean;
  /** false when confidence was too low to trust */
  applied: boolean;
}

export interface DeviceState {
  current: string | null;
  followSourceRate: boolean;
  engineSampleRate: number;
  /** host the stream runs on, matching `HostInfo.id` (SPEC §16) */
  hostId: string | null;
  /** the device is whatever the OS calls default, and follows it */
  followingSystemDefault: boolean;
  /** granted buffer size; `null` when the backend chose its own */
  bufferFrames: number | null;
  /** that buffer at the running rate — the number users care about */
  latencyMs: number | null;
}

export interface AppSnapshot {
  playlist: PlaylistEntry[];
  deckA: DeckState;
  deckB: DeckState;
  transport: TransportState;
  ab: AbState;
  blind: BlindState;
  eq: EqConfig;
  device: DeviceState;
  /** audio extensions plus `.zip` (SPEC §19) */
  supportedExtensions: string[];
  /** theme, accent, fonts, size scale (SPEC §14/§15) */
  appearance: Appearance;
  /**
   * The pasted theme document, verbatim, or `null` for the two designed
   * themes (SPEC §20). Every window gets it on the snapshot, which is how a
   * theme applied in the editor re-skins the main and EQ windows.
   */
  themeDoc: string | null;
  /** the user's `.sf2`, or `null` for the bundled GM bank (SPEC §18) */
  soundfont: string | null;
}

/* ── events (SPEC.md §3.2) ───────────────────────────────────────────────── */

export interface FrameDeck {
  decodedFraction: number;
  waveformBuckets: number;
  analysisReady: boolean;
}

/**
 * The band-solo bandpass, as the *engine* reports it (SPEC §12).
 *
 * It rides on the frame stream because it crosses a window boundary: the
 * sweep is dragged in the detached EQ window, and the main window's "Band
 * solo" badge has to light for it. Neither webview can see the other's
 * JavaScript, so the engine's own state is the only thing both can agree on.
 */
export interface AuditionFrame {
  freqHz: number;
  q: number;
}

export interface FramePayload {
  transport: TransportState;
  meters: MeterSnapshot;
  deckA: FrameDeck;
  deckB: FrameDeck;
  audition: AuditionFrame | null;
}

/**
 * The detached EQ window's lifecycle, owned by Rust (`src-tauri/src/eqwindow.rs`)
 * and broadcast to every webview as `onyx://eq-window`. `open` is asked of the
 * window system, never of a flag, so the main window's EQ button cannot drift
 * from what is actually on screen.
 */
export interface EqWindowState {
  open: boolean;
  /** always-on-top; a tool window floats by default, and it is a preference */
  pinned: boolean;
}

export type ToastKind = "info" | "warn" | "error";

export interface ToastPayload {
  kind: ToastKind;
  message: string;
}

/* ── constants shared with the engine ────────────────────────────────────── */

export const SPECTRUM_BANDS = 96;
export const SPECTRUM_F_MIN = 20;
export const SPECTRUM_F_MAX = 20_000;
