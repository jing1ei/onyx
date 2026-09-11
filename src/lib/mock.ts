/**
 * Mock backend.
 *
 * The Rust side of the IPC contract is written by another agent, so during
 * front-end development `invoke` would simply reject. When the app is built or
 * served with `VITE_ONYX_MOCK=1` (or `--mode mock`), `api.ts` routes every
 * command here instead and this module synthesises a plausible `AppSnapshot`,
 * a 60 Hz frame stream and fake waveform data.
 *
 * It implements the SPEC command surface, including the parts that are only
 * observable through behaviour: level matching is opt-in and reports
 * `ready: false` until "analysis lands", the ABX mapping is never serialised
 * while a test is active, and the spectrum array is only filled while
 * `set_spectrum_enabled(true)`.
 *
 * It also stands in for the *window* half of the EQ (SPEC §12). A browser has
 * no Tauri windows, so `eq_window_open` opens `eq.html` as a popup and the
 * same `onyx://eq-window` event is emitted; the popup's copy of this module
 * notices `window.opener` and forwards every command and subscription to the
 * page that owns the engine, so the preview has one backend and one EQ config
 * exactly as the real app does. See `eqHost` below.
 *
 * Nothing here ships in a normal build: `MOCK` is a compile-time constant, so
 * the bundler drops this module when it is `false`.
 */

import type {
  AbState,
  AlignResult,
  Appearance,
  AppSnapshot,
  AudioSourceState,
  BlindMode,
  BlindState,
  CacheStats,
  Deck,
  DeckState,
  DeviceInfo,
  EngineSource,
  EqConfig,
  FramePayload,
  HostInfo,
  LoudnessAnalysis,
  MeterSnapshot,
  MonitorMode,
  PlaylistEntry,
  SoundFontState,
  SourceChange,
  TrackInfo,
  WaveformData,
} from "./types";
import { SPECTRUM_BANDS } from "./types";

type Handler = (payload: unknown) => void;

const listeners = new Map<string, Set<Handler>>();

function subscribe(event: string, handler: Handler): () => void {
  let set = listeners.get(event);
  if (!set) {
    set = new Set();
    listeners.set(event, set);
  }
  set.add(handler);
  return () => set!.delete(handler);
}

function emit(event: string, payload: unknown): void {
  const set = listeners.get(event);
  if (!set) return;
  for (const h of [...set]) {
    try {
      h(payload);
    } catch {
      // The handler may belong to the EQ popup, and a popup that has just been
      // closed leaves dead functions behind: its JavaScript realm is gone, so
      // calling one throws. Nothing can be reported to a window that no longer
      // exists — drop it, or every frame for the rest of the session throws
      // sixty times a second.
      set.delete(h);
    }
  }
}

/* ── one engine, two windows ─────────────────────────────────────────────
 *
 * The mock preview opens the EQ in a real browser popup, which loads the same
 * bundle and therefore a *second* copy of this module. Two mock engines would
 * be two sources of truth — precisely the bug the Tauri design avoids by
 * keeping the engine in Rust. So the popup does not run one: it finds the
 * page that opened it and forwards everything there, which is the same shape
 * as the real thing (one authority, both windows talking to it) with
 * `window.opener` playing the part of the IPC boundary.
 */

interface MockHost {
  invoke: <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;
  listen: (event: string, handler: Handler) => () => void;
  ensureRunning: () => void;
}

declare global {
  interface Window {
    __onyxMockHost?: MockHost;
  }
}

/** The opener's engine, when this document is the EQ popup. */
function host(): MockHost | null {
  try {
    const opener = window.opener as Window | null;
    if (!opener || opener.closed || opener === window) return null;
    return opener.__onyxMockHost ?? null;
  } catch {
    // A cross-origin opener cannot be read. Nothing to forward to.
    return null;
  }
}

export function listen(event: string, handler: Handler): () => void {
  const remote = host();
  return remote ? remote.listen(event, handler) : subscribe(event, handler);
}

/**
 * Publish this document's engine for a popup to find — unless this *is* the
 * popup, in which case there is nothing here worth finding. Deliberately
 * `subscribe`, not `listen`: what a popup wants is a seat at this window's
 * event table, never a forwarding address back to itself.
 */
if (typeof window !== "undefined" && !host()) {
  window.__onyxMockHost = { invoke, listen: subscribe, ensureRunning };
}

/* ── source material ─────────────────────────────────────────────────────── */

interface Seed {
  file: string;
  title: string;
  artist: string;
  album: string;
  dur: number;
  rate: number;
  bits: number | null;
  codec: string;
  container: string;
  lossless: boolean;
  kbps: number | null;
  lufs: number;
  lra: number;
  tp: number;
  /** seconds of head silence, so auto-align has something real to find */
  head: number;
  /** SPEC §18: a rendered MIDI file, and the bank it went through */
  midi?: boolean;
}

const SEEDS: Seed[] = [
  {
    file: "01 Nocturne in Obsidian (master v4).flac",
    title: "Nocturne in Obsidian",
    artist: "Halden Ross",
    album: "Nightglass",
    dur: 264.4,
    rate: 96000,
    bits: 24,
    codec: "flac",
    container: "FLAC",
    lossless: true,
    kbps: null,
    lufs: -14.2,
    lra: 6.8,
    tp: -0.9,
    head: 0,
  },
  {
    file: "01 Nocturne in Obsidian (master v5 \u00B7 warm).flac",
    title: "Nocturne in Obsidian",
    artist: "Halden Ross",
    album: "Nightglass",
    dur: 264.7,
    rate: 96000,
    bits: 24,
    codec: "flac",
    container: "FLAC",
    lossless: true,
    kbps: null,
    lufs: -11.6,
    lra: 5.1,
    tp: -0.2,
    head: 0.184,
  },
  {
    file: "02 Slow Amber.wav",
    title: "Slow Amber",
    artist: "Mirei Aoki",
    album: "Room Tone",
    dur: 197.2,
    rate: 48000,
    bits: 24,
    codec: "pcm_s24le",
    container: "WAV",
    lossless: true,
    kbps: null,
    lufs: -16.9,
    lra: 9.4,
    tp: -3.1,
    head: 0,
  },
  {
    file: "03 Glass Corridor.aiff",
    title: "Glass Corridor",
    artist: "Vantablack Quartet",
    album: "Long Rooms",
    dur: 421.8,
    rate: 44100,
    bits: 16,
    codec: "pcm_s16be",
    container: "AIFF",
    lossless: true,
    kbps: null,
    lufs: -18.4,
    lra: 12.2,
    tp: -6.4,
    head: 0,
  },
  {
    file: "04 Cinder (reference).m4a",
    title: "Cinder",
    artist: "Lowlight Union",
    album: "Reference Cuts",
    dur: 233.5,
    rate: 44100,
    bits: null,
    codec: "aac",
    container: "MP4",
    lossless: false,
    kbps: 256,
    lufs: -9.8,
    lra: 4.2,
    tp: 1.1,
    head: 0,
  },
  {
    file: "05 Pale Fire (stems bounce).flac",
    title: "Pale Fire",
    artist: "Halden Ross",
    album: "Nightglass",
    dur: 311.0,
    rate: 96000,
    bits: 24,
    codec: "flac",
    container: "FLAC",
    lossless: true,
    kbps: null,
    lufs: -15.7,
    lra: 7.9,
    tp: -1.7,
    head: 0,
  },
  {
    file: "06 Ashfall.mp3",
    title: "Ashfall",
    artist: "Kestrel Vane",
    album: "Singles",
    dur: 188.9,
    rate: 44100,
    bits: null,
    codec: "mp3",
    container: "MP3",
    lossless: false,
    kbps: 320,
    lufs: -8.4,
    lra: 3.6,
    tp: 1.9,
    head: 0,
  },
  {
    file: "07 Undertow (alt take).ogg",
    title: "Undertow",
    artist: "Mirei Aoki",
    album: "Room Tone",
    dur: 254.1,
    rate: 48000,
    bits: null,
    codec: "vorbis",
    container: "OGG",
    lossless: false,
    kbps: 224,
    lufs: -17.2,
    lra: 8.1,
    tp: -2.4,
    head: 0,
  },
  /* SPEC §17: a video container whose audio track is the point. The badge
     has to say `MOV · AAC` — "AAC" alone does not tell you what you were
     handed. */
  {
    file: "08 Trailer Cut (picture lock).mov",
    title: "Trailer Cut",
    artist: "Lowlight Union",
    album: "Reference Cuts",
    dur: 96.4,
    rate: 48000,
    bits: null,
    codec: "aac",
    container: "MOV",
    lossless: false,
    kbps: 320,
    lufs: -19.6,
    lra: 11.4,
    tp: -2.0,
    head: 0,
  },
  /* SPEC §18: rendered through the bundled General MIDI bank. No codec, no
     bit depth — the bank is what decides what you hear. */
  {
    file: "09 Sketch in D minor.mid",
    title: "Sketch in D minor",
    artist: "Halden Ross",
    album: "Sketches",
    dur: 142.6,
    rate: 48000,
    bits: null,
    codec: "gm",
    container: "MIDI",
    lossless: false,
    kbps: null,
    lufs: -21.3,
    lra: 14.7,
    tp: -6.8,
    head: 0,
    midi: true,
  },
];

/* ── zip archives as playlists (SPEC §19) ─────────────────────────────
   The real thing extracts to a temp directory and remembers which `.zip` each
   row came from; here the same rows are synthesised, so the preview can show
   the archive chip and the "contains no audio files" warning without a file
   system. */

/** Which seeds a given archive holds, by file name. */
const ARCHIVES: Record<string, string[]> = {
  "Nightglass masters.zip": [
    "01 Nocturne in Obsidian (master v4).flac",
    "02 Slow Amber.wav",
    "05 Pale Fire (stems bounce).flac",
    "09 Sketch in D minor.mid",
  ],
};
/** Anything else with no audio in it — the plain-spoken refusal of §19. */
const EMPTY_ARCHIVE = "Artwork and notes.zip";

const ROOT = "/Users/mix/Music/Onyx Sessions";
/** Where the real thing extracts an archive to (SPEC §19). */
const TEMP = "/var/folders/9k/T/onyx-archive-8f21c4";
let nextId = 1;

function analysisOf(s: Seed): LoudnessAnalysis {
  return {
    integratedLufs: s.lufs,
    lra: s.lra,
    truePeakDb: s.tp,
    samplePeakDb: s.tp - 0.4,
  };
}

function entryOf(s: Seed, analysed: boolean, archive?: string): PlaylistEntry {
  return {
    id: nextId++,
    // A row out of an archive really does live in a temp directory; the chip
    // in the playlist is the only thing that says where it came from.
    path: archive ? `${TEMP}/${archive.replace(/\.zip$/i, "")}/${s.file}` : `${ROOT}/${s.file}`,
    fileName: s.file,
    title: s.title,
    artist: s.artist,
    durationSecs: s.dur,
    sampleRate: s.rate,
    channels: 2,
    codec: s.codec,
    bitsPerSample: s.bits,
    isLossless: s.lossless,
    analysis: analysed ? analysisOf(s) : null,
    deck: null,
    archive: archive ?? null,
    synthBank: s.midi ? bankName() : null,
    missing: false,
  };
}

function infoOf(s: Seed): TrackInfo {
  return {
    path: `${ROOT}/${s.file}`,
    fileName: s.file,
    durationSecs: s.dur,
    sampleRate: s.rate,
    channels: 2,
    bitsPerSample: s.bits,
    codec: s.codec,
    container: s.container,
    bitrateKbps: s.kbps,
    isLossless: s.lossless,
    sizeBytes: Math.round(
      s.lossless ? s.dur * s.rate * 2 * ((s.bits ?? 16) / 8) * 0.58 : (s.dur * (s.kbps ?? 256) * 1000) / 8,
    ),
    title: s.title,
    artist: s.artist,
    album: s.album,
    synthBank: s.midi ? bankName() : null,
    renderKey: s.midi ? `sf2:${bankName()}` : null,
  };
}

function seedFor(entry: PlaylistEntry): Seed {
  return SEEDS.find((s) => s.file === entry.fileName) ?? SEEDS[0];
}

function emptyDeck(): DeckState {
  return {
    loaded: false,
    entryId: null,
    info: null,
    durationSecs: 0,
    decodedFraction: 0,
    decoded: false,
    truncated: false,
    analysis: null,
    trimDb: 0,
    bitTransparent: false,
    error: null,
    waveformBuckets: 0,
    invert: false,
  };
}

/** SPEC §12: zero bands is the normal resting state. */
function defaultEq(): EqConfig {
  return { enabled: true, bands: [] };
}

function idleBlind(mode: BlindMode = "abx"): BlindState {
  return {
    active: false,
    mode,
    trial: 0,
    trials: 0,
    slots: mode === "abx" ? ["a", "b", "x"] : ["x", "y"],
    currentSlot: mode === "abx" ? "a" : "x",
    votes: [],
    score: 0,
    finished: false,
    pValue: null,
    mapping: null,
    abxMapping: null,
  };
}

/* ── the engine source (SPEC §16) ─────────────────────────────────────
   Two hosts, one of them present but with nothing plugged into it: the empty
   enumeration is a state the panel has to render honestly, and on a machine
   with real audio hardware it is the one path that never gets exercised. */

const HOSTS: HostInfo[] = [
  { id: "coreaudio", name: "CoreAudio", isDefault: true, available: true, deviceCount: 4 },
  { id: "jack", name: "JACK", isDefault: false, available: true, deviceCount: 0 },
];

/** Powers of two inside `[min, max]`, exactly as `BufferRange::new` builds it. */
function bufferRange(min: number, max: number): { min: number; max: number; options: number[] } {
  const options: number[] = [];
  for (let n = 32; n <= 8192; n *= 2) if (n >= min && n <= max) options.push(n);
  if (!options.includes(min)) options.unshift(min);
  if (!options.includes(max)) options.push(max);
  return { min, max, options };
}

const DEVICES: DeviceInfo[] = [
  {
    name: "Apogee Symphony Desktop",
    isDefault: true,
    sampleRates: [44100, 48000, 88200, 96000, 192000],
    hostId: "coreaudio",
    defaultSampleRate: 96000,
    bufferFrames: bufferRange(32, 2048),
    maxChannels: 8,
  },
  {
    name: "MacBook Pro Speakers",
    isDefault: false,
    sampleRates: [44100, 48000],
    hostId: "coreaudio",
    defaultSampleRate: 48000,
    bufferFrames: bufferRange(64, 1024),
    maxChannels: 2,
  },
  {
    name: "RME Babyface Pro FS",
    isDefault: false,
    sampleRates: [44100, 48000, 96000, 192000],
    hostId: "coreaudio",
    defaultSampleRate: 48000,
    bufferFrames: bufferRange(32, 4096),
    maxChannels: 12,
  },
  {
    // The backend that will not be told what buffer to use: `bufferFrames`
    // is null and the panel must then offer no buffer control at all.
    name: "AirPods Max",
    isDefault: false,
    sampleRates: [48000],
    hostId: "coreaudio",
    defaultSampleRate: 48000,
    bufferFrames: null,
    maxChannels: 2,
  },
];

const devicesOnHost = (hostId: string | null): DeviceInfo[] =>
  DEVICES.filter((d) => d.hostId === (hostId ?? source.hostId));

const latencyOf = (frames: number | null, rate: number): number | null =>
  frames == null || rate <= 0 ? null : (frames / rate) * 1000;

let source: EngineSource = {
  hostId: "coreaudio",
  deviceName: "Apogee Symphony Desktop",
  followingSystemDefault: false,
  sampleRate: 96000,
  bufferFrames: 256,
  latencyMs: (256 / 96000) * 1000,
  followSourceRate: true,
};

/** The bundled General MIDI bank, or the user's `.sf2` (SPEC §18). */
let soundfont: SoundFontState = {
  path: null,
  name: "GeneralUser GS v2.0.3",
  bundled: true,
};

const bankName = (): string => soundfont.name;

/** A `.sf2`'s bank name, as the file would declare it. */
const bankNameOf = (path: string): string =>
  (path.split(/[\\/]/).pop() ?? "User bank").replace(/\.sf2$/i, "");

/**
 * Re-label every MIDI row and deck after a bank change. In the real app the
 * bank is baked into the render, so a change applies to the next load; the
 * *label* moves at once, which is what the playlist and the title bar show.
 */
function relabelBanks(): void {
  for (const e of state.playlist) if (e.synthBank) e.synthBank = soundfont.name;
  for (const d of [state.deckA, state.deckB]) {
    if (d.info?.synthBank) d.info = { ...d.info, synthBank: soundfont.name };
  }
}

/** `#rgb` / `#rrggbb`, as `settings::normalise_accent` accepts it. */
function normaliseAccent(input: unknown): string | null {
  if (typeof input !== "string") return null;
  const s = input.trim().replace(/^#/, "").toLowerCase();
  if (/^[0-9a-f]{3}$/.test(s)) return `#${s[0]}${s[0]}${s[1]}${s[1]}${s[2]}${s[2]}`;
  return /^[0-9a-f]{6}$/.test(s) ? `#${s}` : null;
}

/** The font-token grammar of `settings::normalise_font`. */
function normaliseFont(input: unknown): string | null {
  if (typeof input !== "string") return null;
  const s = input.trim();
  return s.length > 0 && s.length <= 48 && /^[A-Za-z0-9 ._-]+$/.test(s) ? s : null;
}

/** The device the OS would pick on a host — what "System default" resolves to. */
const defaultDeviceOn = (hostId: string): DeviceInfo | undefined => {
  const onHost = devicesOnHost(hostId);
  return onHost.find((d) => d.isDefault) ?? onHost[0];
};

/** Same defaults as `Appearance::default()` in `src-tauri/src/settings.rs`. */
const DEFAULT_LOOK: Appearance = {
  theme: "dark",
  accent: "#c9a227",
  uiFont: "system",
  numericFont: "system-mono",
  sizeScale: "normal",
};

/* The appearance is the one piece of mock state that has to outlive a reload.
   In the app it lives in `settings.json`, so the second launch is themed by the
   *engine*, not only by the front end's own pre-paint cache; a preview that
   forgot it would make a reload look like a regression it is not, and would hide
   a real one — a front end that theme-shifts back to dark the moment the first
   snapshot lands. `sessionStorage` because it is per-preview-tab, like a settings
   file is per-user. */
const LOOK_KEY = "onyx.mock.appearance";

function storedLook(): Appearance {
  try {
    const raw = window.sessionStorage.getItem(LOOK_KEY);
    if (!raw) return { ...DEFAULT_LOOK };
    return { ...DEFAULT_LOOK, ...(JSON.parse(raw) as Partial<Appearance>) };
  } catch {
    return { ...DEFAULT_LOOK };
  }
}

function rememberLook(look: Appearance): void {
  try {
    window.sessionStorage.setItem(LOOK_KEY, JSON.stringify(look));
  } catch {
    /* a preview with storage denied simply forgets, exactly as before */
  }
}

let appearance: Appearance = storedLook();

/* ── the theme document (SPEC §20) ───────────────────────────────────────
   Text, exactly as Rust stores it: `settings.rs` keeps the document the user
   pasted and validates only the *file* contract — real text, no control
   characters, at most 256 kB. What a document means is the front end's, in one
   implementation, and the mock must not grow a second opinion about it.

   These two constants and `normaliseThemeDoc` are the mock's half of a parity
   contract that `scripts/check-theme.mjs` holds against `src-tauri/src/
   settings.rs`: same limit, same rejections, same `null`-on-empty. Mock/Rust
   drift is what hid a shipped bug once already (see `check-ab-parity.mjs`). */

/** `settings::MAX_THEME_DOC_BYTES`. */
const MAX_THEME_DOC_BYTES = 256 * 1024;

/**
 * `settings::theme_doc_trim`.
 *
 * ASCII only, deliberately, and for a reason that is invisible until it bites:
 * `String.trim()` and Rust's `str::trim` are *different sets*. U+0085 is
 * whitespace to Rust and not to JS; U+FEFF is whitespace to JS and not to Rust.
 * Either would make one backend store a document the other clears.
 */
const asciiTrim = (s: string): string => s.replace(/^[\t\n\f\r ]+|[\t\n\f\r ]+$/g, "");

/** `settings::theme_doc_is_blank`: an emptied editor clears the theme. */
const themeDocIsBlank = (s: string): boolean => asciiTrim(s).length === 0;

/** `settings::normalise_theme_doc`, to the letter. */
function normaliseThemeDoc(raw: unknown): string | null {
  if (typeof raw !== "string") return null;
  const text = asciiTrim(raw);
  if (text.length === 0 || byteLength(text) > MAX_THEME_DOC_BYTES) return null;
  // Every control character except tab, newline and carriage return. Rust's
  // `char::is_control()` is the Unicode Cc category, so the C1 block
  // (U+0080–U+009F) counts too — a JS regex that stopped at U+007F would
  // accept a document the engine refuses.
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f-\u009f]/.test(text)) return null;
  return text;
}

/** Rust counts bytes, not UTF-16 code units, and a theme may hold em dashes. */
const byteLength = (s: string): number => new TextEncoder().encode(s).length;

/* Persisted next to the appearance and for the same reason: in the app the
   document lives in `settings.json`, so the second launch is themed by the
   engine. A preview that forgot it would make a reload look like a regression
   — and would hide the real one. */
const DOC_KEY = "onyx.mock.themeDoc";

function storedDoc(): string | null {
  try {
    return window.sessionStorage.getItem(DOC_KEY);
  } catch {
    return null;
  }
}

function rememberDoc(text: string | null): void {
  try {
    if (text === null) window.sessionStorage.removeItem(DOC_KEY);
    else window.sessionStorage.setItem(DOC_KEY, text);
  } catch {
    /* a preview with storage denied simply forgets, exactly as before */
  }
}

let themeDoc: string | null = storedDoc();

/* ── the window surface (SPEC §14) ────────────────────────────────────────
   What the main window last reported it was painting, mirroring
   `surface::REPORTED` in `src-tauri/src/surface.rs`. There is no native window
   in a browser tab, so nothing is *painted* with it; it is kept because the
   command's validation is a real boundary and `scripts/check-theme.mjs` drives
   it against the Rust rule. Not persisted, exactly like the engine's: the
   window that wears the theme reports it again on the first apply after a
   launch. */
let windowSurface: { color: string; theme: "dark" | "light" } | null = null;

/** What `set_window_surface` last accepted. For `scripts/check-theme.mjs`. */
export const reportedWindowSurface = (): { color: string; theme: "dark" | "light" } | null =>
  windowSurface;

/* ── mutable mock state ──────────────────────────────────────────────────── */

interface DeckRuntime {
  loadedAt: number;
  waveform: WaveformData | null;
  headSecs: number;
}

const ab: AbState = {
  enabled: true,
  levelMatch: { enabled: false, ready: false, trimDbA: 0, trimDbB: 0 },
  crossfadeMs: 8,
  abOffsetFrames: 0,
};

const state: AppSnapshot = {
  playlist: [],
  deckA: emptyDeck(),
  deckB: emptyDeck(),
  transport: {
    playing: true,
    positionSecs: 42.7,
    durationSecs: 0,
    volume: 0.82,
    muted: false,
    loopEnabled: false,
    loopRegion: null,
    activeDeck: "a",
    abEnabled: true,
    engineSampleRate: 96000,
    buffering: false,
    decodedFraction: 0,
    bitTransparent: true,
    outputUnderruns: 0,
    monitorMode: "stereo",
  },
  ab,
  blind: idleBlind(),
  eq: defaultEq(),
  device: {
    current: source.deviceName,
    followSourceRate: source.followSourceRate,
    engineSampleRate: source.sampleRate,
    hostId: source.hostId,
    followingSystemDefault: source.followingSystemDefault,
    bufferFrames: source.bufferFrames,
    latencyMs: source.latencyMs,
  },
  // Audio plus `.zip` — the same list `playlist::openable_extensions()` builds
  // (SPEC §17/§19); it is what the drop overlay and the dialog filter read.
  supportedExtensions: [
    "wav", "wave", "bwf", "flac", "mp3", "m4a", "mp4", "m4v", "mov", "aac", "alac", "ogg", "oga",
    "opus", "aiff", "aif", "aifc", "caf", "mka", "mkv", "webm", "adpcm", "mid", "midi", "zip",
  ],
  appearance,
  themeDoc,
  soundfont: soundfont.path,
};

const runtime: Record<Deck, DeckRuntime> = {
  a: { loadedAt: 0, waveform: null, headSecs: 0 },
  b: { loadedAt: 0, waveform: null, headSecs: 0 },
};

let spectrumEnabled = false;
let audition: { freqHz: number; q: number } | null = null;
let cache: CacheStats = {
  entries: 412,
  bytes: 96_512,
  path: "/Users/mix/Library/Caches/com.onyxaudio.player/loudness-cache.json",
};

/* ── fake waveform synthesis ─────────────────────────────────────────────── */

function hash01(n: number): number {
  const x = Math.sin(n * 127.1 + 311.7) * 43758.5453;
  return x - Math.floor(x);
}

function makeWaveform(seed: Seed, salt: number): WaveformData {
  const count = 2400;
  const bucketSecs = seed.dur / count;
  const min: number[] = new Array(count);
  const max: number[] = new Array(count);
  const rms: number[] = new Array(count);
  const loud = Math.pow(10, (seed.lufs + 14) / 20);
  const headBuckets = Math.round(seed.head / bucketSecs);
  for (let i = 0; i < count; i += 1) {
    const t = Math.max(0, (i - headBuckets) / count);
    // arrangement: intro → build → chorus → breakdown → outro
    let env =
      0.34 +
      0.3 * Math.sin(Math.PI * Math.min(1, t * 1.05)) +
      0.2 * Math.sin(t * Math.PI * 6.0 + salt) * 0.5 +
      0.18 * Math.sin(t * Math.PI * 23.0 + salt * 2.1) * 0.5;
    if (t < 0.045) env *= t / 0.045;
    if (t > 0.965) env *= (1 - t) / 0.035;
    if (t > 0.55 && t < 0.63) env *= 0.42; // breakdown
    if (i < headBuckets) env = 0;
    // beat transients
    const beat = Math.pow(Math.abs(Math.sin(t * count * 0.045 + salt)), 14);
    const noise = 0.72 + 0.28 * hash01(i * 1.37 + salt * 91.3);
    const peak = Math.min(0.995, env * noise * (0.82 + 0.5 * beat) * loud);
    const body = peak * (0.42 + 0.22 * hash01(i * 3.1 + salt));
    max[i] = peak;
    min[i] = -peak * (0.86 + 0.14 * hash01(i * 7.7 + salt));
    rms[i] = body;
  }
  return { bucketSecs, count, expected: count, min, max, rms };
}

/* ── deck loading ────────────────────────────────────────────────────────── */

function loadDeck(deck: Deck, entry: PlaylistEntry, restart: boolean): void {
  const seed = seedFor(entry);
  for (const e of state.playlist) if (e.deck === deck) e.deck = null;
  entry.deck = deck;
  const cached = entry.analysis; // "cache hit" — loudness known before decode
  const ds: DeckState = {
    loaded: true,
    entryId: entry.id,
    info: infoOf(seed),
    durationSecs: seed.dur,
    decodedFraction: 0,
    decoded: false,
    truncated: false,
    analysis: cached,
    trimDb: 0,
    bitTransparent: deck === "a",
    error: null,
    waveformBuckets: 0,
    invert: false,
  };
  if (deck === "a") state.deckA = ds;
  else state.deckB = ds;
  runtime[deck] = {
    loadedAt: performance.now(),
    waveform: makeWaveform(seed, deck === "a" ? 1 : 7),
    headSecs: seed.head,
  };
  if (restart) {
    state.transport.positionSecs = 0;
    state.transport.playing = true;
  }
  recomputeDerived();
}

function recomputeDerived(): void {
  const t = state.transport;
  t.durationSecs = Math.max(state.deckA.durationSecs, state.deckB.durationSecs);
  t.abEnabled = state.ab.enabled;

  // SPEC §10: only ever attenuate, and only when the user asked for it
  const lm = state.ab.levelMatch;
  const aL = state.deckA.analysis?.integratedLufs ?? null;
  const bL = state.deckB.analysis?.integratedLufs ?? null;
  lm.ready = lm.enabled && aL != null && bL != null && state.deckA.loaded && state.deckB.loaded;
  if (lm.enabled && lm.ready && aL != null && bL != null) {
    const target = Math.min(aL, bL);
    lm.trimDbA = Math.min(0, target - aL);
    lm.trimDbB = Math.min(0, target - bL);
  } else {
    lm.trimDbA = 0;
    lm.trimDbB = 0;
  }
  state.deckA.trimDb = lm.trimDbA;
  state.deckB.trimDb = lm.trimDbB;

  const active = t.activeDeck === "a" ? state.deckA : state.deckB;
  t.decodedFraction = active.decodedFraction;

  /* The engine source is the truth about the output side; `device` is the view
     of it the panel reads (SPEC §16). Following the source rate means the
     stream re-clocks to deck A, so the rate moves without a rebuild — and the
     latency moves with it, which is the number the buffer control shows. */
  if (source.followSourceRate && state.deckA.loaded) {
    source.sampleRate = state.deckA.info?.sampleRate ?? source.sampleRate;
  }
  t.engineSampleRate = source.sampleRate;
  source.latencyMs = latencyOf(source.bufferFrames, source.sampleRate);
  state.device = {
    current: source.deviceName,
    followSourceRate: source.followSourceRate,
    engineSampleRate: source.sampleRate,
    hostId: source.hostId,
    followingSystemDefault: source.followingSystemDefault,
    bufferFrames: source.bufferFrames,
    latencyMs: source.latencyMs,
  };
  state.appearance = appearance;
  state.themeDoc = themeDoc;
  state.soundfont = soundfont.path;

  t.bitTransparent =
    active.loaded &&
    (!state.eq.enabled || state.eq.bands.length === 0) &&
    t.monitorMode === "stereo" &&
    audition == null &&
    Math.abs(t.volume - 1) < 1e-6 &&
    Math.abs(active.trimDb) < 1e-6 &&
    active.info?.sampleRate === t.engineSampleRate;
}

function bootstrap(): void {
  state.playlist = SEEDS.map((s, i) => entryOf(s, i < 6));
  loadDeck("a", state.playlist[0], false);
  loadDeck("b", state.playlist[1], false);
  state.transport.positionSecs = 42.7;
  state.transport.engineSampleRate = 96000;
  recomputeDerived();
}
bootstrap();

/* ── snapshot cloning ────────────────────────────────────────────────────── */

function snapshot(): AppSnapshot {
  recomputeDerived();
  return structuredClone(state);
}

function pushState(): AppSnapshot {
  const snap = snapshot();
  emit("onyx://state", snap);
  return snap;
}

function toast(kind: "info" | "warn" | "error", message: string): void {
  emit("onyx://toast", { kind, message });
}

/* ── 60 Hz frame stream ──────────────────────────────────────────────────── */

let lastTick = performance.now();
let peakHold: [number, number] = [-60, -60];
let holdAge: [number, number] = [0, 0];
const spectrum: number[] = new Array<number>(SPECTRUM_BANDS).fill(-110);
let clipCount = 0;

function deckEnergy(deck: Deck, pos: number): number {
  const wf = runtime[deck].waveform;
  if (!wf) return 0;
  const i = Math.max(0, Math.min(wf.count - 1, Math.floor(pos / wf.bucketSecs)));
  return wf.max[i];
}

function bandFreq(i: number): number {
  return 20 * Math.pow(1000, i / (SPECTRUM_BANDS - 1));
}

function tick(): void {
  const now = performance.now();
  const dt = Math.min(0.1, (now - lastTick) / 1000);
  lastTick = now;
  const t = state.transport;

  // decode progress: ~5 s to fully decode a deck
  for (const deck of ["a", "b"] as Deck[]) {
    const ds = deck === "a" ? state.deckA : state.deckB;
    if (!ds.loaded) continue;
    const secs = (now - runtime[deck].loadedAt) / 1000;
    const f = Math.max(0, Math.min(1, secs / 5));
    ds.decodedFraction = f;
    ds.decoded = f >= 1;
    ds.waveformBuckets = Math.floor(f * (runtime[deck].waveform?.count ?? 0));
  }

  if (t.playing) {
    t.positionSecs += dt;
    const loop = t.loopEnabled ? t.loopRegion : null;
    if (loop && t.positionSecs >= loop[1]) t.positionSecs = loop[0];
    if (t.positionSecs >= t.durationSecs) {
      t.positionSecs = t.loopEnabled ? 0 : t.durationSecs;
      if (!t.loopEnabled) t.playing = false;
    }
  }

  const active = t.activeDeck;
  const trimDb = active === "a" ? state.ab.levelMatch.trimDbA : state.ab.levelMatch.trimDbB;
  const gain = Math.pow(10, trimDb / 20) * (t.muted ? 0 : t.volume);
  // deck B reads at playhead + offset
  const readPos =
    active === "b"
      ? t.positionSecs + state.ab.abOffsetFrames / Math.max(1, t.engineSampleRate)
      : t.positionSecs;
  const inRange = readPos >= 0 && readPos <= (active === "a" ? state.deckA : state.deckB).durationSecs;
  const env = t.playing && inRange ? deckEnergy(active, readPos) * gain : 0;
  const sec = now / 1000;

  const peak: [number, number] = [0, 0];
  const rms: [number, number] = [0, 0];
  for (let ch = 0; ch < 2; ch += 1) {
    const wobble = 0.86 + 0.14 * Math.sin(sec * (ch ? 3.1 : 2.6) + ch);
    const p = Math.max(1e-6, env * wobble);
    peak[ch] = 20 * Math.log10(Math.min(1.02, p));
    rms[ch] = 20 * Math.log10(Math.min(1, p * 0.52));
    holdAge[ch] += dt;
    if (peak[ch] >= peakHold[ch] || holdAge[ch] > 1.4) {
      peakHold[ch] = peak[ch];
      holdAge[ch] = 0;
    }
    if (peak[ch] >= -0.05) clipCount += 1;
  }

  // the analyser only runs while something is looking at it (SPEC §12)
  for (let i = 0; i < SPECTRUM_BANDS; i += 1) {
    if (!spectrumEnabled) {
      spectrum[i] = -110;
      continue;
    }
    const f = i / (SPECTRUM_BANDS - 1);
    let db = -14 - 44 * Math.pow(f, 0.9);
    db += 7 * Math.sin(sec * 1.9 + i * 0.31) * (0.35 + 0.65 * f);
    db += 4.5 * Math.sin(sec * 0.63 + i * 0.09);
    db += 9 * Math.exp(-Math.pow((f - 0.07) / 0.05, 2)) * Math.sin(sec * 4.2);
    db += 6 * Math.exp(-Math.pow((f - 0.46) / 0.09, 2)) * Math.sin(sec * 2.4 + 1.0);
    db += 20 * Math.log10(Math.max(1e-3, env + 0.02)) * 0.55;
    if (audition) {
      // audition bandpass: everything outside the band drops away
      const octaves = Math.log2(bandFreq(i) / audition.freqHz);
      db -= Math.min(60, Math.pow(octaves * audition.q * 1.6, 2) * 9);
    }
    const target = t.playing ? Math.max(-108, Math.min(-2, db)) : -108;
    // asymmetric ballistics: instant attack, slow release
    spectrum[i] = target > spectrum[i] ? target : spectrum[i] + (target - spectrum[i]) * Math.min(1, dt * 7);
  }

  const analysis = (active === "a" ? state.deckA : state.deckB).analysis;
  const base = analysis?.integratedLufs ?? -14;
  const meters: MeterSnapshot = {
    peakDb: [peak[0], peak[1]],
    peakHoldDb: [peakHold[0], peakHold[1]],
    rmsDb: [rms[0], rms[1]],
    truePeakDb: [analysis?.truePeakDb ?? -1, (analysis?.truePeakDb ?? -1) - 0.3],
    lufsMomentary: t.playing
      ? base + 6 * Math.sin(sec * 0.9) + 20 * Math.log10(Math.max(1e-3, env + 0.05)) * 0.3
      : -70,
    lufsShort: t.playing ? base + 2.2 * Math.sin(sec * 0.31) : -70,
    lufsIntegrated: base + trimDb,
    lra: analysis?.lra ?? 0,
    correlation: 0.62 + 0.3 * Math.sin(sec * 0.44) * (t.playing ? 1 : 0),
    spectrum: spectrum.slice(),
    clipCount,
  };

  recomputeDerived();
  const frame: FramePayload = {
    transport: { ...t, loopRegion: t.loopRegion ? [t.loopRegion[0], t.loopRegion[1]] : null },
    meters,
    // Read from "the engine", like the real frame: this is how the main
    // window's Band solo badge learns about a sweep happening in the EQ window.
    audition: audition ? { freqHz: audition.freqHz, q: audition.q } : null,
    deckA: {
      decodedFraction: state.deckA.decodedFraction,
      waveformBuckets: state.deckA.waveformBuckets,
      analysisReady: state.deckA.analysis != null,
    },
    deckB: {
      decodedFraction: state.deckB.decodedFraction,
      waveformBuckets: state.deckB.waveformBuckets,
      analysisReady: state.deckB.analysis != null,
    },
  };
  emit("onyx://frame", frame);
}

let started = false;
let timer = 0;

export function ensureRunning(): void {
  const remote = host();
  if (remote) {
    // The popup never runs its own clock: the frames it draws are the ones the
    // main preview window is already producing.
    remote.ensureRunning();
    return;
  }
  if (started) return;
  started = true;
  lastTick = performance.now();
  timer = window.setInterval(tick, 1000 / 60);
  window.setTimeout(() => toast("info", "Mock engine running \u00B7 no audio device attached"), 700);
}

/** Only used by tests / teardown; keeps the interval from outliving the page. */
export function stop(): void {
  if (!started) return;
  window.clearInterval(timer);
  started = false;
}

/* ── the EQ window, as far as a browser can go (SPEC §12) ────────────────── */

/**
 * A popup standing in for the Tauri `WebviewWindow`.
 *
 * A browser cannot be asked for an always-on-top tool window, and it will only
 * open a popup from a user gesture — but everything that matters about the
 * design survives: it is a *separate document with its own JavaScript*, so the
 * preview exercises the real cross-window paths (frames arriving from
 * elsewhere, `set_eq` coming back from a window this one cannot see) instead of
 * a `position: fixed` div that would prove nothing.
 *
 * What Rust does on destroy — analyser off, audition off — is done here by
 * polling `closed`, because a popup that the user closes with its own title bar
 * runs no JavaScript on the way out either.
 */
const EQ_POPUP_NAME = "onyx-eq";
const EQ_POPUP_W = 940;
const EQ_POPUP_H = 560;
/** No `closed` event exists; this is the only way to notice. */
const EQ_POPUP_POLL_MS = 300;

let eqPopup: Window | null = null;
let eqPinned = true;
let eqWatch = 0;

function eqWindowOpen(): boolean {
  return eqPopup != null && !eqPopup.closed;
}

function announceEqWindow(): void {
  emit("onyx://eq-window", { open: eqWindowOpen(), pinned: eqPinned });
}

function watchEqPopup(): void {
  if (eqWatch) return;
  eqWatch = window.setInterval(() => {
    if (eqWindowOpen()) return;
    window.clearInterval(eqWatch);
    eqWatch = 0;
    eqPopup = null;
    // Exactly what `eqwindow::on_destroyed` does in Rust.
    spectrumEnabled = false;
    spectrum.fill(-110);
    audition = null;
    announceEqWindow();
  }, EQ_POPUP_POLL_MS);
}

function openEqPopup(): void {
  if (eqWindowOpen()) {
    eqPopup?.focus();
    announceEqWindow();
    return;
  }
  // Cascaded off this window, like the real one.
  const left = Math.max(0, (window.screenX || 0) + 40);
  const top = Math.max(0, (window.screenY || 0) + Math.max(60, window.outerHeight - EQ_POPUP_H - 40));
  const features = `popup=yes,width=${EQ_POPUP_W},height=${EQ_POPUP_H},left=${left},top=${top}`;
  const url = new URL("eq.html", window.location.href).href;
  eqPopup = window.open(url, EQ_POPUP_NAME, features);
  if (!eqPopup) {
    // The one failure mode with no equivalent in the real app, so it gets a
    // message that names the cause instead of "could not open the EQ window".
    throw new Error("the browser blocked the EQ window \u2014 allow pop-ups for this preview");
  }
  watchEqPopup();
  announceEqWindow();
}

function closeEqPopup(): void {
  if (eqWindowOpen()) eqPopup?.close();
  // The watcher does the rest, on its next tick, exactly as the destroy
  // handler does in Rust.
}

/* ── the theme editor window (SPEC §20) ──────────────────────────────────── */

/**
 * `theme.html` as a popup, for the same reason the EQ window is one: it is a
 * separate document with its own JavaScript, so the preview exercises the real
 * path — a theme applied *there* re-skinning windows it cannot see, through the
 * snapshot — instead of a modal that would prove nothing about cross-window
 * sync.
 *
 * Unlike the EQ window it is never restored and never pinned (`themewindow.rs`
 * does neither), so there is no `pinned` here to mirror.
 */
const THEME_POPUP_NAME = "onyx-theme";
/** `themewindow::DEFAULT_W` / `DEFAULT_H`: a code editor is tall. */
const THEME_POPUP_W = 720;
const THEME_POPUP_H = 840;

let themePopup: Window | null = null;
let themeWatch = 0;

function themeWindowOpen(): boolean {
  return themePopup != null && !themePopup.closed;
}

function watchThemePopup(): void {
  if (themeWatch) return;
  themeWatch = window.setInterval(() => {
    if (themeWindowOpen()) return;
    window.clearInterval(themeWatch);
    themeWatch = 0;
    themePopup = null;
    // `themewindow::watch` does nothing on destroy on purpose: an applied theme
    // survives closing the editor. Nothing to undo here either.
  }, EQ_POPUP_POLL_MS);
}

function openThemePopup(): void {
  if (themeWindowOpen()) {
    themePopup?.focus();
    return;
  }
  // To the right of this window where it fits, like `first_run_position`.
  const left = Math.max(0, (window.screenX || 0) + Math.max(28, window.outerWidth - THEME_POPUP_W - 28));
  const top = Math.max(0, (window.screenY || 0) + 28);
  const features = `popup=yes,width=${THEME_POPUP_W},height=${THEME_POPUP_H},left=${left},top=${top}`;
  const url = new URL("theme.html", window.location.href).href;
  themePopup = window.open(url, THEME_POPUP_NAME, features);
  if (!themePopup) {
    throw new Error("the browser blocked the theme editor \u2014 allow pop-ups for this preview");
  }
  watchThemePopup();
}

function closeThemePopup(): void {
  if (themeWindowOpen()) themePopup?.close();
}

/* ── statistics ──────────────────────────────────────────────────────────── */

/** One-tailed exact binomial P(K >= score | n, p = 0.5). */
function binomialP(score: number, n: number): number {
  if (n <= 0) return 1;
  let coeff = 1; // C(n, k) built iteratively from k = n downwards
  let sum = 0;
  for (let k = n; k >= score; k -= 1) {
    sum += coeff;
    coeff = (coeff * k) / (n - k + 1);
  }
  return Math.min(1, sum / Math.pow(2, n));
}

/* ── command router ──────────────────────────────────────────────────────── */

function entryById(id: number): PlaylistEntry | undefined {
  return state.playlist.find((e) => e.id === id);
}

function playIndex(index: number): AppSnapshot {
  const entry = state.playlist[index];
  if (entry) loadDeck("a", entry, true);
  return pushState();
}

/** Hidden truth for the running blind test — never serialised while active. */
let abxIsA = true;
let abMapXDeck: Deck = "a";

/* ── the blind-test integrity guard ──────────────────────────────────────────
   `AppState::blind_guard` (src-tauri/src/state.rs), word for word, because the
   refusal is part of the IPC contract and the preview has to refuse the same
   commands with the same sentence. Where this drifted before — `ab_assign` —
   the preview happily did something the shipped app rejects, and the only way
   to notice was to run the packaged build.

   `deckLabel` matches `loader::deck_label`, so "Loading a track into deck B"
   reads identically on both sides. */
export function blindRefusal(testActive: boolean, what: string): string | null {
  if (!testActive) return null;
  return `${what} is not allowed while a blind test is running — finish or abort the test first`;
}

const deckLabel = (deck: Deck): string => (deck === "a" ? "A" : "B");

/** Throws exactly what the Rust command would return as `Err`. */
function blindGuard(what: string): void {
  const refusal = blindRefusal(state.blind.active, what);
  if (refusal) throw new Error(refusal);
}

export async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const remote = host();
  if (remote) return remote.invoke<T>(cmd, args);
  ensureRunning();
  await new Promise((r) => setTimeout(r, 4 + Math.random() * 10));
  const raw = (args ?? {}) as Record<string, unknown>;
  const num = (k: string): number => Number(raw[k]);
  const str = (k: string): string => String(raw[k]);
  const bool = (k: string): boolean => Boolean(raw[k]);

  switch (cmd) {
    case "editor_io":
      return { canceled: true } as T;
    case "app_state":
      return snapshot() as T;

    case "open_files": {
      const paths = ((raw.paths as string[] | undefined) ?? []).filter(Boolean);
      const replace = bool("replace");
      // `replace` clears both decks before anything loads, so the guard is here
      // rather than in `loadDeck` — `loader::open_paths` puts it in the same
      // place, and for the same reason.
      if (replace) blindGuard("Opening files");
      /* SPEC §19: a `.zip` is a playlist. Expanded here the way the real
         loader expands it — rows in natural order, each remembering the
         archive it came from, and a zip with no audio in it says so plainly
         instead of doing nothing. */
      const expanded: { seed: Seed; archive?: string }[] = [];
      const warnings: string[] = [];
      for (const p of paths) {
        const name = p.split(/[\\/]/).pop() ?? p;
        if (!/\.zip$/i.test(name)) {
          expanded.push({ seed: SEEDS[(state.playlist.length + expanded.length) % SEEDS.length] });
          continue;
        }
        const members = ARCHIVES[name] ?? (name === EMPTY_ARCHIVE ? [] : ARCHIVES["Nightglass masters.zip"]);
        if (members.length === 0) {
          warnings.push(`${name} contains no audio files`);
          continue;
        }
        for (const file of members) {
          const seed = SEEDS.find((s) => s.file === file);
          if (seed) expanded.push({ seed, archive: name });
        }
      }
      // Said before the refusal below, exactly as `loader::open_paths` does.
      for (const warning of warnings) toast("warn", warning);
      if (expanded.length === 0) throw new Error("no supported audio files in that selection");
      if (replace) {
        state.playlist = [];
        state.deckA = emptyDeck();
        state.deckB = emptyDeck();
      }
      const added: PlaylistEntry[] = expanded.map(({ seed, archive }, i) => {
        const e = entryOf(seed, false, archive);
        if (!archive) {
          const p = paths[Math.min(i, paths.length - 1)];
          e.path = p;
          e.fileName = p.split(/[\\/]/).pop() ?? seed.file;
        }
        return e;
      });
      state.playlist.push(...added);
      if (added.length && (replace || !state.deckA.loaded)) loadDeck("a", added[0], true);
      toast("info", `${added.length} file${added.length === 1 ? "" : "s"} ${replace ? "opened" : "added"}`);
      return pushState() as T;
    }

    case "pick_and_open_files": {
      const replace = bool("replace");
      if (replace) blindGuard("Opening files");
      const seed = SEEDS[Math.floor(Math.random() * SEEDS.length)];
      if (replace) state.playlist = [];
      const e = entryOf(seed, false);
      state.playlist.push(e);
      loadDeck("a", e, true);
      return pushState() as T;
    }

    case "playlist_play_index":
      blindGuard(`Loading a track into deck ${deckLabel("a")}`);
      return playIndex(num("index")) as T;

    case "playlist_play_entry": {
      blindGuard(`Loading a track into deck ${deckLabel("a")}`);
      const e = entryById(num("id"));
      if (!e) throw new Error(`no playlist entry ${num("id")}`);
      loadDeck("a", e, true);
      return pushState() as T;
    }

    case "playlist_remove": {
      const id = num("id");
      /* `commands::playlist_remove`, rule for rule:

         · removing a row that is on a deck clears that deck, and *that* is the
           material a running test is comparing — so it is refused mid-test,
           while removing any other row stays allowed;
         · clearing a deck does **not** turn A/B off. Only `ab_set_enabled`
           does (`abrules`), and lane B draws its own "empty" state, so the
           comparison the user asked for survives losing one side of it;
         · a row that is already gone is not an error: the UI is out of date and
           a snapshot is the only thing it can act on. */
      const onDecks: Deck[] = ([["a", state.deckA], ["b", state.deckB]] as const)
        .filter(([, ds]) => ds.entryId === id)
        .map(([deck]) => deck);
      if (onDecks.length > 0) blindGuard("Removing a track that is on a deck");
      state.playlist = state.playlist.filter((e) => e.id !== id);
      for (const deck of onDecks) {
        if (deck === "a") state.deckA = emptyDeck();
        else state.deckB = emptyDeck();
      }
      // `loader::clear_deck` recomputes the trims: the surviving deck must not
      // keep an attenuation measured against material that has gone.
      if (onDecks.length > 0) recomputeDerived();
      return pushState() as T;
    }

    case "playlist_clear":
      blindGuard("Clearing the playlist");
      state.playlist = [];
      state.deckA = emptyDeck();
      state.deckB = emptyDeck();
      state.transport.playing = false;
      state.transport.positionSecs = 0;
      return pushState() as T;

    case "playlist_move": {
      const from = num("from");
      const to = num("to");
      if (from >= 0 && from < state.playlist.length) {
        const [moved] = state.playlist.splice(from, 1);
        state.playlist.splice(Math.max(0, Math.min(state.playlist.length, to)), 0, moved);
      }
      return pushState() as T;
    }

    case "playlist_next":
    case "playlist_prev": {
      blindGuard(`Loading a track into deck ${deckLabel("a")}`);
      if (state.playlist.length === 0) return pushState() as T;
      const cur = state.playlist.findIndex((e) => e.id === state.deckA.entryId);
      const delta = cmd === "playlist_next" ? 1 : -1;
      const idx = (cur + delta + state.playlist.length) % state.playlist.length;
      return playIndex(idx) as T;
    }

    case "transport_toggle":
      state.transport.playing = !state.transport.playing;
      return undefined as T;
    case "transport_play":
      state.transport.playing = true;
      return undefined as T;
    case "transport_pause":
      state.transport.playing = false;
      return undefined as T;
    case "transport_stop":
      state.transport.playing = false;
      state.transport.positionSecs = 0;
      return undefined as T;
    case "transport_seek":
      state.transport.positionSecs = Math.max(0, Math.min(state.transport.durationSecs, num("secs")));
      return undefined as T;
    case "transport_nudge":
      state.transport.positionSecs = Math.max(
        0,
        Math.min(state.transport.durationSecs, state.transport.positionSecs + num("secs")),
      );
      return undefined as T;

    case "set_volume":
      state.transport.volume = Math.max(0, Math.min(1, num("value")));
      return undefined as T;
    case "set_muted":
      state.transport.muted = bool("value");
      return undefined as T;
    case "set_loop_enabled":
      state.transport.loopEnabled = bool("value");
      return undefined as T;
    case "set_loop_region": {
      const region = raw.region as [number, number] | null | undefined;
      state.transport.loopRegion = region ?? null;
      if (region) state.transport.loopEnabled = true;
      return undefined as T;
    }

    case "set_monitor_mode":
      state.transport.monitorMode = str("mode") as MonitorMode;
      return undefined as T;

    case "ab_set_enabled":
      // Turning it *off* mid-test would leave one audible deck behind two
      // slots; `blind_abort` is the explicit way to stop a run.
      if (!bool("value")) blindGuard("Switching A/B off");
      state.ab.enabled = bool("value");
      state.transport.abEnabled = state.ab.enabled;
      if (!state.ab.enabled) state.transport.activeDeck = "a";
      return pushState() as T;
    case "ab_select":
      // Only the test's own slots may move the audible deck; naming one would
      // tell the listener which side they are on.
      blindGuard("Selecting a deck by name");
      state.transport.activeDeck = str("deck") === "b" ? "b" : "a";
      return undefined as T;
    case "ab_toggle_deck":
      blindGuard("Switching decks");
      state.transport.activeDeck = state.transport.activeDeck === "a" ? "b" : "a";
      return undefined as T;
    case "ab_assign": {
      const deck: Deck = str("deck") === "b" ? "b" : "a";
      blindGuard(`Loading a track into deck ${deckLabel(deck)}`);
      const e = entryById(num("id"));
      if (!e) throw new Error(`no playlist entry ${num("id")}`);
      loadDeck(deck, e, false);
      /* SPEC §2.8, and the reason this line exists: putting material on deck B
         is a request to compare, so it turns A/B on. Rust has always done this
         (`abrules::ab_enabled_after_assign`, called by `commands::ab_assign`);
         this mock did not, so lane B was never drawn in the preview and every
         verification of "assign to B" passed here while the shipped app looked
         like deck B could not be assigned at all. The rule is pinned for both
         implementations by `src-tauri/tests/fixtures/ab_assign_contract.json`
         — `npm run check:ab` fails if this drifts again. */
      if (deck === "b") state.ab.enabled = true;
      // `recomputeDerived` (inside `snapshot`) mirrors it onto the transport
      return pushState() as T;
    }
    case "ab_set_crossfade_ms":
      state.ab.crossfadeMs = num("value");
      return undefined as T;

    case "set_level_match": {
      // The level difference between the decks is often *what* is under test.
      blindGuard("Level matching");
      state.ab.levelMatch.enabled = bool("enabled");
      // measurement "lands" a beat later, exactly as it would after a decode
      if (state.ab.levelMatch.enabled) {
        state.ab.levelMatch.ready = false;
        window.setTimeout(() => {
          recomputeDerived();
          pushState();
        }, 900);
      }
      pushState();
      return undefined as T;
    }

    case "set_ab_offset":
      blindGuard("Changing the A/B offset");
      state.ab.abOffsetFrames = Math.round(num("frames"));
      pushState();
      return undefined as T;

    case "auto_align_ab": {
      blindGuard("Auto-aligning the decks");
      if (!state.deckA.loaded || !state.deckB.loaded) {
        throw new Error("Both decks need a track before auto-align can run");
      }
      if (state.deckA.decodedFraction < 0.05 || state.deckB.decodedFraction < 0.05) {
        throw new Error("Not enough audio decoded yet \u2014 wait for the decode to reach ~5 s");
      }
      const rate = state.transport.engineSampleRate;
      const trueOffset = Math.round((runtime.b.headSecs - runtime.a.headSecs) * rate);
      // a pair of unrelated tracks gives a low-confidence result
      const related = state.deckA.info?.title === state.deckB.info?.title;
      const confidence = related ? 0.86 : 0.17;
      const applied = confidence >= 0.3;
      if (applied) state.ab.abOffsetFrames = trueOffset;
      pushState();
      const result: AlignResult = {
        offsetFrames: applied ? trueOffset : 0,
        offsetMs: ((applied ? trueOffset : 0) / rate) * 1000,
        confidence,
        polarityInverted: related && state.deckB.info?.fileName.includes("v5") === true,
        applied,
      };
      return result as T;
    }

    case "set_deck_invert": {
      blindGuard("Inverting a deck's polarity");
      const deck: Deck = str("deck") === "b" ? "b" : "a";
      const ds = deck === "a" ? state.deckA : state.deckB;
      ds.invert = bool("invert");
      pushState();
      return undefined as T;
    }

    case "blind_start": {
      const trials = Math.max(1, num("trials") || 8);
      const mode: BlindMode = str("mode") === "ab" ? "ab" : "abx";
      if (!state.deckA.loaded || !state.deckB.loaded) {
        throw new Error("Both decks need a track before a blind test can start");
      }
      abxIsA = Math.random() < 0.5;
      abMapXDeck = Math.random() < 0.5 ? "a" : "b";
      state.blind = {
        ...idleBlind(mode),
        active: true,
        trial: 1,
        trials,
        currentSlot: mode === "abx" ? "a" : "x",
      };
      state.transport.activeDeck = mode === "abx" ? "a" : abMapXDeck;
      pushState();
      return structuredClone(state.blind) as T;
    }

    case "blind_switch": {
      const slot = str("slot");
      if (!state.blind.active) throw new Error("no blind test is running");
      if (!state.blind.slots.includes(slot)) throw new Error(`unknown slot "${slot}"`);
      state.blind.currentSlot = slot;
      if (state.blind.mode === "abx") {
        state.transport.activeDeck = slot === "a" ? "a" : slot === "b" ? "b" : abxIsA ? "a" : "b";
      } else {
        state.transport.activeDeck =
          slot === "x" ? abMapXDeck : abMapXDeck === "a" ? "b" : "a";
      }
      return structuredClone(state.blind) as T;
    }

    case "blind_vote": {
      const slot = str("slot");
      const b = state.blind;
      if (!b.active) throw new Error("no blind test is running");
      const answers = b.mode === "abx" ? ["a", "b"] : ["x", "y"];
      if (!answers.includes(slot)) throw new Error(`"${slot}" is not an answer for this protocol`);
      const correctSlot =
        b.mode === "abx" ? (abxIsA ? "a" : "b") : abMapXDeck === "a" ? "x" : "y";
      const correct = slot === correctSlot;
      b.votes.push({ trial: b.trial, chose: slot, correctSlot, correct });
      if (correct) b.score += 1;
      if (b.trial >= b.trials) {
        b.active = false;
        b.finished = true;
        b.pValue = binomialP(b.score, b.votes.length);
        if (b.mode === "abx") b.abxMapping = { x: abxIsA ? "a" : "b" };
        else b.mapping = { x: abMapXDeck, y: abMapXDeck === "a" ? "b" : "a" };
      } else {
        b.trial += 1;
        abxIsA = Math.random() < 0.5;
        abMapXDeck = Math.random() < 0.5 ? "a" : "b";
        b.currentSlot = b.mode === "abx" ? "a" : "x";
        state.transport.activeDeck = b.mode === "abx" ? "a" : abMapXDeck;
      }
      pushState();
      return structuredClone(state.blind) as T;
    }

    case "blind_abort":
      state.blind = idleBlind(state.blind.mode);
      pushState();
      return structuredClone(state.blind) as T;

    case "set_eq": {
      const config = raw.config as EqConfig | undefined;
      if (!config) throw new Error("set_eq called without a config");
      if (config.bands.length > 16) throw new Error("too many EQ bands");
      state.eq = structuredClone(config);
      pushState();
      return undefined as T;
    }

    case "set_eq_audition": {
      const f = raw.freqHz;
      audition = f == null ? null : { freqHz: Number(f), q: Number(raw.q ?? 8) };
      return undefined as T;
    }

    case "set_spectrum_enabled":
      spectrumEnabled = bool("enabled");
      if (!spectrumEnabled) spectrum.fill(-110);
      return undefined as T;

    case "eq_window_open":
      openEqPopup();
      return undefined as T;
    case "eq_window_close":
      closeEqPopup();
      return undefined as T;
    case "eq_window_toggle":
      if (eqWindowOpen()) closeEqPopup();
      else openEqPopup();
      return undefined as T;
    case "eq_window_set_pinned":
      // A browser popup cannot float above other applications; the preference
      // is still recorded so the control in the panel behaves.
      eqPinned = bool("pinned");
      announceEqWindow();
      return undefined as T;
    case "eq_window_state":
      return { open: eqWindowOpen(), pinned: eqPinned } as T;

    case "waveform_get": {
      const deck: Deck = str("deck") === "b" ? "b" : "a";
      const from = Math.max(0, num("from") || 0);
      const wf = runtime[deck].waveform;
      const ds = deck === "a" ? state.deckA : state.deckB;
      if (!wf) return { bucketSecs: 0, count: 0, expected: 0, min: [], max: [], rms: [] } as T;
      if (from > wf.count) throw new Error(`waveform_get: from ${from} beyond ${wf.count}`);
      const end = Math.max(from, ds.waveformBuckets);
      return {
        bucketSecs: wf.bucketSecs,
        count: end,
        expected: wf.expected,
        min: wf.min.slice(from, end),
        max: wf.max.slice(from, end),
        rms: wf.rms.slice(from, end),
      } as T;
    }

    case "devices_list":
      return structuredClone(devicesOnHost(null)) as T;
    case "device_set": {
      const name = (raw.name as string | null | undefined) ?? null;
      source.deviceName = name ?? defaultDeviceOn(source.hostId)?.name ?? null;
      source.followingSystemDefault = name == null;
      toast("info", `Output \u2192 ${name ?? "system default"}`);
      return pushState() as T;
    }
    case "set_follow_source_rate": {
      source.followSourceRate = bool("value");
      if (!source.followSourceRate) source.sampleRate = 48000;
      return pushState() as T;
    }

    /* ── engine source (SPEC §16) ─────────────────────────────────── */

    case "audio_hosts":
      return structuredClone(HOSTS) as T;

    case "audio_devices": {
      const hostId = (raw.hostId as string | null | undefined) ?? null;
      return structuredClone(devicesOnHost(hostId)) as T;
    }

    case "audio_source":
      return {
        source: structuredClone(source),
        hosts: structuredClone(HOSTS),
        devices: structuredClone(devicesOnHost(null)),
      } as AudioSourceState as T;

    case "audio_source_set": {
      const change = (raw.change ?? {}) as SourceChange;
      const hostId = change.hostId ?? source.hostId;
      const host = HOSTS.find((h) => h.id === hostId);
      if (!host || !host.available) {
        throw new Error(`could not open that audio device: ${hostId} is not available`);
      }
      const onHost = devicesOnHost(hostId);
      if (onHost.length === 0) {
        // The real backend fails the rebuild the same way, with the device it
        // was asked for in the message.
        throw new Error(`could not open that audio device: ${host.name} reports no output devices`);
      }
      let deviceName = source.deviceName;
      let systemDefault = source.followingSystemDefault;
      if (change.systemDefaultDevice) {
        deviceName = defaultDeviceOn(hostId)?.name ?? null;
        systemDefault = true;
      } else if (change.deviceName != null) {
        deviceName = change.deviceName;
        systemDefault = false;
      }
      if (hostId !== source.hostId && !onHost.some((d) => d.name === deviceName)) {
        // A device name from the old host means nothing on the new one.
        deviceName = defaultDeviceOn(hostId)?.name ?? null;
        systemDefault = true;
      }
      const device = onHost.find((d) => d.name === deviceName) ?? onHost[0];
      const follow = change.followSourceRate ?? source.followSourceRate;
      let rate = change.sampleRate ?? source.sampleRate;
      if (follow) rate = state.deckA.info?.sampleRate ?? rate;
      if (!device.sampleRates.includes(rate)) {
        // What was *granted*, not what was asked for: the closest rate the
        // device will actually run at.
        rate = device.sampleRates.reduce((best, r) =>
          Math.abs(r - rate) < Math.abs(best - rate) ? r : best,
        );
      }
      let buffer = change.bufferFrames ?? source.bufferFrames;
      if (device.bufferFrames == null) buffer = null;
      else if (buffer != null) {
        buffer = Math.max(device.bufferFrames.min, Math.min(device.bufferFrames.max, buffer));
      }
      source = {
        hostId,
        deviceName: device.name,
        followingSystemDefault: systemDefault,
        sampleRate: rate,
        bufferFrames: buffer,
        latencyMs: latencyOf(buffer, rate),
        followSourceRate: follow,
      };
      const prefix = systemDefault ? "system default: " : "";
      const tail =
        buffer != null && source.latencyMs != null
          ? ` \u00B7 ${buffer} frames (${source.latencyMs.toFixed(1)} ms)`
          : "";
      toast("info", `Output \u2192 ${prefix}${device.name} \u00B7 ${(rate / 1000).toFixed(1)} kHz${tail}`);
      return pushState() as T;
    }

    /* ── the General MIDI bank (SPEC §18) ─────────────────────────── */

    case "soundfont_get":
      return structuredClone(soundfont) as T;

    case "soundfont_set": {
      const path = ((raw.path as string | null | undefined) ?? "").trim();
      if (!path) {
        soundfont = { path: null, name: "GeneralUser GS v2.0.3", bundled: true };
        toast("info", "SoundFont \u2192 the bundled General MIDI bank");
      } else {
        if (!/\.sf2$/i.test(path)) throw new Error(`${path} is not a SoundFont`);
        soundfont = { path, name: bankNameOf(path), bundled: false };
        toast("info", `SoundFont \u2192 ${soundfont.name}`);
      }
      // A bank change is what a MIDI row is rendered through, so every row and
      // deck that names one has to be re-labelled (SPEC §18).
      relabelBanks();
      pushState();
      return structuredClone(soundfont) as T;
    }

    case "pick_soundfont": {
      // A browser has no native file dialog to open; the preview stands in for
      // one with a plausible pick, which is enough to exercise the panel.
      const path = "/Users/mix/Library/Audio/Sounds/Banks/FluidR3 GM.sf2";
      soundfont = { path, name: bankNameOf(path), bundled: false };
      relabelBanks();
      toast("info", `SoundFont \u2192 ${soundfont.name}`);
      pushState();
      return structuredClone(soundfont) as T;
    }

    /* ── appearance (SPEC §14/§15) ────────────────────────────────── */

    case "set_appearance": {
      // Validated the way `commands::validated_appearance` validates it: an
      // unparseable accent or font name is *rejected*, not quietly replaced,
      // so the panel's error path is real in the preview too.
      const next = (raw.appearance ?? {}) as Appearance;
      const accent = normaliseAccent(next.accent);
      if (!accent) throw new Error(`"${next.accent}" is not a colour \u2014 use #rrggbb`);
      const uiFont = normaliseFont(next.uiFont);
      if (!uiFont) throw new Error(`"${next.uiFont}" is not a usable font name`);
      const numericFont = normaliseFont(next.numericFont);
      if (!numericFont) throw new Error(`"${next.numericFont}" is not a usable font name`);
      appearance = {
        theme: ["dark", "light", "system"].includes(next.theme) ? next.theme : "dark",
        accent,
        uiFont,
        numericFont,
        sizeScale: ["compact", "normal", "large"].includes(next.sizeScale)
          ? next.sizeScale
          : "normal",
      };
      rememberLook(appearance);
      pushState();
      return structuredClone(appearance) as T;
    }

    /* ── the theme document (SPEC §20) ────────────────────────────── */

    case "set_theme_doc": {
      // `commands::set_theme_doc`, to the letter: `null` and blank clear it,
      // anything else must survive the *file* contract. What the document
      // means is the front end's business — the mock, like Rust, stores text.
      const text = raw.text;
      let cleaned: string | null;
      if (text == null || (typeof text === "string" && themeDocIsBlank(text))) {
        cleaned = null;
      } else {
        cleaned = normaliseThemeDoc(text);
        if (cleaned === null) {
          throw new Error(
            "that is not a theme document Onyx can store: it must be text, " +
              `no control characters, at most ${MAX_THEME_DOC_BYTES / 1024} KB`,
          );
        }
      }
      themeDoc = cleaned;
      rememberDoc(themeDoc);
      pushState();
      return cleaned as T;
    }

    case "reset_appearance": {
      // `commands::reset_appearance_in`: designed themes, champagne accent,
      // system fonts, no document — and a toast, so the escape hatch says it
      // fired even when it fired from a window you cannot read.
      const changed =
        themeDoc !== null || JSON.stringify(appearance) !== JSON.stringify(DEFAULT_LOOK);
      appearance = { ...DEFAULT_LOOK };
      themeDoc = null;
      windowSurface = null;
      rememberLook(appearance);
      rememberDoc(null);
      if (changed) toast("info", "Appearance reset");
      pushState();
      return structuredClone(appearance) as T;
    }

    /* ── the window surface (SPEC §14) ────────────────────────────── */

    case "set_window_surface": {
      /* `commands::set_window_surface`. A browser tab has no native window to
         paint, so what the preview stands in for is the *boundary*: the same
         two rejections, with the same sentences, so the reporting path in
         `src/main.tsx` is proven against something that refuses what the engine
         refuses. `reportedWindowSurface()` is how `check-theme.mjs` reads the
         answer back — the engine's equivalent is `surface::REPORTED`. */
      const theme = str("theme");
      if (theme !== "dark" && theme !== "light") {
        throw new Error(`"${theme}" is not a resolved theme \u2014 use dark or light`);
      }
      const color = normaliseAccent(raw.color);
      if (!color) throw new Error(`"${String(raw.color)}" is not a colour \u2014 use #rrggbb`);
      windowSurface = { color, theme };
      return undefined as T;
    }

    case "theme_window_open":
      openThemePopup();
      return undefined as T;
    case "theme_window_close":
      closeThemePopup();
      return undefined as T;
    case "theme_window_toggle":
      if (themeWindowOpen()) closeThemePopup();
      else openThemePopup();
      return undefined as T;
    case "theme_window_state":
      return themeWindowOpen() as T;

    case "meters_get":
      return {
        peakDb: [-12, -12],
        peakHoldDb: [-9, -9],
        rmsDb: [-18, -18],
        truePeakDb: [-1, -1],
        lufsMomentary: -14,
        lufsShort: -14,
        lufsIntegrated: -14,
        lra: 6,
        correlation: 0.6,
        spectrum: spectrum.slice(),
        clipCount,
      } as T;
    case "reset_meters":
      clipCount = 0;
      peakHold = [-90, -90];
      holdAge = [0, 0];
      return undefined as T;

    case "cache_stats":
      return structuredClone(cache) as T;
    case "cache_clear":
      cache = { ...cache, entries: 0, bytes: 0 };
      return structuredClone(cache) as T;

    case "reveal_in_finder":
      toast("info", `Reveal \u00B7 ${str("path").split(/[\\/]/).pop() ?? ""}`);
      return undefined as T;

    default:
      throw new Error(`mock: unknown command "${cmd}"`);
  }
}
