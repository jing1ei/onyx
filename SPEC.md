# Onyx — build spec (authoritative contract)

Onyx is a **fast, professional audio playback + A/B comparison desktop app**
(Resonic-class), Tauri 2 shell, Rust audio engine, React/TS front end.
macOS first, Windows second. Style: minimal, artistic luxury, flowing.

> **This is the single authoritative spec.** It was consolidated from the
> original `SPEC.md` (v1) and separate v2 and v3 addenda — each superseded
> parts of what came before, so keeping them apart meant an earlier file read
> alone was actively misleading. Layout:
>
> * **§0–§5** — core contract: layout, core API, behaviour rules, the IPC
>   surface, visual language, build targets. Corrected for v2 and v3 throughout.
> * **§6–§12** — the v2 features and the reasoning behind them: monitor matrix,
>   ABX, loudness cache, audit requirements, opt-in level matching, A/B time
>   alignment, interactive EQ curve.
> * **§13** — the diagnostics contract: the logger, and why real-time faults are
>   counted rather than logged.
> * **§14–§19** — the v3 features: two designed themes, user customisation of
>   accent / fonts / scale, the selectable playback engine source, the full
>   format list, General MIDI playback, and zip archives as playlists.
> * **§20** — the v4 feature: the theme document, an appearance as a text file
>   you can hand to a language model and paste back.
>
> The v2 features live in **§6–§12 of this file** and the v3 features in
> **§14–§19**; no section number changed meaning in either merge. The v3
> addendum numbered its own sections §13–§18 on the assumption that §12 was the
> last one — §13 was already the diagnostics contract, so the v3 sections are
> shifted by one here and nowhere else. Both addenda are retired and deleted:
> there is one spec file. §20 was written straight into it.

### What v2 changed about v1

| Was in v1 | Now |
|---|---|
| fixed 8-band EQ strip, `eq_set_enabled` / `eq_set_preamp` / `eq_set_band` / `eq_reset` / `eq_get` / `eq_curve` IPC | **removed.** Interactive curve, 0–16 dynamic bands, one setter `set_eq { config }` — §12 |
| A/B level matching applied **automatically**, `ab_set_level_match` | **opt-in, default off, attenuate-only**, `set_level_match { enabled }` — §10 |
| single blind protocol (2AFC), `blind_start { trials }` | 2AFC **and** ABX, exact binomial p-value, `blind_start { trials, mode }` — §7 |
| no monitor fold | monitor matrix after the meter tap — §6 |
| no A/B time alignment | signed offset + auto-align — §11 |
| no persistence | loudness cache + `settings.json` — §8, §12 *Persistence* |

### What v3 changed about v2

| Was in v2 | Now |
|---|---|
| one theme (obsidian / champagne), colours partly literal in components | **two designed themes** plus `system`, every colour from the token layer — §14 |
| appearance fixed at build time | user-chosen accent, UI font, numeric font and size scale, persisted and applied live — §15 |
| output device only (`devices_list` / `device_set`) | host / device / sample rate / buffer, with the latency read-out — §16 |
| WAV, FLAC, MP3, AAC, ALAC, Vorbis | **plus Opus, AIFF/AIFC, CAF, Matroska/WebM and the audio track of MOV / MP4**, all detected by content — §17 |
| audio files only | `.mid` / `.midi` rendered through a bundled GM SoundFont — §18 |
| one file or a folder | a `.zip` is a playlist, with the security limits that implies — §19 |
| macOS pinned `"theme": "Dark"` in `tauri.macos.conf.json` | **no window theme is pinned anywhere**; the frame follows §14, `system` included |

### What v4 changed about v3

| Was in v3 | Now |
|---|---|
| appearance is six pickers (§15), and everything else about the look is fixed at build time | the whole token layer is **a document**: JSONC text, copied out, edited by hand or by a model, pasted back — §20. The pickers still exist and still work |
| `settings.json` schema 3 | **schema 4**, one new key: `themeDoc` — §20 |
| two windows (main, EQ) | **three**: the theme editor is its own webview, `theme.html` — §20 |
| no native menu | an **Appearance** menu, for the one case where the UI cannot be seen — §20 |

§3 below is the **consolidated, current** IPC surface (66 commands): if a
command is not in §3.1 it does not exist.

## 0. Repository layout

```
onyx/
  Cargo.toml               cargo workspace (src-tauri + crates/onyx-core)
                           NOTE: the build dir is ./target, not src-tauri/target
  crates/onyx-core/        audio engine crate (do not redesign)
    assets/gm/             the bundled General MIDI SoundFont + its licence (§18)
  src-tauri/               Tauri 2 app layer (state, commands, events, cache,
                           settings, persist, archives)
  src/                     React + TS front end
  scripts/                 build-mac.sh / build-windows.ps1 / make-icon.py,
                           the screenshot harness, the fixture generator
  SPEC.md                  this contract — the only spec file
  README.md                user- and contributor-facing documentation
  CONTRIBUTING.md          orientation for a new contributor: where each concern
                           lives, every check command and what it guards, the
                           invariants and the tests that enforce them, and what
                           has never been observed running
  THEMING.md               the token layer, the canvas palette (§14) and the
                           theme-document contract you paste to an agent (§20)
  THIRD-PARTY.md           bundled third-party assets and their licences (§18)
```

## 1. onyx-core public API (already implemented — use as-is)

```rust
use onyx_core::{Deck, TrackInfo, LoudnessAnalysis, EqBand, EqConfig, FilterKind,
                MonitorMode, MeterSnapshot, DeviceInfo, TransportState,
                db_to_lin, lin_to_db, MIN_DB, LUFS_SILENCE};
use onyx_core::align::{self, AlignEstimate, MIN_CONFIDENCE, MIN_ALIGN_SECS};
use onyx_core::decode::{self, DecodeHandle, DecodeStatus, DEFAULT_DECK_BUDGET_BYTES,
                        SUPPORTED_EXTENSIONS, is_supported_path, probe, open};
use onyx_core::deck::{DeckSlot, DeckState};
use onyx_core::engine::{AudioEngine, EngineConfig, RtShared, list_output_devices,
                        FALLBACK_RATE, MAX_AB_OFFSET_SECS};
use onyx_core::waveform::{Waveform, WaveformData};
```

* `decode::open(path: &Path, target_rate: u32, budget_bytes: usize) -> Result<DecodeHandle>`
  returns immediately; audio streams into `handle.pcm` (an `Arc<SharedPcm>`),
  waveform buckets into `handle.waveform`, loudness into
  `handle.status.analysis()` once finished.
* `AudioEngine::new(EngineConfig) -> Result<Arc<AudioEngine>>`
* Engine methods: `request_rate`, `set_device`, `list_devices`, `current_device`,
  `engine_rate`, `set_follow_source_rate`, `follow_source_rate`,
  `load_deck(Deck, Arc<SharedPcm>, trim_db)`, `clear_deck`,
  `set_trim_db`, `play`, `pause`, `toggle`, `stop`, `seek_secs`, `seek_frames`,
  `set_volume`, `volume`, `set_muted`, `muted`, `set_loop_enabled`,
  `loop_enabled`, `set_loop_region(Option<(f64,f64)>)`, `loop_region`,
  `set_ab_enabled`, `ab_enabled`, `select_deck`, `active_deck`,
  `set_crossfade_ms`, `crossfade_ms`,
  `set_eq(EqConfig) -> EqConfig`, `eq_config`, `eq_curve(&[f32])`,
  `set_eq_audition(Option<f32>, f32)`, `eq_audition`,
  `set_monitor_mode`, `monitor_mode`,
  `set_ab_offset_frames`, `ab_offset_frames`,
  `set_deck_invert`, `deck_inverted`,
  `set_spectrum_enabled`, `spectrum_enabled`,
  `meters() -> MeterSnapshot`, `reset_meters`, `shared() -> &Arc<RtShared>`.
  There is **no** `set_eq_enabled` / `set_eq_preamp` / `set_eq_band` /
  `set_eq_config`: `set_eq` is the single authoritative setter (§12).
* `RtShared`: `position_secs`, `position_frames`, `is_playing`, `is_buffering`,
  `underruns`, `active_deck`, `engine_rate`, `take_ended()`.
* `align::estimate_from_pcm` / `align::estimate_offset` — two-stage A/B offset
  estimation (§11), never called from the audio thread.

## 2. Behaviour requirements (non-negotiable)

1. **Open = replace.** Opening file(s) through the dialog, the OS ("open with"),
   or CLI args **clears the playlist**, loads the first file and **plays it
   immediately**.
2. **Drop = append.** Files dropped on the window are appended; the playlist is
   *not* cleared and playback is *not* interrupted. (Exception: if the playlist
   is empty, the first dropped file starts playing.)
3. **One tap = play.** A single click on a playlist row loads it into deck A and
   plays from 0 with no confirmation, no double-click.
4. **Instant.** Perceived click-to-sound must stay under ~50 ms: `decode::open`
   returns after the header parse and the engine starts as soon as ~150 ms of
   audio exists (the core already handles the starving case).
5. **Accurate.** Engine follows the source sample rate (`request_rate`) so deck A
   is not resampled. Report `bitTransparent` only when *all* of: engine rate ==
   source rate, PCM stored at that rate, EQ transparent, no audition bandpass,
   `monitorMode == "stereo"`, deck not polarity-inverted, not muted,
   volume == 1.0, trim == 0.
6. **A/B.** Two decks share one playhead. Switching is instant, position
   preserved. Level matching is **opt-in and attenuate-only** (§10). Deck B may
   carry a signed time offset (§11). Blind mode has two protocols, randomises
   the mapping, hides identity, records votes and reveals a summary with an
   exact binomial p-value (§7).
7. **Two waveforms.** When A/B is enabled the waveform area shows deck A above
   deck B in one column, above the playlist, sharing one playhead and one
   click/drag seek surface. When A/B is off, one lane.
8. **Deck B is reachable.** Rule 3 gives a plain click one meaning and one only
   — *load deck A and play* — so putting material on **deck B** needs routes of
   its own, and they are part of the contract, not decoration. A build in which
   deck B can only be reached by a gesture nothing on screen mentions is a build
   in which deck B cannot be assigned; that was reported from the field as
   "deck b is not assignable, only a", and every route below exists because of
   it. All four call the same `ab_assign` command (§3.1):

   | route | gesture |
   |---|---|
   | row chips | `A` / `B` buttons in the playlist's Deck column. The chip for the deck a row is on is **always** visible and lit in that deck's accent; the other appears on row hover, on the selected row, and whenever either chip has keyboard focus. Never hover-only. |
   | drag to a lane | Drag a playlist row onto a waveform lane. The lane must highlight in that deck's accent **before** the drop and name what will happen. |
   | drop a file on a lane | An OS file dropped on a lane is appended to the playlist and assigned to *that* deck. The lane takes precedence over the window-level drop of rule 2, which is unchanged everywhere else. |
   | keyboard | `⇧A` / `⇧B` assign the **selected** row (§4). With nothing selected, say so rather than doing nothing. |

   Also required:

   * **Assigning deck B enables A/B.** Putting material on B is a request to
     compare, so `ab.enabled` becomes true; assigning deck A never turns it off.
     This rule is one line in `src-tauri/src/abrules.rs`, and the fixture
     `src-tauri/tests/fixtures/ab_assign_contract.json` pins it for **both**
     implementations of the backend — Rust (`src-tauri/tests/ab_assign_contract.rs`)
     and the browser mock (`scripts/check-ab-parity.mjs`, in `npm run build`).
     The mock once left A/B off here, which made every preview verification of
     deck B pass while the shipped app looked broken.
   * **Assignment is not selection.** `A` / `B` (and the A|B buttons, and a
     click on a lane) change only *which deck you hear*. The audible lane is
     marked in words and in its deck's colour, and the other lane says `silent`,
     so a switch can never be misread as a failed assignment.
   * **An empty lane explains itself.** With A/B on and a deck holding nothing,
     that lane names the routes above instead of drawing a blank rectangle.
   * A blind test refuses all of it, like every other deck-changing action (§7).

## 3. Tauri IPC contract

### 3.1 Commands (`#[tauri::command]`, snake_case names, camelCase payloads)

The complete registered surface — 66 commands. Every one returns
`Result<T, String>`; nothing panics on user input. This table, the
`generate_handler!` list, the `src/lib/api.ts` wrappers and the `case` labels of
`src/lib/mock.ts`'s dispatch are held equal in every direction — the count above
included — by `scripts/check-ipc.mjs` (`npm run build`, `npm run build:mock`) and
by `every_front_end_command_is_registered_and_nothing_extra_is` in
`src-tauri/src/lib.rs`.

| command | args | returns |
|---|---|---|
| `app_state` | – | `AppSnapshot` |
| `open_files` | `paths: Vec<String>`, `replace: bool` | `AppSnapshot` |
| `pick_and_open_files` | `replace: bool` | `AppSnapshot` |
| `playlist_play_index` | `index: usize` | `AppSnapshot` |
| `playlist_play_entry` | `id: u64` | `AppSnapshot` |
| `playlist_remove` | `id: u64` | `AppSnapshot` |
| `playlist_clear` | – | `AppSnapshot` |
| `playlist_move` | `from: usize`, `to: usize` | `AppSnapshot` |
| `playlist_next` / `playlist_prev` | – | `AppSnapshot` |
| `transport_toggle` / `transport_play` / `transport_pause` / `transport_stop` | – | `()` |
| `transport_seek` | `secs: f64` | `()` |
| `transport_nudge` | `secs: f64` (relative) | `()` |
| `set_volume` | `value: f32` | `()` |
| `set_muted` | `value: bool` | `()` |
| `set_loop_enabled` | `value: bool` | `()` |
| `set_loop_region` | `region: Option<(f64,f64)>` | `()` |
| `set_monitor_mode` | `mode: MonitorMode` | `()` — §6 |
| `ab_set_enabled` | `value: bool` | `AppSnapshot` |
| `ab_select` | `deck: "a" \| "b"` | `()` |
| `ab_toggle_deck` | – | `()` |
| `ab_assign` | `deck: "a" \| "b"`, `id: u64` | `AppSnapshot` |
| `ab_set_crossfade_ms` | `value: f32` | `()` |
| `set_level_match` | `enabled: bool` | `()` — §10 |
| `set_ab_offset` | `frames: f64` (whole frames, clamped ±30 s) | `()` — §11 |
| `auto_align_ab` | – | `AlignResult` — §11 |
| `set_deck_invert` | `deck: "a" \| "b"`, `invert: bool` | `()` — §11 |
| `blind_start` | `trials: usize`, `mode: "ab" \| "abx"` | `BlindState` — §7 |
| `blind_switch` | `slot: String` | `BlindState` |
| `blind_vote` | `slot: String` | `BlindState` |
| `blind_abort` | – | `BlindState` |
| `set_eq` | `config: EqConfig` | `()` — §12, the only EQ setter |
| `set_eq_audition` | `freqHz: Option<f32>`, `q: f32` | `()` — §12 |
| `set_spectrum_enabled` | `enabled: bool` | `()` — §12 |
| `eq_window_open` / `eq_window_close` / `eq_window_toggle` | – | `()` — §12, the window is Rust's to create |
| `eq_window_set_pinned` | `pinned: bool` | `()` — §12 |
| `eq_window_state` | – | `EqWindowState` — §12 |
| `waveform_get` | `deck: "a" \| "b"`, `from: usize` | `WaveformData` |
| `devices_list` | – | `Vec<DeviceInfo>` |
| `device_set` | `name: Option<String>` | `AppSnapshot` |
| `set_follow_source_rate` | `value: bool` | `AppSnapshot` |
| `audio_hosts` | – | `Vec<HostInfo>` — §16 |
| `audio_devices` | `hostId: Option<String>` | `Vec<DeviceInfo>` — §16 |
| `audio_source` | – | `AudioSourceState` — §16 |
| `audio_source_set` | `change: SourceChange` | `AppSnapshot` — §16, one stream rebuild |
| `soundfont_get` | – | `SoundFontState` — §18 |
| `soundfont_set` | `path: Option<String>` | `SoundFontState` — §18, `null` = the bundled bank |
| `pick_soundfont` | – | `Option<SoundFontState>` — §18, `null` = cancelled |
| `set_appearance` | `appearance: Appearance` | `Appearance` — §14/§15, normalised, rejects bad input |
| `set_theme_doc` | `text: Option<String>` | `Option<String>` — §20, the stored text; `null` clears it |
| `set_window_surface` | `color: String` (`#rrggbb`), `theme: "dark" \| "light"` | `()` — §14, the window's own background colour |
| `reset_appearance` | – | `Appearance` — §20, document *and* appearance back to stock |
| `theme_window_open` / `theme_window_close` / `theme_window_toggle` | – | `()` — §20, the window is Rust's to create |
| `theme_window_state` | – | `bool` — §20, is the editor window open |
| `meters_get` | – | `MeterSnapshot` |
| `reset_meters` | – | `()` |
| `cache_stats` / `cache_clear` | – | `CacheStats` — §8 |
| `reveal_in_finder` | `path: String` | `()` |

Removed in v2 (do not reintroduce): `ab_set_level_match`, `eq_set_enabled`,
`eq_set_preamp`, `eq_set_band`, `eq_reset`, `eq_get`, `eq_curve`.
The EQ curve is evaluated in the front end from the same coefficients; the
snapshot's `eq` field is the authoritative config.

### 3.2 Events emitted to the front end

* `onyx://frame` — at 60 Hz, payload `FramePayload`:
  ```ts
  { transport: TransportState, meters: MeterSnapshot,
    deckA: { decodedFraction: number, waveformBuckets: number, analysisReady: boolean },
    deckB: { ... } }
  ```
* `onyx://state` — whenever the playlist / decks / A/B config changes, payload `AppSnapshot`.
* `onyx://toast` — `{ kind: "info"|"warn"|"error", message: string }`.
* `onyx://eq-window` — `EqWindowState`, broadcast to *both* webviews whenever the
  EQ window opens, closes or is pinned (§12).
* `onyx://client-log` — emitted **by** the webview, not to it (§13).

### 3.3 Shapes

```ts
type Deck = "a" | "b";
type MonitorMode = "stereo" | "mono" | "left" | "right" | "swap" | "side" | "flipRight";
type BlindMode = "ab" | "abx";
type FilterKind = "bell" | "lowShelf" | "highShelf" | "highPass" | "lowPass" | "notch" | "bandPass";

interface TrackInfo {
  path: string; fileName: string; durationSecs: number; sampleRate: number;
  channels: number; bitsPerSample: number | null; codec: string;
  container: string;              // sniffed from content, never the extension — §17
  bitrateKbps: number | null; isLossless: boolean;
  sizeBytes: number; title: string | null; artist: string | null; album: string | null;
  synthBank: string | null;       // §18 — the bank a MIDI file was rendered through
  renderKey?: string | null;      // §18 — loudness-cache salt; the UI never reads it
}
interface LoudnessAnalysis {
  integratedLufs: number; lra: number; truePeakDb: number; samplePeakDb: number;
}
interface PlaylistEntry {
  id: number; path: string; fileName: string; title: string | null;
  artist: string | null; durationSecs: number; sampleRate: number;
  channels: number; codec: string; bitsPerSample: number | null;
  isLossless: boolean; analysis: LoudnessAnalysis | null;
  deck: Deck | null;          // which deck it currently occupies
  archive: string | null;     // §19 — the .zip this row came out of
  synthBank: string | null;   // §18 — null for anything that is not MIDI
  missing: boolean;           // file disappeared / failed to probe
}
interface DeckState {
  loaded: boolean; entryId: number | null; info: TrackInfo | null;
  durationSecs: number; decodedFraction: number; decoded: boolean;
  truncated: boolean; analysis: LoudnessAnalysis | null; trimDb: number;
  bitTransparent: boolean; error: string | null; waveformBuckets: number;
}
/** `AppSnapshot.deckA` / `deckB`: DeckState flattened, plus the polarity flip. */
interface DeckSnapshot extends DeckState { invert: boolean }
interface TransportState {
  playing: boolean; positionSecs: number; durationSecs: number; volume: number;
  muted: boolean; loopEnabled: boolean; loopRegion: [number, number] | null;
  activeDeck: Deck; abEnabled: boolean; engineSampleRate: number;
  buffering: boolean; decodedFraction: number; bitTransparent: boolean;
  outputUnderruns: number;
  monitorMode: MonitorMode;                        // §6
}
interface MeterSnapshot {
  peakDb: [number, number]; peakHoldDb: [number, number];
  rmsDb: [number, number]; truePeakDb: [number, number];
  lufsMomentary: number; lufsShort: number; lufsIntegrated: number;
  lra: number; correlation: number; spectrum: number[];  // 96 log bands
  clipCount: number;
}
/** §12 — dynamic list, 0..16 bands. No preamp, no fixed indices. */
interface EqBand {
  id: number; enabled: boolean; kind: FilterKind;
  freqHz: number;      // 20..20000, clamped below nyquist * 0.49
  gainDb: number;      // -30..+30, ignored by highPass/lowPass/notch/bandPass
  q: number;           // 0.1..40
  slopeDbOct: number;  // highPass/lowPass only: 12 | 24 | 48
}
interface EqConfig { enabled: boolean; bands: EqBand[] }
/** §10 — opt-in, default false, both trims <= 0. */
interface LevelMatchState {
  enabled: boolean; ready: boolean; trimDbA: number; trimDbB: number;
}
/** §11 */
interface AlignResult {
  offsetFrames: number; offsetMs: number; confidence: number;
  polarityInverted: boolean; applied: boolean;
}
interface WaveformData {
  bucketSecs: number; count: number; expected: number;
  min: number[]; max: number[]; rms: number[];
}
/** §16 — the engine source. Enumeration finds nothing on a machine with no
    audio API; that is a state to render, not an error. */
interface HostInfo {
  id: string; name: string; isDefault: boolean;
  available: boolean;        // compiled in but uninitialisable here
  deviceCount: number;       // 0 with available: true = the API works, nothing plugged in
}
interface BufferRange { min: number; max: number; options: number[] }
interface DeviceInfo {
  name: string; isDefault: boolean; sampleRates: number[]; hostId: string;
  defaultSampleRate: number | null;
  bufferFrames: BufferRange | null;   // null: the backend only offers its own size
  maxChannels: number;
}
/** What was *granted*, not what was asked for. */
interface EngineSource {
  hostId: string; deviceName: string | null; followingSystemDefault: boolean;
  sampleRate: number; bufferFrames: number | null; latencyMs: number | null;
  followSourceRate: boolean;
}
interface AudioSourceState { source: EngineSource | null; hosts: HostInfo[]; devices: DeviceInfo[] }
/** Every field absent means "leave it alone", so `deviceName: null` is
    *unchanged* — `systemDefaultDevice` is how you ask for the OS default. */
interface SourceChange {
  hostId?: string; deviceName?: string; systemDefaultDevice?: boolean;
  sampleRate?: number; followSourceRate?: boolean; bufferFrames?: number;
}
/** §18 */
interface SoundFontState { path: string | null; name: string; bundled: boolean }
/** §14/§15 — mirrored by `src/lib/theme.ts`, which owns the curated font and
    accent lists; this layer only guarantees a token can be nothing else. */
interface Appearance {
  theme: "dark" | "light" | "system";     // default "dark"
  accent: string;                         // #rrggbb, lower case
  uiFont: string; numericFont: string;    // font tokens, not stacks
  sizeScale: "compact" | "normal" | "large";
}
/** §12 */
interface EqWindowState { open: boolean; pinned: boolean }
/** §7 — two protocols, exact binomial p-value. */
interface BlindTrialResult {
  trial: number; chose: string; correctSlot: string; correct: boolean;
}
interface BlindState {
  active: boolean; mode: BlindMode; trial: number; trials: number;
  slots: string[];               // ab: ["x","y"]   abx: ["a","b","x"]
  currentSlot: string;           // which slot is audible right now
  votes: BlindTrialResult[];
  score: number; finished: boolean;
  pValue: number | null;         // P(K >= score | p = 0.5); null until finished
  // never leak either mapping while active
  mapping: { x: Deck; y: Deck } | null;   // ab mode, revealed when finished
  abxMapping: { x: Deck } | null;         // abx mode, revealed when finished
}
/** §8 */
interface CacheStats { entries: number; bytes: number; path: string }
interface AppSnapshot {
  playlist: PlaylistEntry[];
  deckA: DeckSnapshot; deckB: DeckSnapshot;
  transport: TransportState;
  ab: { enabled: boolean; levelMatch: LevelMatchState; crossfadeMs: number;
        abOffsetFrames: number };
  blind: BlindState;
  eq: EqConfig;
  device: { current: string | null; followSourceRate: boolean; engineSampleRate: number;
            hostId: string | null; followingSystemDefault: boolean;
            bufferFrames: number | null; latencyMs: number | null };   // §16
  supportedExtensions: string[];   // audio extensions plus `zip` — §19
  appearance: Appearance;          // §14/§15
  themeDoc: string | null;         // §20 — the pasted theme's source text, verbatim
  soundfont: string | null;        // §18 — null = the bundled bank
}
```

## 4. Visual design language

**Mood:** obsidian gallery at night. Dark, quiet, expensive. Nothing shouts.
Motion is *flowing* — long, silky, ease-out curves; audio-reactive shimmer on the
waveform; never bouncy, never playful.

The values below are the **dark** theme, and they are unchanged. Since v3 they
live in the token layer (`src/styles/tokens.css`) alongside a second, designed
light theme, and the accent is user-choosable — see §14 and §15. No component,
canvas included, may hold a colour literal; the numbers here describe what the
tokens resolve to at the default accent, not where a colour may be written.

* **Palette**
  * `--ink-900 #0A0A0C` app background
  * `--ink-800 #101014` panels
  * `--ink-700 #16161B` raised surfaces
  * `--hairline rgba(255,255,255,0.06)` 1px separators
  * `--text-hi rgba(255,255,255,0.92)`, `--text-mid rgba(255,255,255,0.56)`,
    `--text-lo rgba(255,255,255,0.30)`
  * `--gold #C9A227` → `--gold-2 #E8D9A0` (champagne accent, used sparingly)
  * deck A accent `--deck-a #E8D9A0` (champagne), deck B accent
    `--deck-b #7FA8B8` (cold steel blue) — the two are instantly separable.
  * meter scale: `#6FCF97` (safe) → `#E8D9A0` (-6) → `#E07A5F` (-1) → `#D64545` (clip)
* **Type:** system UI stack for labels; tabular numerals everywhere numbers
  change (`font-variant-numeric: tabular-nums`). Uppercase 10–11px labels with
  0.14em letter-spacing. No font files to download.
* **Geometry:** 2px radii on controls, 10px on panels, 1px hairlines instead of
  heavy borders, generous negative space, no drop shadows except a single soft
  ambient one on floating panels.
* **Motion:** `cubic-bezier(0.22, 1, 0.36, 1)`, 240–420 ms for panels, 120 ms for
  hover states. Waveform playhead moves every frame (rAF), never with CSS
  transitions.
* **Custom title bar** (`decorations: false`, `titleBarStyle: Overlay` on macOS):
  36 px tall, drag region, traffic-light inset of 78 px on macOS, right side
  holds the format badge and the settings/EQ toggles.

### Layout (main window: 1180×760 default, min 420×560)

```
┌──────────────────────────────────────────────────────────────┐
│ titlebar: ONYX · file name · FLAC 24/96 · -14.2 LUFS   ⚙ EQ │ 36px
├───────────────────────────────────┬──────────────────────────┤
│ waveform lane A (deck badge, name)│ LUFS I ·  M / S          │
│ waveform lane B (only when A/B on)│ TP · LRA · corr │ L R    │
├───────────────────────────────────┴──────────────────────────┤
│ playlist (virtualised rows)                                  │
│  #  ▸ name                       artist            A  3:24   │
├──────────────────────────────────────────────────────────────┤
│ transport: ⏮ ▶ ⏭ | 00:42 / 03:24 | vol | A|B  MATCH  BLIND  │ 64px
└──────────────────────────────────────────────────────────────┘
```

The playlist carries `# · title · artist · deck · time` and nothing else. A
per-row LUFS number and a format string were tried and removed: both are already
in the title bar for the loaded file, both were stale for every row that had not
been analysed yet, and eight tracks' worth of them turned a quiet list into a
spreadsheet. Loudness itself is untouched — the meter cluster and the persistent
analysis cache (§8) still need it.

There is **no right-hand column**: the metering is a compact cluster inside the
waveform region, to the right of the lanes, and it shares that region's height
so two stacked A/B lanes and the meters stay in one visual block. There is **no
standalone spectrum display** either — the 96-band analyser exists only as the
backdrop of the EQ curve (§12), which is also the only thing that turns the FFT
on.

The EQ is **not in this window at all**: it is a second, freely resizable window
(§12) with its own entry point. Nothing in the main layout moves when it opens.

**Minimum size is 420×560** (`tauri.conf.json`, and the same numbers in the
macOS override), narrow enough to park the window beside a DAW on one screen.
It degrades in deliberate steps rather than merely not overflowing:

| below | what changes |
|---|---|
| 1280px | transport and A/B rail tighten their gaps; the volume slider shortens |
| 1100px | the meter column narrows from 236px to 198px — no number shrinks |
| 960px | the meter cluster moves *under* the lanes, full width, and lays its read-outs out in a row; transport and rail may take a second line |
| 680px | title bar drops the format badge; the 1-sample nudges (`⌥,` / `⌥.`) leave the alignment bar, which is now two lines |
| 600px | transport becomes a deliberate two-row grid rather than a wrap lottery |
| 480px | playlist drops the row number and the artist column; title bar drops the LUFS read-out; the 100 ms nudges go, and the survivors get full-size targets |

Height has its own steps at 700 / 640 / 580 px, all of them lane-height caps, so
that two A/B lanes and the meter cluster still fit at 560 px tall.

Nothing is *only* hidden: every control removed at a breakpoint has a keyboard
route (`,` / `.` with `⇧` / `⌥` for the nudges) or lives one row down. Every hit
target stays ≥ 22px and no text goes below 8.5px at any width; `scripts/shots.mjs`
asserts both, plus zero overflow, at 1180 / 900 / 640 / 480 / 420 px.

### Keyboard map (must be implemented)

The single source of truth is `SHORTCUTS` in `src/lib/keys.ts`, which also
drives the `?` overlay. Keys are ignored while focus is in a text field.

Keys are also ignored **while an input method is composing** — `isComposing` on
the event, or the legacy `keyCode === 229`, both behind `src/lib/ime.ts`. A
candidate window owns `Space`, `Tab`, `Enter`, `Escape` and the digits until it
commits, and this app's map must not `preventDefault()` any of them while it
does: picking a Pinyin candidate with `Tab` in the theme editor used to tear the
composition down and drop the characters that were pending. The rule holds for
every handler that intercepts a key — the global map, the editor's textarea, the
accent field, the EQ and editor windows — with exactly one deliberate exception:
the §20.11 reset chord, a three-modifier combination that is not part of any
composition and is the way out of a theme nobody can read. `src/lib/layout.ts`
does not learn legends from composing events either, or a Pinyin session would
teach the overlay that `,` is printed “，”.
`scripts/shots-themedoc.mjs` drives a real composition (`compositionstart`, a
phonetic buffer with `isComposing`, `Tab`, `⌘Enter`, `compositionend`) and
asserts both halves: nothing is claimed while composing, and `Tab` still indents
when nothing is.

Matching is split, deliberately, and `src/lib/layout.ts` carries the argument:

* **Mnemonic keys match the character** (`event.key`): `L`, `M`, `E`, `G`, `O`,
  `S`, `P`, `X`, `Y` and `?`. `M` is mute on every layout, and that is the point
  of a mnemonic. `A` / `B` try the physical key (`KeyA` / `KeyB`) first and fall
  back to the character, so they land on the key legended A or B on a US layout
  *and* on the key in that position elsewhere.
* **Positional keys match the physical key** (`event.code`): `[ ] \ , . 1 2`,
  which are chosen for where they sit under the hand and are AltGr-only or
  unreachable on many non-US layouts. Binding them by position also makes them
  immune to a modifier rewriting the character — `⇧,` reports `<` and `⌥,`
  reports `≤`, which is what silently killed the 100 ms and one-sample nudges.

The overlay must show the *local* legend for the positional keys where the
platform can supply it (Keyboard Map API on Windows and Linux; WKWebView cannot,
so macOS falls back to the US legend).

| key | action |
|---|---|
| `Space` | play / pause |
| `←` / `→` | nudge ∓5 s (with `Shift` ∓1 s) |
| `↑` / `↓` | volume ±1 dB |
| `Home` | seek to 0 |
| `A` / `B` | **listen to** deck A / deck B — during an ABX test, slots A / B |
| `⇧A` / `⇧B` | **assign** the selected playlist row to deck A / deck B (B enables A/B) — §2.8 |
| `Tab` | toggle A/B deck (ignored during a blind test) |
| `X` / `Y` | blind slot switch (`X` only, in ABX) |
| `1` / `2` | blind vote — 2AFC: "X is A" / "Y is A"; ABX: "X = A" / "X = B" |
| `L` | loop on/off |
| `M` | mute |
| `E` | open the EQ window — or close it, from either window · `⇧E` EQ bypass, opening nothing |
| `⌘/Ctrl` + drag | EQ band-solo sweep (X = frequency, Y = Q) — §12 |
| `⌥` + click node | bypass that EQ band |
| `G` | level match on/off — §10 |
| `,` / `.` | nudge deck B earlier / later — 10 ms, `⇧` 100 ms, `⌥` 1 sample — §11 |
| `⌥` + drag lane B | slide the A/B time offset — §11 |
| `O` `S` `[` `]` `\` `P` | monitor mono · side · left · right · swap · flip-right; each key returns to stereo when its fold is already active — §6 |
| `Delete` / `Backspace` | remove the selected playlist row |
| `Esc` | close the top-most overlay — or, in the EQ window, that window |
| `⌘/Ctrl+O` | open files (replaces playlist) |
| `⌘/Ctrl+⇧+O` | add files (appends) |
| `⌘/Ctrl+K` | clear playlist |
| `?` | shortcut overlay |

## 5. Build targets

* macOS: universal (`aarch64-apple-darwin` + `x86_64-apple-darwin`), `.dmg` +
  `.app`, min system 11.0, category `public.app-category.music`.
* Windows: `x86_64-pc-windows-msvc`, NSIS installer.
* Bundle must register the audio file types so "Open with Onyx" works, and the
  app must handle `RunEvent::Opened` (macOS) plus `argv` (Windows/Linux). Every
  extension the installer claims must be one Onyx can actually open — including
  the v3 additions `mid`, `midi`, `zip`, `mov`, `mp4` and `opus` (§17–§19) — and
  the claim, the file dialog's filter and the drag-and-drop filter all read the
  same list.
* The converse does **not** hold, deliberately: `mkv`, `m4v`, `webm` and `adpcm`
  are decodable (§17) and accepted from a drop or the dialog, but Onyx does not
  claim them in Finder or Explorer. `mka` (Matroska *audio*) is claimed and
  `mkv`/`webm`/`m4v` are not, because a video container in the "Open with" menu
  of an audio player is noise for the user who owns those files, while `mov` and
  `mp4` are claimed for the picture-lock bounce a mastering engineer really does
  open. `adpcm` is not a container at all, only a codec-shaped extension, and
  nothing on either platform associates it. This asymmetry is intentional; the
  extension-by-extension list lives in `bundle.fileAssociations` and both build
  scripts print it at the end of a build.
* The General MIDI SoundFont of §18 ships **inside** the binary
  (`include_bytes!`, `crates/onyx-core/assets/gm/`), so there is no resource to
  resolve at runtime and no bank to lose on a partial install. Its licence text
  is bundled as a resource and recorded in `THIRD-PARTY.md`; the bundle grows by
  the size of the bank and that is expected.
* This is a cargo **workspace**: build output lands in `./target/<triple>/release`,
  *not* `src-tauri/target`. `scripts/build-mac.sh` and
  `scripts/build-windows.ps1` must reference the workspace target dir.
* `bundle.targets` is `["app", "dmg", "nsis", "deb"]`. The Linux `.deb` is a
  dev/CI smoke test rather than a shipping target, and it is what proves the
  bundle configuration still parses and that the file associations and the
  bundled bank really end up in a package.
* The front end builds **three** documents into one bundle: `index.html` (the
  player), `eq.html` (§12) and `theme.html` (§20). Each detached window is a
  real Vite entry point with its own capability file in
  `src-tauri/capabilities/`; a window added without one cannot even subscribe
  to an event, so the bundle check is "all three documents and all three
  capabilities ship", not "the app starts".

### 5.1 Security posture (non-negotiable)

* **The capability set is a whitelist, never `core:default`.** The renderer gets
  the three `core:event` verbs and only the window verbs the custom title bar
  actually calls. No `fs:`, `dialog:`, `opener:` or `shell:` permission is
  granted: those plugins are initialised for their **Rust** APIs only. Anything
  the front end needs must go through a `#[tauri::command]` in §3.1.
* **`tauri-plugin-fs` must not be registered.** Onyx reads files with `std::fs`
  from Rust; the webview has no filesystem surface to scope.
* **`app.security.csp` must be set, never `null`**, with `script-src 'self'`
  (no `unsafe-inline`, no `unsafe-eval`), `object-src`/`frame-src`/`child-src`/
  `worker-src 'none'`, `form-action 'none'`, `frame-ancestors 'none'` and
  `connect-src` limited to `'self'` plus the IPC origin. `freezePrototype` on,
  `assetProtocol` disabled with an empty scope. The dev-server CSP may be
  looser but must stay dev-only.
* **Commands that touch the OS validate their arguments.** `reveal_in_finder`
  reveals a path only if it is already an entry in the playlist; a rejected path
  is logged and returns `Err`.
* **Untrusted parsers are contained.** All `symphonia` entry points run inside
  `catch_unwind` (it panics rather than erroring on some malformed headers), the
  pre-allocated PCM buffer is bounded by what the file's size *could* decode to
  rather than by what its header claims, and non-finite or impossible header
  values are discarded before they cross IPC. A hostile-input corpus test is
  required, and a second one for archives (§19): zip slip, symlink entries,
  compression bombs, entry-count limits and malformed archives, driving the real
  extraction path rather than the guards in isolation.
* **`argv` is read as `OsString`** so a non-UTF-8 filename still opens, and a
  relative path is resolved against the launching instance's working directory
  before it is forwarded to the running instance.
* Window chrome is platform-specific: macOS keeps native decorations plus its
  traffic lights (`src-tauri/tauri.macos.conf.json`), Windows and Linux draw the
  custom title bar with `decorations: false`. The two config files must describe
  the *same* window in every other respect — Tauri's RFC 7386 merge replaces the
  array wholesale, so a field added to one and not the other silently disappears
  on macOS.
* **No `theme` is pinned on a window, in either config.** The frame follows the
  OS by default and the user's choice (§14, `system` included) is pushed to it at
  runtime. A hard-coded window theme would put a dark frame around a light
  window and would defeat `system` outright.
* **A `backgroundColor` *is* pinned on the main window, in both configs**, and is
  the *default* theme's surface (§14). This is not the same concession: the main
  window is created from the config before any Rust runs, so a literal is the only
  way it can be born with a colour rather than the system's grey, and the resolved
  colour is pushed to it from `setup` like every other window's. The literal must
  be the one `tokens.css` states for the dark theme, and a test must hold it there.

---

# v2 features

Sections §6–§12 are the v2 addendum, merged into this file. They describe
features that did not exist in v1 and the reasoning behind them; where they
touch something from §0–§5, §0–§5 has already been corrected to match.

---

## 6. Monitor matrix (mono / side solo / channel isolation)

A monitoring fold applied on the master bus, **after** the meter tap.

```rust
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MonitorMode {
    Stereo,     // L, R                      (default, bit-transparent)
    Mono,       // (L+R)/2 to both legs      (mono-compatibility check)
    Left,       // L to both legs
    Right,      // R to both legs
    Swap,       // R, L
    Side,       // (L-R)/2 to both legs      (side/difference solo)
    FlipRight,  // L, -R                     (polarity check)
}
```

Rules:

* `Stereo` **must** be a true no-op — do not multiply by 1.0, branch out of the
  function so the bit-transparent path is preserved. There is a test for this.
* `Mid` is deliberately **not** a mode: mid *is* `(L+R)/2`, i.e. exactly `Mono`.
  Do not add a redundant duplicate.
* The matrix sits after the meter tap on purpose: the LUFS / true-peak
  read-outs always describe the **programme**, not the monitoring fold, so
  flipping to `Side` for two seconds does not corrupt the integrated
  measurement. Document this in the README and put a one-line hint in the UI
  tooltip.
* Switching modes must not click: 5 ms equal-gain crossfade between the old and
  new matrix (reuse the existing envelope pattern, no allocation).
* `Side` and `FlipRight` are the two modes where a novice will think something
  is broken, so the UI must show a persistent, unmissable badge while any mode
  other than `Stereo` is active.

**Engine API:** `AudioEngine::set_monitor_mode(MonitorMode)`,
`AudioEngine::monitor_mode() -> MonitorMode`.

**IPC:** command `set_monitor_mode { mode: MonitorMode }` → `()`.
`TransportState` and `AppSnapshot.transport` gain `monitorMode: MonitorMode`.

**Keys:** `O` mono toggle, `S` side-solo toggle, `[` left only, `]` right only,
`\` swap, `P` polarity-flip right. Each key toggles back to `Stereo` when the
mode it selects is already active.

---

## 7. ABX (in addition to the existing 2AFC blind test)

`BlindState` gets a `mode` and grows a proper statistic. The existing AB test
stays exactly as it is; ABX is a second protocol.

```ts
type BlindMode = "ab" | "abx";

interface BlindTrialResult {
  trial: number;
  chose: string;          // slot the subject picked
  correctSlot: string;    // slot that was actually right
  correct: boolean;
}

interface BlindState {
  active: boolean;
  mode: BlindMode;
  trial: number;          // 1-based while active
  trials: number;
  slots: string[];        // ab: ["x","y"]   abx: ["a","b","x"]
  currentSlot: string;    // audible slot
  votes: BlindTrialResult[];
  score: number;
  finished: boolean;
  /** One-tailed exact binomial P(K >= score | p = 0.5). null until finished. */
  pValue: number | null;
  /** Both null while active; populated on reveal. */
  mapping: { x: Deck; y: Deck } | null;   // ab mode
  abxMapping: { x: Deck } | null;         // abx mode: which deck X was on the final trial
}
```

Protocol semantics:

* **`ab` (unchanged):** slots `x` / `y` map randomly to decks A / B. Question:
  *"which slot is deck A?"*. Vote records correctness.
* **`abx` (new):** slot `a` is always deck A, slot `b` is always deck B, and
  slot `x` is randomly one of them, re-randomised every trial. The subject may
  switch freely between `a`, `b` and `x`. Question: *"is X the same as A or as
  B?"* — so `blind_vote("a")` means "X is A". `correctSlot` is `"a"` or `"b"`.
* Mapping for the current trial must never be serialised while `active`.
* `pValue` = one-tailed exact binomial, `P(K >= score | n = votes.len(), p = 0.5)`
  = `sum_{k=score}^{n} C(n,k) / 2^n`. Compute in `f64` with a loop over
  log-gamma or an iterative binomial coefficient — no new dependency. Report
  `null` until `finished`.
* The reveal screen must state the result in plain language, e.g.
  *"11 / 12 correct, p = 0.003 — you can reliably hear a difference"* vs
  *"7 / 12 correct, p = 0.387 — no evidence you can hear a difference"*.
  Threshold for the positive wording: `p < 0.05`.

**IPC changes:**

* `blind_start { trials: usize, mode: BlindMode }` → `BlindState`
  (the `mode` argument is **new and required**).
* `blind_switch { slot: String }` → `BlindState` (slot validated against
  `slots`; invalid slot is an `Err`, not a panic).
* `blind_vote { slot: String }` → `BlindState`.
* `blind_abort` unchanged.

ABX requires both decks loaded; `blind_start` must return `Err` with a clear
message if either deck is empty (same for `ab`).

---

## 8. Persistent loudness cache

Purpose: integrated LUFS / LRA / true peak are known for files you have played
before, without playing them again — the meter cluster and the A/B level-match
trims are correct the moment a file loads. The playlist deliberately does not
show them as a column (§4); the cache exists for the meters and the trims, and
removing that column must not remove the measurement.

* Location: `app.path().app_cache_dir()` + `loudness-cache.json`.
* Key: everything the *decoded* output depends on, not just the file's bytes —
  `format!("{path}|{size}|{mtime_ms}|{rate}|d{semantics}|{render_key}")`:
  * `canonical_path`, `size_bytes`, `mtime_unix_millis` — size **and** mtime so an
    edited file is re-measured;
  * `rate` — the rate the decode actually ran at. With §9.6 follow-source-rate a
    file measured on a 48 kHz device and the same file measured on a 96 kHz one
    are two different measurements (different resampling, different true peak),
    so they are two different records;
  * `semantics` — `onyx_core::decode::DECODE_SEMANTICS`, a version for *how* a
    file is turned into samples: Opus pre-skip, AAC/MP4 edit-list priming, length
    bounds, channel folding, the resampler, dither, and the loudness / true-peak
    maths. **Bump it in the same commit that changes any of them**, or every user
    who has played a file keeps its old numbers for ever. The constant's own doc
    comment carries the list, and `decode::tests` fails if the behaviours it
    fingerprints move without it;
  * `render_key` — the SoundFont for a MIDI render (§18), so a bank change does
    not hand back the old bank's measurement.

  Hash the key (FNV-1a or `DefaultHasher`) to keep the file small; store the
  readable path alongside for debuggability.
* Value: `{ integratedLufs, lra, truePeakDb, samplePeakDb, lastUsedUnix, schema }`.
* `schema: u32` — bump it whenever the measurement or the *key* changes so stale
  entries are discarded instead of silently trusted. Currently `2` (`1` was keyed
  without the rate or the decode semantics).
* Bounded: max 5 000 entries; evict least-recently-used on insert.
* Write behaviour: **debounced** (≥2 s) and atomic (write `*.tmp`, then rename).
  Never write from the audio or decode thread. Never block a command on disk.
* Read behaviour: loaded once at startup; a corrupt or unparseable file is
  discarded with a warning, never fatal.
* Wire-up:
  * on playlist add / probe → look the entry up and populate
    `PlaylistEntry.analysis` immediately,
  * on decode completion → insert / refresh,
  * cache hits must also feed the A/B level-match trims (see §10), so that when
    level matching is enabled, switching to a previously-played file is matched
    instantly instead of after a decode,
* Waveform peaks are **not** cached (they are cheap to regenerate and would
  dominate the file size). Say so in the README rather than leaving it implicit.

**IPC:** `cache_stats()` → `{ entries: number; bytes: number; path: string }`,
`cache_clear()` → `{...same...}`. Surfaced in `SettingsPanel`.

---

## 9. Audit requirements

Both layers must be swept for, and fixed:

1. **Stale paths / dead files** — leftover starter files (`src/assets/*`,
   `public/tauri.svg`, `public/vite.svg` if unreferenced), unused icon sets,
   unreferenced CSS, dead exports, `dist/` build output committed by accident,
   `.vscode/` from the scaffold, references in docs to files that no longer
   exist, and the `artifacts/` + `dist/` directories at the project root (these
   are build output — make sure `.gitignore` covers them).
2. **Redundancies** — duplicated type definitions between Rust and TS that have
   drifted, duplicated formatting helpers, two ways of doing the same IPC call,
   copy-pasted canvas setup code, the `Mono`/`Mid` class of duplication.
3. **Error logic** — every `unwrap`/`expect`/`panic!` on a path reachable from
   user input, swallowed `Result`s (`let _ = ...` where the error matters),
   `catch {}` blocks in TS that hide failures from the user, commands that
   return `Ok` after a partial failure, and any place a failure leaves the
   engine in an inconsistent state (half-loaded deck, orphaned decode thread).
4. **Real-time safety regressions** — the allocation-counting tests must still
   pass after the monitor matrix is added.
5. **Concurrency** — lock ordering between the playlist mutex, the deck slots and
   the engine's internal mutexes; any path that holds a lock across an `emit` or
   across a blocking call.
6. **Off-by-one / boundary** — seek at exactly the end, zero-length files,
   single-frame files, playlist operations on an empty playlist, `playlist_move`
   with out-of-range indices, waveform `from` greater than `count`.

---

## 10. Level matching is opt-in, not automatic

v1 applied loudness-matching trims to the A/B decks automatically. That is
wrong: a mastering engineer switching between two versions must be able to trust
that what they hear is bit-transparent unless they asked for it otherwise.

* New state `levelMatch.enabled`, **default `false`**.
* While disabled, both deck trims are exactly unity. Not "0.0 dB converted to a
  gain of 1.0 and multiplied" — the gain stage must early-out so the signal path
  is untouched. There is a bit-transparency test for this.
* While enabled, trims are derived from the integrated-LUFS difference between
  the two decks and only ever **attenuate**: the louder deck is brought down to
  the quieter deck's loudness, the quieter deck stays at unity. Never boost —
  boosting risks clipping and true-peak overs on material that is already hot.
* If it is switched on before analysis is available (decode still running, no
  cache entry), report `ready: false`, apply unity, and apply the real trims
  automatically as soon as the measurement lands. Do not silently behave as if
  matched.
* Enabling / disabling must not click — reuse the existing trim glide.

```ts
interface LevelMatchState {
  enabled: boolean;
  ready: boolean;      // false = measurement pending, trims are unity
  trimDbA: number;     // <= 0
  trimDbB: number;     // <= 0
}
```

**IPC:** `set_level_match { enabled: boolean }` → `()`.

**UI:** a toggle in the A/B section, off by default, that states the applied
trim when active (e.g. `MATCHED · −1.8 dB on B`) and shows `MATCHED · pending`
while `ready` is false. Key: `G`.

---

## 11. A/B time alignment

Two versions of the same piece frequently do not start at the same offset (a
different amount of head silence, a different bounce region). Comparing them at
the same playhead is then meaningless. Onyx needs both one-tap automatic
alignment and manual control.

### Offset model

A single signed offset, deck B relative to deck A:

* `ab_offset_frames: i64`, at the engine sample rate.
* Deck B reads at `playhead + offset`. Positive offset therefore means B's
  content sits *later* in its own file (more head silence) and must be advanced
  to line up with A.
* Clamp to ±30 s worth of frames.
* Where the offset pushes B's read position before zero or past its end, deck B
  outputs **silence**. Do not clamp to the first/last frame — a held sample or a
  repeated start is a lie about the material.
* The offset applies to playback, meters, and both waveform lanes. Loop bounds
  and the displayed timeline stay defined on **deck A's** timeline; A is the
  reference.
* Alignment is offset-only. Onyx does not time-stretch, and does not attempt to
  align versions with different tempo or edit structure — say so in the README
  instead of failing mysteriously.

**IPC:** `set_ab_offset { frames: number }` → `()`.
Add `abOffsetFrames: number` to the A/B state in `AppSnapshot`.

A drag moves the offset at pointer rate, so the front end (`src/lib/align.ts`)
holds it optimistically and writes it **one call per animation frame and one call
at a time**: a queued write is replaced by the newest value rather than added to,
and the next one does not leave until the last has been acked. Coalescing per
frame alone is not enough — on a machine where a write outlives a frame (any
machine, while the engine is re-decoding a deck) two writes would be in flight at
once, the engine may ack them in either order, and the "last confirmed" offset a
rejection rolls back to would then be a superseded one. `scripts/check-ab-parity.mjs`
drives a drag through the real module and asserts the writes are ordered, never
overlap, and end on the value the drag ended on.

### One-tap auto-align

`auto_align_ab()` → `AlignResult`:

```ts
interface AlignResult {
  offsetFrames: number;
  offsetMs: number;
  confidence: number;        // 0..1, correlation peak scaled by its distinctness
  polarityInverted: boolean; // best match was at negative correlation
  applied: boolean;          // false when confidence was too low to trust
}
```

Two-stage estimation, on a worker thread — never on the audio thread, and never
blocking the command longer than it takes to hand off:

1. **Coarse.** Mono-sum both decks, take a short-window energy envelope
   (~2 ms hop), FFT cross-correlate over up to the first 60 s. Envelope
   correlation is what makes this robust to two masters that differ in EQ and
   compression.
2. **Fine.** Normalised cross-correlation at full rate within ±50 ms of the
   coarse peak, over a high-energy segment rather than the first thing found —
   quiet intros give a confident-looking peak on noise. This is what gets the
   result to sample accuracy, which matters because engineers will use this to
   null two versions against each other.

Rules:

* Requires both decks loaded with at least ~5 s of decoded audio; otherwise
  return `Err` with a message telling the user to wait for decode, not a bogus
  offset.
* `confidence < 0.3` → do **not** apply, return `applied: false`, and have the
  UI say plainly that it could not find a confident alignment and that manual
  alignment is available. A wrong automatic offset is worse than none.
* **Confidence must measure distinctness, not just peak height.** A bare
  normalised peak is useless on the material this feature is used with: a
  perfect loop, or a steady beat, correlates at ~1.0 one bar out as readily as
  at the truth. So the reported figure is the winning peak scaled down by how
  close the runner-up candidate at a *different* offset comes to it — within
  2 % and the audio simply does not say which is right, so the confidence
  collapses and the offset is refused. Required behaviour, covered by
  `crates/onyx-core/tests/align_adversarial.rs`: exact resolution on plain
  offsets (including through EQ, compression and noise), and honest **refusal**
  — not a guess — on a perfect loop, on a ±2 % tempo/tape-speed difference and
  on unrelated material; on a different edit it must either refuse or align one
  section honestly, never report a confident wrong offset.
* If the best correlation peak is negative, set `polarityInverted: true` and
  surface it — two versions differing only in polarity is a real and confusing
  situation. Offer to invert: `set_deck_invert { deck: Deck, invert: boolean }`
  → `()`, exposed as a small `ø` toggle per deck.
* Re-running auto-align must be idempotent: aligning twice must not drift.
  Estimate against the raw files, not against the currently offset positions.

### Manual alignment

* Nudge controls in an `ALIGN` group: `±1 sample`, `±1 ms`, `±10 ms`, `±100 ms`,
  plus `RESET` to zero and the auto-align button.
* Read-out shows the offset in both ms (2 dp) and samples, signed, and which
  deck is being moved.
* Keys: `,` nudge B earlier, `.` nudge B later; plain = 10 ms,
  `Shift` = 100 ms, `Alt` = 1 sample. `Cmd/Ctrl+,` is not to be used, it
  conflicts with settings conventions.
* Direct manipulation: holding `Alt` and dragging horizontally on waveform lane
  B slides the offset instead of seeking, with a live ms read-out following the
  cursor. This is the interaction most users will reach for, so it must feel
  smooth — update the offset optimistically at pointer rate and let the engine
  glide to it.
* When the offset is non-zero, lane B draws its waveform shifted by `-offset` so
  that the two lanes correspond vertically and the shared playhead line is
  visually truthful. Regions where B has no material are drawn flat and dimmed
  rather than blank, so it is obvious *why* there is silence.
* A non-zero offset needs a persistent, quiet badge — like the monitor matrix,
  this is state a user can forget they left on.

---

## 12. EQ — interactive spectrum curve (replaces the v1 8-band strip)

The fixed 8-band strip is removed. In its place: a Pro-Q-style interactive
spectrum display with a dynamic number of bands, direct manipulation, and
band-solo auditioning. **It is its own window, closed by default, and opens with
one tap** (`E`, plus the EQ button in the title bar).

### A window, not a panel

An EQ curve wants to be as large as the screen allows and to sit beside the
material it is shaping, not on top of it. As a drawer it either cramped the
waveforms or covered them, and on a second monitor it could not be moved at all.
So it is a real window.

* A Tauri `WebviewWindow` (`eq.html` → `src/eq/main.tsx`), 940×560 on first run,
  resizable, minimum 620×360. Vite builds it as a second Rollup entry.
* **Rust owns its lifecycle** (`src-tauri/src/eqwindow.rs`): create, focus,
  close, and the `onyx://eq-window` broadcast both windows listen to. The
  renderer has no window-creation permission — `capabilities/eq.json` grants the
  EQ window event listen/unlisten/emit and nothing else.
* Opening while it is already open **focuses** it. There is never a second one.
* **EQ state lives in the engine**, not in either document. The window is a view:
  close it mid-phrase and the bands, the bypass and the audio are untouched; open
  it again and the curve is exactly where it was. Two windows cannot disagree
  about a filter because neither of them owns it.
* Whether it is open, and whether it is pinned above other windows, persist in
  `settings.json` and are restored on launch. Pinning is a preference (`Float` /
  `Behind` in its header), not a law: on a second monitor always-on-top buys
  nothing.
* Closing the main window closes the EQ window with it.
* The EQ window carries a **reduced keyboard map** — `Space` (play/pause), `E`
  and `Esc` (close it), `⇧E` (bypass). Transport keys that would need the
  playlist or the lanes are deliberately absent; the main window has them.
* Indicators that must be seen from the main window — band solo above all — ride
  the 60 Hz `FramePayload`, because a JavaScript ref in one webview is invisible
  to the other.

### Band model

Dynamic list, 0 to 16 bands. Zero bands is the normal resting state.

```rust
pub enum FilterKind { Bell, LowShelf, HighShelf, HighPass, LowPass, Notch, BandPass }

pub struct EqBand {
    pub id: u32,
    pub enabled: bool,
    pub kind: FilterKind,
    pub freq_hz: f32,     // 20 .. 20_000, clamped to < nyquist * 0.49
    pub gain_db: f32,     // -30 .. +30  (ignored by HP/LP/Notch/BandPass)
    pub q: f32,           // 0.1 .. 40
    pub slope_db_oct: u8, // HP/LP only: 12 | 24 | 48, cascaded biquads
}

pub struct EqConfig { pub enabled: bool, pub bands: Vec<EqBand> }
```

* **Every shape is the RBJ cookbook prototype**, designed at `f64` and run at
  `f64`. `q` on a shelf is the cookbook **shelf Q** (the `2·√A·α` term), not an
  `S`/slope parameter, over the whole advertised range; `q = 1/√2` is the
  steepest monotonic shelf and above that a shelf resonates, which is the
  behaviour engineers expect from a Pro-Q-style equaliser. Design-time `q` is
  clamped at 0.05, below the 0.1 the band model exposes.
* **HP/LP slopes are cascaded Butterworth sections**: 12 / 24 / 48 dB/oct is
  1 / 2 / 4 biquads with the Butterworth Q ladder, and the band's own `q` scales
  the outermost section only, so the knob still shapes the corner at every
  slope. The realised slope must be *measured* an octave out, not assumed.
* `MAX_BANDS = 16`. The realtime side must pre-allocate for `MAX_BANDS` and
  receive config changes through the existing lock-free command path using a
  **fixed-size inline array**, never a `Vec` that would be allocated or dropped
  on the audio thread. The allocation-counting test must cover adding and
  removing bands while playing.
* `enabled: false` or an empty band list must early-out to the bit-transparent
  path, not run unity-gain filters. Bit-transparency test required.
* Adding, removing or retyping a band must not click: crossfade the filter
  cascade over ~5 ms, as the v1 EQ already does for coefficient changes.
* The **frontend owns the band list** and always sends the complete config via
  `set_eq { config }`. Do not add per-band IPC commands — one authoritative
  setter avoids the two-sources-of-truth bug class.

### Display

* Log frequency axis 20 Hz – 20 kHz, gain axis ±18 dB with the grid labelled;
  bands sitting beyond ±18 dB stay draggable, the axis does not rescale under
  the cursor.
* Real-time FFT analyser drawn *behind* the curve: dim, translucent, champagne
  at low opacity, with a slow peak-hold trace that decays. It must read as a
  backdrop, not compete with the curve.
* Per-band response curves drawn thin and dim; the composite response drawn as
  the one bright champagne line with a soft fill to the 0 dB axis. The composite
  is computed from the actual biquad coefficients — do not approximate it with a
  parametric sketch, because a curve that disagrees with what you hear is worse
  than no curve.
* **The curve is computed twice, in two languages, so the agreement must be
  enforced mechanically.** The engine designs the filters in Rust; the EQ window
  draws them in TypeScript (`src/lib/eq.ts`). Every semantic detail — shelf-Q
  form, the 0.05 design clamp, the Butterworth cascade and where the band's `q`
  is applied, and the magnitude/dB conversion including its underflow floor —
  must match. Required: a checked-in fixture of the engine's response over a
  spread of kinds, frequencies, Q values, gains and all three slopes
  (`crates/onyx-core/tests/eq_curve_contract.rs`), and a check that runs the
  **real** TypeScript source against it (`npm run check:eq`, wired into
  `npm run build` and `npm run build:mock`). Tolerance ≤ 10⁻³ dB. Neither side
  may be changed without the other failing.
* Band nodes: small champagne handles, labelled with frequency on hover/drag,
  showing note name alongside the frequency (e.g. `440 Hz · A4`) since this is a
  music tool.
* The analyser runs only while the EQ **window** is open — the window's lifetime
  drives `set_spectrum_enabled { enabled }`, so a closed EQ costs zero FFT. That
  includes closing it from the OS title bar, not just from the app.

### Interaction

* Click empty graph area → create a `Bell` at that frequency and gain.
* Drag node → frequency on X, gain on Y. `Shift` constrains to gain only,
  `Alt` to frequency only.
* Wheel / trackpad scroll over a node → `Q`. `Shift`+wheel → fine.
* Double-click node → delete. Click node's small dot → bypass that band.
* Right-click node → filter-type menu, and slope choice for HP/LP.
* Drag on the axis background with no node under the cursor must **not** create a
  band accidentally on a click that was really a pan — require a real click, and
  treat drags beginning on empty space beyond a small threshold as a marquee-free
  no-op.

### Band solo / audition sweep — the reason this window exists

This is what "drag to choose the frequency range you want to hear" means, and it
must feel immediate.

* Hold `Cmd`/`Ctrl` and drag anywhere on the graph → a narrow bandpass follows
  the cursor. X sets centre frequency, Y sets `Q` (higher = narrower,
  roughly `Q` 2 at the bottom to `Q` 24 at the top). Release → back to normal
  monitoring. This is how an engineer hunts a resonance.
* Each band also gets a hold-to-solo control that auditions **that band's**
  frequency region using the band's own `freq` and `Q`.
* Implemented in the engine as a dedicated audition bandpass on the master bus,
  after the EQ, with the same 5 ms crossfade on entry and exit so sweeping does
  not zipper. Sweeping must be allocation-free.
* **IPC:** `set_eq_audition { freq_hz: number | null, q: number }` → `()`.
  `null` frequency means audition off.
* While auditioning, show an unmistakable indicator — this is a monitoring state
  the user must never mistake for the actual signal, same reasoning as the
  monitor matrix badge in §6. It must appear in **both** windows: the sweep is
  started in the EQ window, but the engineer is looking at the waveforms.

### Keys

`E` opens the EQ window, or closes it if it is already open (from either
window) · `Shift+E` EQ bypass without opening anything · `Cmd/Ctrl+drag` solo
sweep · `Alt+click` node bypass · `Esc` in the EQ window closes it. Update the
shortcut overlay so the map stays truthful.

`Shift` and `Alt` are constraint modifiers **inside the EQ graph only**
(gain-only and frequency-only drags). On the waveform lanes they mean loop
selection and A/B alignment (§11). The two never meet: they are different
windows, and the EQ graph swallows the events it uses.

### Persistence

EQ config, monitor mode, level-match toggle, and the EQ window's open and
pinned state persist across
launches in a `settings.json` next to the loudness cache of §8, reusing the same
debounced atomic-write helper. Corrupt settings → fall back to defaults with a
warning, never a crash, and never a silent reset that loses a user's curve
without telling them.

> **Implementation note (deliberate deviation).** "Next to" is read as "through
> the same writer", not "in the same directory": `settings.json` lives in
> `app_config_dir()` while `loudness-cache.json` lives in `app_cache_dir()`. The
> cache is regenerable measurement data that a backup or sync client should be
> free to drop; a user's EQ curve is not. Both still go through the one
> `persist::AtomicWriter`.

---

## 13. Diagnostics: logging and real-time fault reporting

One sink, or the diagnostic is lost. A packaged build has no terminal and no
devtools, so a message that is only printed has been deleted with extra steps.

### Logger

`tauri-plugin-log`, installed once at startup, with exactly two kinds of target:

- a rotating file in the platform log directory, stem `onyx` — 4 MiB
  (`max_file_size`), `RotationStrategy::KeepSome(2)`. Always present. This is the
  file a user is asked to attach.
- stderr, **only** under `cfg!(debug_assertions)`. A shipped app is not attached
  to a terminal, so it must not pay to format for one.

Timestamps are UTC (`TimezoneStrategy::UseUtc`): a log is read next to a
wall-clock report from a different machine, and local time without an offset is
ambiguous.

### Level

Default **`warn`**. A player must not write to a user's disk for ever, and
everything above `warn` is something a user can reasonably be asked about.
`ONYX_LOG` overrides it without a rebuild — `off|error|warn|info|debug|trace`,
case-insensitive, surrounding whitespace ignored. An unusable value is **not** a
startup failure: fall back to `warn` and report it once, after the logger exists.
`info` must be enough to follow the lifecycle (device, rate, loads, cache state).
A load logs the **file name only, never the directory**, so the file stays safe
to attach; full paths are `debug`.

### Real-time faults are counted, never logged

`log::` on the audio thread formats, may allocate and may block on a file, all
three of which §2 and the allocation-counting tests forbid. Therefore:

1. the engine only **increments atomics** — dropped engine commands (the bounded
   RT command queue was full, so a control change never reached the callback) and
   stream faults plus the last fault kind — drained through
   `AudioEngine::take_faults() -> RtFaults`, a swap-to-zero read.
2. the 60 Hz frame thread is the **only** caller of `take_faults`. It drains
   every 250 ms and **coalesces**: the first fault after a quiet spell is
   reported immediately, then at most one line every 5 s carrying the count and
   the window it covers. A permanently failing device must not be able to fill a
   disk or an event loop.
3. every such line names the current output device and engine rate, because
   "audio dropped out" without them is not actionable.
4. a fault only the user can fix (output device gone) additionally raises **one
   toast per episode**, not per occurrence. A backend hiccup already visible in
   the transport's underrun counter is a log line and nothing more.

### Webview diagnostics

`src/lib/log.ts` is the only place in the front end allowed to report a failure
that is not already visible; `console.*` in app code is banned outside
`vite dev`. Anything the user must act on stays a toast.

It emits `onyx://client-log` with `{ level, message, detail }` — an *event*, not
a command, because §3.1 fixes the command surface and a log line must never fail
loudly or block. Uncaught errors, unhandled rejections and the React error
boundary all funnel through it.

The Rust listener treats the payload as untrusted input from a webview:

- an object with a non-empty string `message`, or the record is dropped (at
  `debug`, so a broken UI cannot spam the file complaining about itself);
- control characters folded to spaces — a `\n` would forge a second log line;
- `message` and `detail` truncated by **characters**, never bytes, so a
  multi-byte sequence is never split;
- a budget per window bounds what a runaway render loop can write, and reports
  how many records it dropped instead of writing them;
- every line carries the `webview` target, so the origin of a line is never
  ambiguous.

Nothing in the diagnostic path may throw, block, or report its own failure
through itself.

---

# v3 features

Sections §14–§19 are the v3 addendum, merged into this file. They describe
features that did not exist in v2 and the reasoning behind them; where they
touch something from §0–§5, §0–§5 has already been corrected to match. The
addendum numbered these §13–§18; §13 was already the diagnostics contract, so
they are shifted by one here and the numbering of everything before them is
untouched.

---

## 14. Theming

Two designed themes, not one theme with inverted colours.

* `theme: "dark" | "light" | "system"`, default `"dark"`. `"system"` follows the
  OS and must react live when the OS switches, without a restart.
* **Dark** is the existing obsidian/champagne identity — unchanged. Do not
  "improve" it while adding the light one.
* **Light** must be designed with the same intent: warm off-white / alabaster
  paper rather than pure `#fff`, bronze or aged-brass accents in place of
  champagne, and enough contrast on canvas elements (waveform bars, meters, EQ
  curve) that they read as crisp on a light field. A light theme that is just
  the dark one with a flipped background will look cheap and is not acceptable.
* Every colour must come from the token layer (`src/styles/tokens.css`).
  Hard-coded colours in components are a bug — canvas code included, which
  cannot read CSS variables implicitly and must resolve them explicitly on
  theme change and on first paint.
* The EQ window (§12) is a separate webview and must follow the theme too,
  including when the theme changes while it is open.
* The **native window frame** is not the webview's to paint: the macOS traffic
  lights and their title bar, the Windows caption and the scrollbars belong to
  the window manager. Neither `tauri.conf.json` nor `tauri.macos.conf.json` may
  pin a window `theme` (see §5) — the frame follows the OS unless the user chose
  otherwise, and the choice is applied to every window at startup and on change.
  `system` means "no override", not "dark".
* The **window surface** — the colour the window manager paints *under* the
  webview — is not the webview's either, and must be set. An untold window keeps
  the system's own background (light grey on macOS), which shows around the
  antialiased rounded corners of a decorated window and for the frames before the
  webview's first paint: a pale rim around a dark app. The surface is the
  **resolved** theme's `--ink-900`, never a hard-coded colour — a fixed dark value
  would put an obsidian rim around the light theme, and a theme document (§20)
  may move that token anywhere, so the window that wears the document reports the
  colour it is really painting through `set_window_surface` and the app layer
  paints the windows with it. Required of all three windows, at creation and on
  every change of theme, of OS appearance under `system`, of document, and on
  reset; the theme editor's window takes the *designed* colour rather than the
  document's, for the same reason its webview ignores the document (§20.10). A
  test must tie the window surface to the resolved theme's background so the two
  cannot drift. What is explicitly **not** required is removing AppKit's own 1 px
  stroke around a decorated window: that would mean `decorations: false`, and the
  traffic lights and native edge-resize are worth more than a hairline.
* Every window-level listener the appearance module installs — `resize` for the
  zoom clamp, `storage` for the cross-window relay, the capture-phase reset chord
  — is **owned and removable**: `initAppearance()` installs one set (a second call
  does not double up), `teardownAppearance()` hands all of them back, and a Vite
  hot update disposes of them. Three windows come and go here; a listener nobody
  kept a handle to keeps a dead document's closure, and its stale appearance,
  alive. `scripts/shots-theme.mjs` counts them through CDP's listener table and
  checks that a torn-down module has really gone deaf.

## 15. User customisation

In `SettingsPanel`, persisted in `settings.json` (§12), applied live:

* **Accent colour** — a small curated set of tasteful presets plus a free hex
  entry. Validate the hex, reject unparseable input visibly rather than
  silently falling back. Derive hover/active/dim variants programmatically from
  the chosen accent so a custom accent doesn't break states that were
  hand-tuned for champagne.
* **UI font** — a short curated list that actually suits the product, plus the
  system UI font. Do not offer a font the app cannot render offline.
* **Numeric font** — the read-outs (LUFS, timecode, dB, sample counts) are
  tabular and must stay monospaced and column-stable. Offer a small list of
  monospaced faces; reject proportional fonts for this slot.
* **Size scale** — `compact | normal | large`, driving a root scale factor.
  Must not break the 420 px minimum layout (§4 *Layout*) at `large`.
* **Reset to defaults**, and a visible indication when settings differ from
  default.

Bundle any non-system font with the app; never fetch a webfont at runtime —
this is an offline desktop tool and the CSP forbids remote origins.

These six controls are the *simple* face of appearance. Everything else about
the look — every token in `tokens.css` — is reachable as a document, §20, and a
document in force overrides whatever of these it states.

## 16. Playback engine source

Expose what the engine already knows, in Settings:

* **Host / audio API** — enumerate cpal hosts (CoreAudio on macOS; WASAPI, and
  ASIO where available, on Windows). Changing it re-opens the stream.
* **Output device** — enumerate devices for the chosen host, plus a "system
  default" entry that follows the OS default when it changes.
* **Sample rate** — `follow source` (default, preserves the bit-transparent
  path) or a fixed rate chosen from those the device reports.
* **Buffer size** — offer the device's supported range; show the resulting
  latency in ms, because that is the number the user actually cares about.
* Device changes must be **safe while playing**: rebuild the stream, restore
  position and transport state, and never panic if the device disappears
  mid-playback — that already has a fault path, so reuse it.
* A rate change is **atomic as far as the listener is concerned**: no deck may be
  audible while its PCM is stored at a rate the engine no longer runs at. PCM
  decoded at 44.1 kHz on a device re-opened at 96 kHz is not a glitch, it is a
  different record — a fifth-and-a-bit sharp and 2.18× too fast — and in a
  mastering tool that is worse than silence. So a deck whose rate no longer
  matches is taken off the audio callback *before* the re-decode that fixes it,
  stays loaded (file, trim and playlist row intact) while it is quiet, and comes
  back when it agrees again. This holds on the failure paths too: a load that
  re-clocks the device and then fails unwinds the rate, and if the device refuses
  to go back, the decks stay silent and are re-decoded at whatever rate the
  device ended up on — with an error the user can read, never wrong-pitch audio.
  `loader::rate_change_plan` is that decision as a pure function, so the
  device-failure sequences no test with an output device could reach are unit
  tested; the invariant it is tested against is the sentence in bold above.
* If a chosen device is gone at next launch, fall back to system default and
  tell the user why rather than failing to start.
* Persist selection in `settings.json`.

## 17. Format coverage

Must decode: **WAV/BWF and raw PCM (8–32 bit integer, float, A-law/µ-law and
IMA-ADPCM), FLAC, MP3, AAC (.m4a/.aac), ALAC, Ogg Vorbis, Opus, AIFF/AIFC, CAF,
Matroska (.mka/.mkv) and WebM**, and the **audio track of MOV and MP4**
containers. Enable the corresponding Symphonia features rather than assuming
defaults — the enabled set is `crates/onyx-core/Cargo.toml`
`[dependencies.symphonia]`, and Opus is decoded by our own decoder because
Symphonia demuxes Ogg/Opus but ships no Opus decoder.

* Container and codec are detected by content, not by extension — a `.wav` that
  is really an MP3 must still play, and a wrong extension must never panic.
* Video containers: take the first audio track, ignore video streams entirely,
  and report the container plus codec honestly in the UI (`MP4 · AAC`).
* Decoding a format and *claiming its extension in Finder / Explorer* are two
  different decisions: the accepted-extension filter is
  `decode::SUPPORTED_EXTENSIONS`, the associations are
  `bundle.fileAssociations`, and the associations are deliberately the smaller
  set (§5).
* Add a decode test per format against generated or checked-in short fixtures,
  asserting sample rate, channel count and duration — the existing malformed
  input corpus covers hostile files, this covers the happy path per format.

## 18. MIDI playback (General MIDI)

`.mid` / `.midi` files play back rendered through a General MIDI synthesiser.

* Use a pure-Rust SoundFont synth (`rustysynth` or equivalent) with a **bundled,
  redistributably-licensed GM SoundFont**. Check the licence and record it in
  `README.md` and a `THIRD-PARTY` note. Prefer a compact bank; do not bloat the
  bundle with a 100 MB orchestral set.
* Users may point at their own `.sf2` in Settings; invalid or missing files fall
  back to the bundled bank with a clear message.
* Render to PCM at the engine rate on the decode thread, then treat it exactly
  like any other decoded source — it then inherits waveform, seeking, loudness
  analysis, A/B, EQ and metering for free. Do not build a parallel playback
  path.
* Honour tempo maps and multi-track files. Report duration accurately, including
  the release tail rather than cutting the last note dead.
* Loudness analysis of a synthesised render is legitimate — it measures what you
  hear — but the file itself has no inherent loudness, so make sure the cache
  key (§8) accounts for the SoundFont in use, or a bank change will serve a
  stale measurement.
* Show format as `MIDI · GM` with the bank name.

## 19. Zip archives as playlists

Opening or dropping a `.zip` loads its audio contents as a playlist.

* Follows the existing open/drop rule: **open replaces the playlist and plays the
  first track; drop appends.**
* Entries are added in natural sort order (`track2` before `track10`), walking
  nested directories, skipping non-audio entries silently and anything that is
  not decodable with a single summary warning rather than one per file.
* **Security — this is untrusted input.** Guard zip-slip (entries that escape the
  extraction root via `..` or absolute paths), symlink entries, and zip bombs:
  cap total uncompressed size and entry count, and refuse with a clear message
  rather than filling the disk. Do not recurse into nested zips.
* Extract to a temp directory so seeking and re-decode work normally; clean it
  up on exit and on playlist clear. A crash must not leave the temp tree behind
  forever — prefer a location the OS reclaims, or clean stale dirs at startup.
* The playlist must show which archive an entry came from, and an empty or
  audio-free archive must say so plainly instead of silently doing nothing.

---

# v4 feature

Section §20 is the v4 addendum, merged into this file. It describes a feature
that did not exist in v3 and the reasoning behind it; where it touches
something from §0–§5, §0–§5 has already been corrected to match.

---

## 20. The theme document

§15 lets a user pick six things. §20 lets them change everything, by handing
the appearance to a language model as text and pasting the answer back.

The workflow, and the only one that matters:

> **Copy** the current look out of Settings → **paste** it into a chat with a
> sentence ("make it a cold graphite studio look") → **paste** the reply back
> → **Apply**, and the app is wearing it.

Everything below exists to make that round trip work on the first attempt and
to make it safe when it does not.

### 20.1 What a theme document is

A **theme document** is text: JSON with `//` comments, trailing commas, single
quotes and bare keys tolerated, because that is what a model returns. It is
**data, never CSS and never code**. No part of it is ever concatenated into a
stylesheet, and it cannot name anything outside the token layer.

```jsonc
{
  "onyx": "theme",      // the marker; its absence is a warning, not an error
  "version": 1,         // the document format's version, not the app's
  "name": "Cold Graphite",
  "appearance": { … },  // the §15 settings, all optional
  "base":  { … },       // tokens that are the same in both themes
  "dark":  { … },       // the dark theme's tokens
  "light": { … }        // the light theme's tokens
}
```

* Any top-level key other than those seven is an **error**, with a line number
  and a "did you mean".
* Every block is optional and **partial**: a document that states four tokens
  is a valid document. What it does not state keeps the designed value, which
  is why a theme need not be complete to be applied and why reverting is
  removing properties rather than restoring them.
* `version` greater than the build's is an error ("this build understands 1"),
  never a best-effort read. A missing `version` or marker is read as the
  current version with a warning.

### 20.2 The token catalogue is generated, not written

The set of legal keys is **derived at runtime from `src/styles/tokens.css`**
(`src/lib/tokens.ts`, via Vite's `?raw`). Hand-listing the defaults would let
the export drift from the stylesheet the first time a colour was nudged, and an
export that lies about the current look breaks the whole feature quietly. A
token added to `tokens.css` is therefore in the contract immediately; what is
hand-maintained is only the prose a machine cannot infer — group, one-line
description, numeric range.

* `scope: base` tokens are stated once, in `"base"`; `scope: theme` tokens are
  stated per theme, in `"dark"` / `"light"`. Putting one in the wrong block is
  an error that says which block it belongs in — not a silent no-op.
* The **derived** tokens (the `--accent*` family, `--deck-b*`, `--zoom`) are
  computed by the runtime from `appearance.accent` and the size scale. They are
  *legal* keys — a theme that wants to hand-tune the accent ramp may state them
  and wins — but they are **excluded from the export**, because a copied
  default that pinned them would freeze the accent picker, and a dead control
  is exactly the "it feels broken" failure this feature must not have. Stating
  one produces a warning that says the picker will no longer move it.

### 20.3 Values are typed, re-emitted, and bounded

Every value is tokenised, checked against a whitelist, converted to typed data
and **re-emitted from that data** (`src/lib/cssvalue.ts`). The only CSS the app
applies is CSS that module wrote. The types are the whole vocabulary:

| type | accepted |
|---|---|
| `color` | `#rgb`/`#rgba`/`#rrggbb`/`#rrggbbaa`, a CSS colour name, `rgb()/rgba()`, `hsl()`, `oklch()`, `transparent`, `var(--token)`, `rgb(var(--token-rgb) / 0.4)` |
| `rgb` | three 0–255 numbers, `"232 217 160"`, or `var(--x-rgb)` |
| `number` | unitless, clamped per token |
| `length` | `px` |
| `duration` | `ms` (`s` accepted and converted) |
| `tracking` | `em` / `px` / `0` |
| `easing` | a CSS easing keyword, `cubic-bezier()`, `steps()` |
| `font` | a family stack: letters, digits, spaces, hyphens; at most 12 families |
| `background` | a colour, a gradient, or a comma-separated stack of them |
| `shadow` | up to four px lengths, a colour, optional `inset`, comma-separated |
| `blend` | one CSS blend keyword from a fixed list |
| `image` | only `--select-arrow`, only an inline `data:image/svg+xml` URL |

Non-negotiable, because a pasted theme is untrusted input:

* `;`, `{`, `}`, `@`, `\`, `<`, `>`, `/*` and `//` are rejected inside a value.
  `--text-hi: red; } * { display:none } .x {` must die in the tokeniser, not in
  the browser.
* No `url()` other than the one inline SVG, no remote origin of any kind, no
  `<script>`/`<use>`/`href`/`on…=` inside that SVG. The CSP already forbids
  remote origins; this is the second lock, and "the CSP will catch it" is not a
  posture for text pasted out of a chat window.
* Everything is bounded: value length, token count, nesting depth, numeric
  magnitude, family count. There is no input for which applying a theme is
  slow.
* `var()` takes exactly one token name, no fallback, and that token must exist.
* **Out of range is clamped and reported; unparseable rejects the document.**
  Refusing a whole theme over one silly radius would be pedantry with a line
  number on it; silently accepting `1e999` would not be.

### 20.4 Atomic, or nothing

**One error rejects the whole document and changes nothing on screen.** The
parser returns no document at all when it has an error, and the DOM layer
computes every custom property before it writes any of them. There is no
"most of the theme". A half-applied theme is the failure that makes an app look
broken and cannot be reasoned about by the next paste.

Applying is: validate → paint locally in one pass → persist. If the persist
fails, the previous look is put back, because a theme that survives until the
next launch and then vanishes is worse than one that never landed.

### 20.5 Diagnostics are the feature

Silence is the failure mode that makes a workflow like this feel broken, so:

* every problem carries a **line and column**, and the editor scrolls to and
  selects that line when the problem is clicked;
* an unknown key gets a **did-you-mean** by bounded edit distance, or nothing
  at all when the nearest name is a different idea (`--m-hot` for
  `--midi-channel` is worse than saying nothing);
* problems are ordered by position, errors and warnings are distinguishable,
  and the count of each is visible before Apply.

### 20.6 Contrast is measured, and only warned about

Before a document can land, the pairs that decide whether the app is readable
are measured for **both** themes, compositing translucent inks over the surface
they actually sit on, at WCAG 2.1 ratios: body text 4.5:1, secondary text and
non-text objects 3:1, the deliberately quiet tokens lower. The audit runs on
the parsed document, so the warning arrives *before* anything is applied.

It **warns and never blocks**. An engineer working in the dark may genuinely
want a display quieter than any guideline allows, and a tool that refuses is a
tool they stop using. What it may not do is let the consequence be a surprise.

### 20.7 Reading is tolerant, within limits

The reader (`src/lib/jsonc.ts`) accepts comments, trailing commas, single
quotes, bare identifier keys and a document still wrapped in a ```` ```json ````
fence, because all of those come back from models and none of them is
ambiguous. It refuses `__proto__` / `prototype` / `constructor` as keys, and it
is bounded: 256 KiB of source, depth 16, 2048 entries, 4096 characters per
string. A syntax error reports the position it gave up at.

### 20.8 Persistence: schema 4

`settings.json` goes to **schema 4** with one new key, `themeDoc: string |
null`, holding the document's **source text verbatim** — comments, ordering and
the notes the model wrote, because those are half of what makes the next edit
work.

Rust stores text and validates **storage**, not meaning: it trims, rejects
empty, rejects anything over 256 KiB, and rejects control characters other than
tab / CR / LF. The schema lives in the front end and only there; two validators
would disagree within a release. A blank document clears the key.

* A `themeDoc` that no longer parses is **not fatal and never partial**: the
  window comes up in the designed themes, remembers why, and says so once. It
  is also not a reason to reset the rest of the settings file — a v3 file
  simply has no `themeDoc`.
* The document rides on `AppSnapshot`, like `appearance`, so every window —
  main, EQ, editor — wears the same theme without asking for it, and a reset
  made anywhere is seen everywhere.

### 20.9 The IPC surface it adds

| command | args | returns |
|---|---|---|
| `set_theme_doc` | `text: Option<String>` | `Option<String>` — the text as stored; `null` clears it |
| `reset_appearance` | – | `Appearance` — clears the document **and** the appearance in one write |
| `theme_window_open` / `theme_window_close` / `theme_window_toggle` | – | `()` — the window is Rust's to create |
| `theme_window_state` | – | `bool` — is the editor window open |

`AppSnapshot` gains `themeDoc: string | null`, and `onyx://state` therefore
carries it to every window.

### 20.10 Where the editor lives

**Two surfaces, one implementation.** Both drive the same validate / apply /
revert seam (`src/lib/themeio.ts`); they may not behave differently.

* **Settings → Appearance** has two faces, a segmented control: **Theme code**
  (the default) and **Simple** (the §15 pickers, demoted but unchanged). The
  Simple face says so when a document overrides what it controls. The compact
  editor must stay usable inside the 360 px panel and must hold at the
  **420 px minimum window width in both themes** (§4 *Layout*).
* **A window of its own** — `theme.html`, a third document in the same bundle,
  opened from Settings or from the native menu, with its own capability file
  (`src-tauri/capabilities/theme.json`) granting exactly the three event verbs
  the EQ window gets and no window-creation permission.

The editor offers: **Copy default**, **Copy current**, **Copy for agent**
(the document prefixed with the compact contract, so pasting into any chat is
enough to get a usable answer back), **Validate**, **Apply** (`Cmd/Ctrl+Enter`),
**Revert** (drop the document, keep the §15 choices) and **Reset appearance**.
It shows the problem list, the contrast findings and an unapplied-edits marker.

**The editor window never wears the document it is editing.** It is the window
you recover a bad theme in; a theme that could eat it would be a trap. The
§15 appearance still applies there — that is five validated values, not an
arbitrary token layer.

### 20.11 The escape hatch

A document can make every surface the same colour. The settings panel is then
still there, still clickable, and invisible. So the way out may not live in the
themed UI. Three paths, all ending at `reset_appearance`:

1. **`Ctrl`/`Cmd` + `Alt` + `Shift` + `R`**, bound at the capture phase on
   `window` in *every* window, at module scope, before React mounts and whether
   or not it mounts. Not rebindable, not in `keys.ts`, not swallowed by a
   focused text field.
2. The native **Appearance ▸ Reset Appearance** menu item, drawn by the window
   manager and therefore immune to the theme (`src-tauri/src/appmenu.rs`). The
   same menu opens the editor window.
3. The editor window itself, per §20.10.

Each repaints locally in the same frame *and* persists, in that order, so the
chord still works in a window whose IPC is wedged.

### 20.12 Verification

* `scripts/check-theme.mjs`, run by `npm run build` and `npm run build:mock`,
  transpiles the **real** modules and checks the catalogue against
  `tokens.css`, the export → import → export fixed point, the reader's
  tolerances, the sanitiser against hostile values, the contrast maths, the
  suggestions, an LLM-shaped fixture, and **mock ↔ Rust storage parity** from
  one shared fixture (`src-tauri/tests/fixtures/theme_doc_contract.json`). A
  divergence between the two backends must fail the build, not a screenshot.
* `scripts/shots-themedoc.mjs` photographs the workflow end to end: the code
  tab, the standalone editor, an applied theme in both windows, an error with
  its line, a contrast warning, a deliberately unreadable theme, and the reset
  that recovers from it.
* [THEMING.md](THEMING.md) is the document you paste to an agent: the exact key
  set, the value types and ranges, what each token visibly controls, a worked
  example and the failure modes. It is load-bearing — the workflow only works
  if the first edit is a correct one.
