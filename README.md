# Onyx

[![CI](https://github.com/jing1ei/onyx/actions/workflows/ci.yml/badge.svg)](https://github.com/jing1ei/onyx/actions/workflows/ci.yml)
[![Audit](https://github.com/jing1ei/onyx/actions/workflows/audit.yml/badge.svg)](https://github.com/jing1ei/onyx/actions/workflows/audit.yml)
[![Latest release](https://img.shields.io/github/v/release/jing1ei/onyx?display_name=tag&sort=semver)](https://github.com/jing1ei/onyx/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

**Onyx is a desktop audio player for critical listening.** It plays your files
bit-transparently, shows you what they actually measure, and lets you flip
between two versions of the same material — instantly, at the same playhead,
time-aligned if they do not start together, and blind if you want the answer to
be honest.

It is aimed at the moment where you have `mix_v3.wav` and `mix_v4.wav`, or the
24/96 master and the 320 kbps encode, and you need to know whether you can hear
the difference at all.

Platforms: **macOS 11+** (universal, Apple silicon + Intel) is the primary
target, **Windows 10/11 x64** second. Linux builds and runs for development.

> **Verification status.** Everything below that is a property of the Rust
> engine and the app layer is covered by the test suite in this tree
> (`cargo test --workspace`, which is the only place a test count is worth
> reading) and by a full release build. Every push now runs that suite plus
> clippy, `rustfmt` and the four contract checks on macOS, Windows and Linux
> ([CI](.github/workflows/ci.yml)), and a tag builds the shipping bundles
> ([Release](.github/workflows/release.yml)). What CI proves is that the app
> **compiles, links, bundles and passes its tests** on all three platforms. What
> it cannot prove is what a runner has no way to check: the app has still never
> been *run* — no display, no audio device — so the macOS `.dmg` / notarisation
> path and the Windows NSIS / WASAPI path remain unobserved past the point where
> a file lands on disk. See [Known limitations](#known-limitations).

---

## Download

Builds are attached to each [release](https://github.com/jing1ei/onyx/releases/latest).

| Platform | File | Notes |
|---|---|---|
| macOS 11+ | `Onyx_<version>_universal.dmg` | One binary for Apple silicon and Intel |
| Windows 10/11 x64 | `Onyx_<version>_x64-setup.exe` | Per-user NSIS install; registers the file associations |

Unless a release says it was signed, the builds are **unsigned**, and both
systems will say so:

- **macOS** quarantines anything downloaded and unsigned. Open it once with
  right-click → *Open* → *Open*, or clear the flag yourself with
  `xattr -dr com.apple.quarantine /Applications/Onyx.app`.
- **Windows** shows SmartScreen's "Windows protected your PC" — *More info* →
  *Run anyway*. Running the bare `.exe` instead of the installer works but
  registers no file associations.

There is no auto-updater and Onyx phones nowhere: a new version is a new
download. Building it yourself is [three commands](#building-releases).

---

## Why this stack

| Layer | Choice | Why |
|---|---|---|
| Shell | **Tauri 2** | The OS webview draws the UI, so there is no Chromium to ship: the Linux release binary built from this tree is roughly **45 MiB**, of which **32,319,396 bytes are the bundled General MIDI bank** (the one exact number here, because it is a file in the repo) — so the app itself is around 14 MiB, and the whole `.deb` around **33 MiB**, against ~5 MiB before the SoundFont. Still a fifth of the ~150 MB an Electron build of the same app would need. Sizes move with every dependency bump, so they are rounded on purpose; `ls -l target/release/bundle/deb/*.deb` after `npx tauri build --bundles deb` is the current answer. File associations, `RunEvent::Opened`, single-instance forwarding and native dialogs come from Rust, and the same Rust code builds for macOS and Windows. |
| Audio output | **`cpal`** | Talks straight to **CoreAudio** on macOS and **WASAPI** on Windows — one code path, no wrapper layer, no JS anywhere near the audio callback. That is also what makes it possible to ask the device to *change sample rate* to match the file, which the Web Audio API cannot do. |
| Decoding | **`symphonia`** (+ `rubato` for SRC, `rustysynth` for MIDI) | Pure-Rust, no ffmpeg to bundle or license, no process boundary: WAV/BWF, FLAC, MP3, AAC/M4A, ALAC, Vorbis, Opus, AIFF, CAF, Matroska (MKA/MKV), WebM, and the audio track of MOV/MP4. `.mid` is *rendered* through a bundled General MIDI bank and then travels the same path — see [What it plays](#what-it-plays). |
| DSP | **hand-written in `crates/onyx-core`** | Accuracy is the product. The metering has to be standards-compliant to the coefficient, the output callback must never allocate, lock or block, and "bit-transparent" has to mean bit-transparent. Those are properties you can only guarantee by owning the code — and the crate is testable on its own, with no audio device, which is why most of the suite lives in it. |
| UI | **React 19 + TypeScript, `<canvas>`** | The webview is very good at layout, typography and animation, and terrible at 60 Hz React re-renders. So the waveforms, meters and the EQ curve with its analyser backdrop are canvases driven by one `requestAnimationFrame` loop that reads a mutable frame object; React state only holds low-frequency things (playlist, snapshot, settings). Meters that lag are meters you stop trusting. |

One engine, two platforms: everything audible lives in `crates/onyx-core`, which
knows nothing about Tauri or the UI.

---

## Architecture

```
┌───── webview 1 · index.html ─────┐  ┌──── webview 2 · eq.html ─────────────┐
│  App.tsx ── TitleBar ·           │  │  eq/main.tsx ── EqWindow ── EqPanel   │
│    WaveformStack (lanes+meters) ·│  │  same frame bus, same store, its own  │
│    Playlist · TransportBar ·     │  │  document · opened & owned by Rust    │
│    SettingsPanel · BlindTest ·   │  └───────────────────┬──────────────────┘
│    AbRail · Badges · Toasts      │                      │
│                                  │                      │
│  lib/api.ts     typed invoke() wrappers, one per command                     │
│  lib/frame.ts   listens to onyx://frame, keeps a *mutable* frame object;      │
│                 canvases subscribe to one shared rAF loop (no re-render)     │
│  lib/store.ts   zustand: AppSnapshot, UI flags, waveform chunks, toasts      │
│  lib/eq.ts      RBJ biquads in TS — the curve you see is the filter you hear  │
│  lib/eqwindow.ts  open / close / pin, mirrored from onyx://eq-window         │
└───────────▲──────────────────────────────────────────────┬───────────────────┘
   invoke() │                                              │ onyx://frame  60 Hz
            │                                              │ onyx://state  ≤10 Hz
            │                                              ▼ onyx://toast
┌───────────┴──────────────── src-tauri (app layer) ───────────────────────────┐
│  commands.rs  66 commands, every one -> Result<T, String>                    │
│  state.rs     AppState: engine + playlist + 2 deck slots + A/B + blind +     │
│               settings, each behind its own parking_lot::Mutex               │
│  playlist.rs  monotonic ids, background probe pool, missing/analysis flags   │
│  loader.rs    request_rate -> decode::open -> load_deck -> watch -> trims    │
│  blind.rs     2AFC + ABX, per-trial randomised mapping, exact binomial p     │
│  cache.rs     persistent loudness cache (LRU, schema-versioned)              │
│  eqwindow.rs  the EQ WebviewWindow: create / focus / close / pin, restore    │
│  surface.rs   the window background the webview cannot paint, per theme      │
│  archive.rs   a .zip as a playlist: guarded extraction into a temp dir       │
│  settings.rs  appearance · engine source · SoundFont · EQ · monitor ·        │
│               level match · window state, across launches                    │
│  persist.rs   one debounced atomic writer shared by cache.rs + settings.rs   │
│  frame.rs     60 Hz pump: frame events, auto-advance, debounced snapshots    │
│  lib.rs       plugins, CLI argv, RunEvent::Opened, single-instance forward   │
└───────────────────────────────────┬──────────────────────────────────────────┘
                                    │  Arc<AudioEngine>
┌───────────────────────────────────▼──── crates/onyx-core ────────────────────┐
│  decode  symphonia + rubato ─► pcm (lock-free, grows while playing)          │
│                             ├─► waveform (min/max/RMS pyramid, ~2400 buckets)│
│                             └─► loudness analysis (integrated LUFS / TP)     │
│  align   envelope + band-limited waveform nominators, full-rate NCC arbiter  │
│  engine  cpal stream ─ deck A ────────┐                                     │
│                       deck B (+off) ──┴─ xfade ─ trim ─ ø ─ vol ─ EQ ──────┐ │
│                                                                            │ │
│              meter tap ◄───────────────────────────────────────────────────┘ │
│                            └─ audition ─ MONITOR MATRIX ─► device            │
│  midi    rustysynth + the bundled GM bank ─► the ordinary decode path        │
│  dsp     biquads · dynamic EQ · BS.1770-4 loudness · 4× true peak · FFT      │
└──────────────────────────────────────────────────────────────────────────────┘

output callback: no allocation, no locks (one try_lock on device rebuild),
                 degrades to silence rather than stalling.
```

Signal order, in the engine's own words:
`decks → trim → crossfade → volume/env → EQ → [meter tap] → audition → monitor
fold → device`. Note where the meter tap sits: **after** the EQ, **before** the
band-solo audition filter and the monitor matrix. That is a deliberate design
decision, explained under [Monitor matrix](#monitor-matrix).

Threads: the cpal output callback, one analysis thread (meters), one decode
thread per loaded deck, a small probe pool for playlist metadata, one 60 Hz
frame/event thread, one debounced disk-writer thread, plus short-lived
loader / auto-align / auto-advance threads.

---

## The fast path

The whole point of the app is that nothing gets between you and the sound.

- **Open = replace and play.** Opening files — the dialog, `⌘/Ctrl+O`, "Open
  with Onyx" from Finder/Explorer, or a path on the command line — **clears the
  playlist**, loads the first file and **starts playing immediately**. No
  "add to library" step, no confirmation.
- **Drop = append.** Files dropped on the window are appended and playback is
  *not* interrupted. (If the playlist was empty, the first dropped file plays.)
- **One click = play.** A single click on a playlist row loads it into deck A and
  plays from 0. There is no double-click gesture to learn.
- **Progressive decode.** `decode::open` returns after the header parse and
  playback starts as soon as ~150 ms of audio exists, so clicking a 2-hour FLAC
  sounds immediately instead of after a full decode. The waveform, the loudness
  numbers and the seekable region fill in behind you.
- Folders are walked recursively (depth-limited, up to 5 000 files per open) and
  filtered to the list below. Metadata is probed on a background pool, so
  dropping 200 files fills rows in as they resolve and never blocks the UI.
  Unreadable files are marked *missing* rather than silently dropped.

---

## What it plays

**Audio.** WAV/BWF (PCM 8–32 bit, float, A-law/µ-law, IMA-ADPCM), FLAC, MP3,
AAC and ALAC in M4A, Ogg Vorbis, **Opus**, AIFF/AIFC, CAF, Matroska (MKA and
MKV) and WebM — and the **audio track of MOV and MP4**, so a picture-lock bounce
plays without being re-exported first. Detection is by *content*, not extension:
a `.wav` that is really a FLAC opens anyway, and a renamed file cannot lie its
way into the wrong demuxer. The accepted extension list is one constant,
`decode::SUPPORTED_EXTENSIONS`, and the file dialog, drag-and-drop and the
folder walk all read it (through `playlist::openable_extensions()`, which adds
`zip`). Which of those extensions Onyx *claims* in Finder or Explorer is a
narrower, deliberate list — see [file associations](#windows-nsis-installer).

**MIDI.** `.mid` / `.midi` are **rendered**, not decoded: `rustysynth` plays the
file through a General MIDI bank and the render then travels the ordinary
decoded-audio path, so waveform, seeking, loudness, A/B, EQ and metering work on
it with no MIDI-awareness anywhere downstream. Tempo maps and multi-track files
are honoured, and rendering continues past the last event until the tail has
actually decayed, so the final chord is not cut dead. The title bar reads
`MIDI · GM · <bank>`.

The bundled bank is **GeneralUser GS v2.0.3** by S. Christian Collins
(32,319,396 bytes, ~30.8 MiB), a compact full-coverage GM set rather than a
100 MB orchestral one. It is redistributable under the GeneralUser GS License
v2.0; the terms, the checksum and the upstream source are recorded in
[THIRD-PARTY.md](THIRD-PARTY.md) and the licence text ships beside the binary.
Point Settings at your own `.sf2` if you prefer; an invalid or missing file
falls back to the bundled bank and says so. Because a render's loudness is a
property of the file *and* the bank, the bank identity is folded into the
loudness-cache key — changing bank does not serve you a stale measurement.

**Zip archives.** Opening a `.zip` makes it the playlist (dropping one appends),
entries in natural order — `track2` before `track10` — nested directories
walked, non-audio entries skipped silently, undecodable ones summarised in one
warning rather than one per file. The playlist head names the archive, and rows
name theirs when a playlist mixes sources. An empty or audio-free archive says
so in words.

A zip is untrusted input, so extraction is guarded: no entry may escape the
extraction root (`..`, absolute paths, drive prefixes, UNC roots — checked
before *and* after joining), symlink entries are refused outright, nested zips
are not recursed into, and the archive's own unix mode is ignored so nothing
arrives executable. Size is capped at **2 000 entries**, **4 GiB** total
uncompressed and a **1 000:1** compression ratio — and because those figures
come from the archive itself, the byte budget is enforced *again* while reading,
so a central directory that lies cannot fill the disk. Extraction goes to a temp
directory owned by the playlist entry that needs it, removed on playlist clear
and on exit, with stale directories from a previous crash swept at startup.


---

## A/B comparison

Two decks, **one playhead**. Both decks are decoded and positioned identically;
`Tab` (or the A/B rail) moves which one is audible, with a short equal-gain
crossfade (default 8 ms, configurable 0–200 ms) so the switch does not click.
Position is preserved exactly, because the position *is* shared — there is no
seek involved in switching, and no gap to lose your place in.

### Getting a track onto deck B

A plain click on a playlist row always means *load deck A and play* — that is
the app's fastest path and it does not change. Deck B therefore has routes of
its own, and there are four of them, because for a while there was effectively
one: a right-click menu nothing on screen mentioned, which a user reported as
"deck B is not assignable, only A".

| route | how |
|---|---|
| **Row chips** | Each playlist row carries small `A` / `B` buttons in the Deck column. The chip for the deck a row is on is always lit in that deck's colour; the other appears when you hover the row, when the row is selected, or when a chip has keyboard focus. One click assigns. |
| **Drag to a lane** | Drag a playlist row onto a waveform lane. The lane lights up in that deck's colour and says `assign to deck B` before you let go. |
| **Drop a file on a lane** | A file dragged in from the Finder and dropped *on a lane* is added to the playlist and put on that deck. Dropped anywhere else in the window it appends, as always. |
| **`⇧A` / `⇧B`** | Assigns the selected row. Plain `A` / `B` are a different thing — they choose which deck you *hear*. |
| Right-click a row | Still there, with the same two entries. |

Assigning to **deck B turns A/B on**: putting something on B is a request to
compare. Assigning deck A never turns it off. That rule lives in one place,
`src-tauri/src/abrules.rs`, and a checked-in fixture holds both the Rust engine
and the browser mock to it (`cargo test --workspace`, `npm run check:ab`) — they
had drifted apart on exactly this, which is why the preview looked healthy while
the shipped app did not.

When A/B is on, the waveform area shows lane A above lane B in one column,
sharing one playhead and one click/drag seek surface. The audible lane is
labelled `audible` in its deck's colour and carries a coloured edge bar; the
other says `silent`. A deck with nothing on it says so, and lists the routes
above, rather than showing an empty rectangle.

### Level matching is opt-in, and only ever attenuates

Loudness differences dominate quality judgements, so Onyx can remove them — but
only when you ask. **The toggle is off by default** (`G`, or the button in the
A/B rail).

*Why off by default:* a mastering engineer switching between two versions has to
be able to trust that what they hear is the file. With matching off, the gain
stage **early-outs**: the trim is not "0 dB converted to a gain of 1.0 and
multiplied", the multiply does not happen at all, and the path stays
bit-transparent. There is a test that asserts the output is sample-for-sample
identical to the input in that state.

*Why attenuate-only:* the quieter deck is the reference and stays at unity; the
louder deck is brought **down** to meet it. Boosting a loud master to match a
louder one is a fast route to clipping and true-peak overs on material that is
already hot, so Onyx never does it. Both trims are therefore always ≤ 0 dB
(clamped at −24 dB so an absurd pair cannot mute a deck).

- Target = the quieter deck's **integrated LUFS** (BS.1770-4 gated), measured
  over the whole file by the decoder — not by the live meters.
- Switch it on before a measurement exists and the state reports
  `ready: false`, applies unity, and shows `MATCHED · pending`. The real trims
  land automatically the moment the measurement does. It never pretends to be
  matched when it isn't.
- Files that measure as silence (≤ −70 LUFS) are left alone.
- Enabling, disabling and re-deriving the trims all glide, so the toggle does
  not click.
- A non-zero trim means the chain is no longer bit-transparent, and the title-bar
  badge says so.

### Time alignment

Two masters of one track rarely start on the same sample — a different amount of
head silence, a different bounce region — and comparing them at the same playhead
is then meaningless. The `ALIGN` group slides **deck B only**; deck A is the
reference timeline, and loop bounds and the displayed clock stay on A.

- **Auto-align** is two-stage, on a worker thread, over up to the first 60 s of
  each deck (so a three-minute master costs the same as a one-minute one:
  ~0.2 s).
  - *Coarse* — two nominators, neither of which decides. A mono-summed
    short-window energy envelope (2 ms hop), FFT cross-correlated, which is
    what survives two masters that differ in EQ and compression; **and** the
    band-limited waveform (250 Hz – 1.2 kHz, decimated to 6 kHz) cross-
    correlated over the same range. The second one exists because music is
    periodic: on a track with a steady beat the envelope correlates almost as
    well one beat out as it does at the truth, and waveform detail is what
    tells two beats apart. The band starts at 250 Hz because mastering EQ works
    below that, and a minimum-phase filter shifts the phase of the band it
    touches.
  - *Fine* — every coarse candidate is refined independently by a normalised
    cross-correlation at full rate over a high-energy segment, and the
    candidates then compete on **that** score, not on the coarse one. Sample
    accuracy matters because people use this to null two versions against each
    other.
  - Both stages run on a spectrally flattened (pre-emphasised) copy, so a
    sustained bass note cannot own the correlation.
- **Confidence below 0.3 changes nothing.** It returns `applied: false` and says
  it could not find a confident alignment. A wrong automatic offset is worse
  than no offset. The figure reported is the peak normalised correlation scaled
  by how *distinct* that peak is: when a second candidate at a different offset
  correlates within 2 % of the winner, the audio does not say which one is
  right, and the confidence collapses. A perfectly looped file therefore
  correlates at 1.0 and is still refused, which is the honest answer — and
  genuinely unrelated files, where every candidate scores much the same, land
  around 0.02–0.10 rather than the ~0.4 a bare peak would report.
- If the best correlation peak is **negative**, it reports
  `polarityInverted: true` and points you at the per-deck `ø` toggle — two
  versions differing only in polarity is a real and very confusing situation.
- Re-running auto-align is idempotent: it estimates against the raw files, not
  against the currently offset positions, so aligning twice does not drift.
- Manual: ±1 sample / ±1 ms / ±10 ms / ±100 ms buttons, `,` / `.` with `⇧` and
  `⌥` for step size, or `⌥`+drag on lane B with a live ms read-out under the
  cursor. Offsets clamp to ±30 s.
- Where the offset pushes B's read position before zero or past its end, deck B
  outputs **silence** — not a held sample, not a repeated start, because either
  would be a lie about the material. Lane B draws those regions flat and dimmed
  so it is obvious *why* it is quiet, and draws its waveform shifted by
  `−offset` so the two lanes line up under the shared playhead.
- A non-zero offset raises a persistent badge; clicking it resets to zero.

> **Honest limitation: alignment is offset-only.** It finds one signed constant
> delay and applies it. It does **not** time-stretch, and it will **not** align
> two versions that differ in tempo, that have a different edit or arrangement,
> that drift (tape, or a non-integer resample), or that differ in length because
> a section was added or removed. On that material auto-align will either return
> low confidence (a 2 % tape-speed difference measures 0.01, and is refused) or
> lock onto whichever section happens to correlate best, and the rest will still
> be misaligned. Onyx tells you the confidence rather than
> failing mysteriously, but there is no fix for it in the offset model.

### Blind testing: 2AFC and ABX

Two protocols, both requiring both decks loaded (`blind_start` returns an error
with a clear message otherwise), 1–100 trials chosen up front.

- **2AFC** (`ab`) — slots `X` and `Y` map randomly to decks A and B. The
  question is *"which slot is deck A?"*.
- **ABX** (`abx`) — slot `A` is always deck A, slot `B` is always deck B, and
  slot `X` is randomly one of them. You switch freely between all three; the
  question is *"is X the same as A, or as B?"*. This is the standard listening-
  test protocol and the one to reach for when the difference is small, because
  you get to re-reference the two knowns as often as you like.

Both protocols re-randomise the mapping **every trial**, so remembering "X was
the nice one" is worth nothing. The mapping is `null` in the serialised state
while the test is active and is only populated on reveal — the front end cannot
leak what it never receives. Direct deck selection (`Tab`, and `A`/`B` in 2AFC)
is ignored while a test runs, level matching is locked, and the UI masks every
deck-derived read-out: lane names, meter numbers, the A/B rail and the deck
badges all go to `?` or `···`, with slot buttons in fixed slot order so nothing
in the DOM correlates with deck identity.

**The statistic.** The reveal reports the exact **one-tailed binomial** p-value

```
p = P(K ≥ score | n trials, p₀ = 0.5) = Σ_{k=score}^{n} C(n,k) / 2ⁿ
```

computed in `f64` with an iterative binomial coefficient — no dependency, no
normal approximation, correct for the small `n` that real listening tests use.

**How to read it.** It is the probability of scoring *at least* this well by pure
guessing. `11 / 12 correct, p = 0.003` means a coin-flipper gets that result
about three times in a thousand, so you can reliably hear a difference.
`7 / 12 correct, p = 0.387` means a coin-flipper gets that result about four
times in ten, so there is **no evidence** you can hear a difference — which is
not the same as proof that there is nothing to hear; it may just mean 12 trials
were not enough. The reveal wording flips at `p < 0.05`. `p` is `null` until the
test finishes, so there is no way to stop early on a lucky streak and quote a
number.

Randomness is a xorshift64\* seeded from the system clock mixed with a
process-wide counter. It is not cryptographic and does not need to be; it does
guarantee that two runs started back-to-back get different sequences.

---

## Monitor matrix

A monitoring fold on the master bus: **stereo** (default) · **mono** `(L+R)/2` ·
**left only** · **right only** · **swap** · **side** `(L−R)/2` · **flip right**
(polarity). One key each — `O S [ ] \ P` — and each key returns to stereo when
the fold it selects is already active, so it is a one-key check-and-release.
Switching crossfades over 5 ms, allocation-free, so it never clicks.

`stereo` is a **true no-op**: the code branches out of the matrix entirely rather
than multiplying by 1.0, so the bit-transparent path survives. There is a test
asserting the stereo path is sample-for-sample identical to no matrix at all.

There is no separate `mid` mode, because mid *is* `(L+R)/2`, which is exactly
`mono`. Adding both would be two names for one matrix.

**Why the meters are tapped before the fold.** The matrix sits *after* the meter
tap on purpose. LUFS, LRA and true peak therefore always describe the
**programme** — the thing you would deliver — and never the monitoring choice you
happen to be auditioning through. Flipping to `side` for two seconds in the
middle of a track would otherwise dump a −20 LU passage into the integrated
measurement and quietly corrupt it. The cost of this decision is that the meters
do *not* tell you what is coming out of the speakers while a fold is engaged,
which is why any fold other than `stereo` raises a persistent, unmissable badge:
`side` and `flip right` in particular sound broken, and the user must never have
to guess why.

---

## Interactive EQ — a window of its own

**Press `E`.** The EQ is not a drawer inside the player; it is a separate,
resizable window (940×560 to start, minimum 620×360) that you can throw onto a
second screen, size to the whole display, or float above everything with the
`Float` toggle in its header. A curve wants room, and it wants to sit *beside*
the waveforms it is shaping rather than on top of them.

Press `E` again — in either window — and it closes. So does `Esc`, or the OS
close button. Closed, it costs nothing: `set_spectrum_enabled(false)` stops the
FFT analyser, and the audition bandpass goes with it.

Nothing about the audio depends on that window being open. **The bands live in
the engine**, not in the EQ document, so closing it mid-phrase does not touch
playback, the curve, or the bypass state; reopening shows exactly what you left.
Two windows cannot disagree about a filter because neither of them owns it.
Whether the window was open, and whether it was pinned, come back on next launch.

Rust creates and destroys it (`src-tauri/src/eqwindow.rs`) — the renderer has no
window-creation permission, and the EQ window's capability file grants it three
event verbs and nothing else. Asking for the EQ twice focuses the window you
already have; there is never a second one.

- Log frequency axis 20 Hz – 20 kHz, gain axis ±18 dB with a labelled grid.
  Bands pushed beyond ±18 dB stay draggable; the axis does not rescale under the
  cursor.
- **0 to 16 bands, created and deleted on the curve itself.** Zero bands is the
  resting state, and an empty or disabled band list **early-outs to the
  bit-transparent path** rather than running unity-gain filters. Tested.
- Click empty graph area → a bell at that frequency and gain. Drag a node →
  frequency on X, gain on Y (`⇧` constrains to gain, `⌥` to frequency). Wheel
  over a node → `Q` (`⇧`+wheel for fine). Double-click → delete. Right-click →
  filter type, and 12/24/48 dB/oct slope for HP/LP. `⌥`+click → bypass that
  band. Adding, removing or retyping a band crossfades over ~5 ms.
- The bright composite curve is evaluated from the **same RBJ biquad
  coefficients the engine runs** (`src/lib/eq.ts` mirrors `dsp/biquad.rs`), not
  from a parametric sketch — a curve that disagrees with what you hear is worse
  than no curve. Per-band responses sit behind it, dim and thin, and a decaying
  peak-hold FFT analyser is the backdrop. Shelf `Q` is the **RBJ shelf Q** (the
  cookbook `2·√A·α` term) on both sides, and the 12/24/48 dB/oct slopes are
  cascaded Butterworth sections with the knob's `Q` applied to the outermost
  section only — again on both sides.
- That agreement is **pinned, not asserted.** `crates/onyx-core/tests/eq_curve_contract.rs`
  freezes the engine's response for 462 band configurations × 32 log-spaced
  probe frequencies (14 784 points), and `npm run check:eq` transpiles the real
  `src/lib/eq.ts` and compares its composite curve against that fixture. Both
  run in CI-shaped commands (`cargo test`, and `npm run build` /
  `npm run build:mock`, which call it before Vite). Worst measured disagreement
  today is **5.6 × 10⁻⁵ dB**, against a 10⁻³ dB tolerance. Two real divergences
  were found and fixed this way: a Q clamp floor of 0.1 in TS against the
  engine's 0.05, and a −120 dB per-section stopband floor in TS that the
  cascaded engine response goes well below.
- Band nodes label frequency with the nearest note name (`440 Hz · A4`), because
  this is a music tool.
- The front end owns the band list and always sends the **complete** config via
  one setter, `set_eq { config }`. There are no per-band IPC commands, on purpose:
  one authoritative setter is what stops the two-sources-of-truth bug class.
- Real-time safety: the audio side pre-allocates for all 16 bands and receives
  configs through a fixed-size inline array over the lock-free command queue.
  Adding and removing bands during playback is covered by the
  allocation-counting test.

### Band solo — the reason the window exists

**Hold `⌘`/`Ctrl` and drag anywhere on the graph.** A narrow bandpass follows the
cursor: X sets the centre frequency, Y sets `Q` (roughly 2 at the bottom of the
graph to 24 at the top). Release and you are back to normal monitoring. This is
how you hunt a resonance — you sweep until the thing you can hear becomes the
only thing you can hear. Every band also has a hold-to-solo control that
auditions that band's own `freq`/`Q` region.

It is implemented in the engine as a dedicated audition bandpass on the master
bus after the EQ, with the same 5 ms crossfade on entry and exit so sweeping does
not zipper, and it is allocation-free. While auditioning, an unmistakable
indicator stays up **in both windows** — same reasoning as the monitor badge:
this is a monitoring state you must never mistake for the actual signal, and you
are looking at the waveforms, not at the EQ, when you find the note. It rides
the 60 Hz frame payload, because one webview cannot see the other's state.

---

## Persistent loudness cache

Integrated LUFS / LRA / true peak are known for files you have played before,
without playing them again — so the meter cluster is populated and level
matching is correct the instant you load one, instead of after a decode.

(The playlist itself does **not** show a LUFS column. It had one, and a format
column, and both were removed: the numbers were blank for anything not yet
analysed, they duplicate the title bar for the file you are actually listening
to, and eight rows of them made a quiet list read like a spreadsheet. The cache
is not for the list, it is for the meters and the trims.)

- Lives in the OS cache dir as `loudness-cache.json`
  (`cache_stats` reports the exact path; `cache_clear` empties it — both are
  surfaced in the settings panel).
- Keyed on everything the *decoded* audio depends on, not just the file:
  `path | size | mtime | rate | decode semantics | soundfont`. Size and mtime so an
  **edited** file is re-measured; the **rate** because with *follow source sample
  rate* the same file decoded for a 48 kHz device and for a 96 kHz one is a
  different measurement (different resampling, different true peak); the
  **decode-semantics version** (`onyx_core::decode::DECODE_SEMANTICS`) because how
  a file becomes samples has moved before and will again — Opus pre-skip, AAC
  edit-list priming, channel folding, dither, the loudness maths — and a
  measurement made under the old rules must not be handed back under the new
  ones; and the **SoundFont** for a MIDI render. Keys are hashed to keep the file
  small, with the readable path stored alongside for debuggability.
- Every record carries a `schema` version (currently 2 — 1 was keyed without the
  rate or the decode semantics). Bump it when the measurement or the key changes,
  and stale entries are discarded rather than silently trusted.
- Bounded at 5 000 entries with least-recently-used eviction.
- Writes are **debounced** (≥2 s) and atomic (`*.tmp` then rename) on a dedicated
  thread — never from the audio or decode thread, never blocking a command. A
  corrupt or unparseable file is discarded with a warning, never fatal.
- Cache hits feed the A/B level-match trims too, so with matching on, switching
  to a previously-played file is matched instantly instead of after a decode. A
  completed decode is authoritative and overwrites the cached value; a truncated
  or partial decode is never written.

**Waveform peaks are deliberately not cached.** They are cheap to regenerate
from PCM you are decoding anyway, and they are orders of magnitude larger than
four floats per file — caching them would make the cache file dominate the disk
footprint for no perceptible gain. The waveform lanes redraw progressively on
every load instead, which is why they fill in rather than appearing complete.

Your **appearance** (theme, accent, both fonts, size scale), the **engine
source** (host, device, rate, buffer), the **SoundFont** path, EQ config, monitor
mode, the level-match toggle, the EQ window's open and pinned state, volume and
budget persist separately in `settings.json` (schema 3 — schema 2 files are
migrated, not discarded) in the OS
**config** dir, using the same debounced atomic writer. The two are deliberately
in different directories: the loudness cache is regenerable measurement data and
belongs in the **cache** dir, where a backup or a sync client is free to ignore
it, while your EQ curve is not. Corrupt settings fall back to defaults **with a
warning** — never a crash, and never a silent reset that loses your curve without
telling you.

---

## Metering: what "standards-compliant" means here

`crates/onyx-core/src/dsp/loudness.rs` implements **ITU-R BS.1770-4** /
**EBU R 128**:

- The two-stage **K-weighting** filter (shelving + RLB high-pass) is designed
  analytically per sample rate rather than copied from the recommendation's
  48 kHz coefficient table. At 48 kHz the design reproduces the printed
  coefficients to ~1e-9 (there is a test that asserts exactly that), and 44.1 /
  88.2 / 96 / 176.4 / 192 kHz all measure the same programme identically — a
  test asserts that too.
- Mean-square energy is accumulated in 400 ms blocks overlapping by 75 %.
- **Momentary** = 400 ms, **short-term** = 3 s.
- **Integrated** applies both gates: the absolute −70 LUFS gate and the relative
  −10 LU gate computed from the absolutely-gated mean.
- **LRA** = the 95th minus the 10th percentile of the short-term distribution
  above the −20 LU relative gate.
- Channel weighting is per BS.1770 (L/R at 1.0; Onyx folds to stereo, so no
  surround +1.5 dB channels arise).

**Validated, not asserted.** `crates/onyx-core/tests/loudness_ebu.rs` runs the
**EBU Tech 3341 (2023) Table 1** minimum-requirement signals 1–6 and 9–14 and
the **Tech 3342 (2023) Table 1** LRA signals 1–4, at 48 kHz — with the
steady-tone and gating cases repeated at 44.1 and 96 kHz — inside
the recommendations' own tolerances (±0.1 LU for the loudness read-outs, ±1 LU
for LRA). Tech 3341 tests 7–8 and Tech 3342 tests 5–6 use authentic-programme
material that cannot be synthesised, so they are **not** covered — and Tech 3341
test 6 is the 5.0 surround signal, which Onyx answers about 5 LU low *by design*
because it folds to stereo before measuring; there is a test that pins that
shortfall so it cannot drift silently.

`truepeak.rs` implements **BS.1770-4 Annex 2** true peak: 4× polyphase
oversampling. Instead of the 48-tap table (specified only for one base rate) the
interpolator is generated from a Kaiser-windowed sinc with 12 taps per phase,
which tracks the reference filter within a few hundredths of a dB at any rate.
`tests/truepeak_bs1770.rs` checks it against **Tech 3341 tests 15–23** (the
dBTP cases), asserts it never reads below the sample peak, and measures the
interpolator's passband flatness and its behaviour on a band-limited impulse.

The meter cluster beside the waveform shows digital peak + 1.5 s hold, 300 ms
RMS, true peak in dBTP, LUFS M/S/I, LRA and stereo correlation, with the true
peak cell doubling as the clip indicator and the meter reset. The 96-band log FFT
is not displayed there: it is the backdrop of the EQ curve, and nothing else
switches it on. Two independent measurement paths exist, for different jobs:

- the **decoder** measures the whole file once, off the real-time path — that is
  what the cache stores, what the title bar reports and what level matching uses;
- the **live meters** measure the live bus (post-trim, post-volume, post-EQ,
  pre-audition, pre-monitor-fold) for the moving read-outs. `reset_meters`
  restarts the integration.

---

## Sample-rate following and bit transparency

Onyx prefers to move the *device* rather than the audio. When a track is loaded:

1. the source rate is read from the probe,
2. `engine.request_rate(source_rate)` re-clocks the output device (CoreAudio /
   WASAPI) **before** any decoding starts,
3. only then is `decode::open(path, engine_rate, budget)` called, so the decoder
   writes PCM at the rate the device is really running at and the resampler is
   bypassed entirely.

If the device refuses the rate (a fixed-rate USB interface, or a shared WASAPI
endpoint locked by the system mixer), Onyx keeps the current rate, warns via a
toast, and resamples with `rubato` — accurate, just not bit-transparent.

The title bar's **BIT-TRANSPARENT** badge is deliberately strict. It requires
*all* of: engine rate == source rate, the PCM actually stored at that rate, EQ
transparent, no audition bandpass, `monitorMode == stereo`, deck not
polarity-inverted, not muted, volume exactly 1.0, and level-match trim 0.
Anything else and the badge goes away, because the samples reaching the device
are no longer the samples in the file. Every one of those is a state you can
leave switched on by accident, which is precisely why the badge has to be honest.

Switching the output device (or re-enabling "follow source rate") re-decodes both
decks, because PCM already in memory is stored at the old rate.

**While that is happening, a deck whose PCM is at the wrong rate is silent.** It
stays loaded — the file, the trim and the playlist row are all still there — but
it is taken off the audio output until it has been re-decoded, and given back the
moment it agrees with the engine again. 44.1 kHz PCM played by a 96 kHz clock is
not a glitch you would recognise as one: it is a fifth-and-a-bit sharp and 2.18×
too fast, which sounds like a different master rather than a fault. This also
covers the ugly path — a load that re-clocks the device, fails, and then cannot
put the rate back: the decks stay quiet, an error says so in words, and they are
re-decoded at whatever rate the device ended up on. Silence and an honest message
beat confident wrong-pitch audio in a tool people make mastering decisions with.

### Choosing the source: host, device, rate, buffer

Settings exposes what the engine already knows, in that order: **host** (the
audio API — CoreAudio on macOS, WASAPI and ASIO where present on Windows, ALSA
on Linux), **output device** for that host plus a *System default* entry that
keeps following the OS default when it changes, **sample rate** (`Follow source`,
the default and the only bit-transparent choice, or a fixed rate from the ones
the device reports), and **buffer size** from the device's own supported range.

The buffer row shows the number you actually care about — the resulting
**latency in ms** — next to the frame count, because "256 frames" means nothing
until you know the rate. Every change rebuilds the stream in place: position and
transport state are restored, and a device that vanishes mid-playback lands in
the existing output-fault path rather than a panic. A device that is gone at next
launch falls back to system default and says so in a toast instead of failing to
start. The selection persists in `settings.json`.

Enumeration is honest about an empty machine: with no audio hardware — a
container, a CI runner — the lists come back empty and the panel says
*No output devices found*, rather than inventing a default.

---

## Appearance

Two designed themes, **Dark** and **Light**, plus **Auto** (follow the OS, live,
no restart). Dark is the original obsidian/champagne identity, untouched. Light
is designed rather than inverted: warm alabaster paper, aged-brass accents, and
canvas elements re-tuned so waveform bars, meters and the EQ curve read as crisp
on a light field instead of washing out.

Every colour in `src/` comes from `src/styles/tokens.css` — including the
canvases, which cannot inherit a CSS variable and therefore resolve the palette
explicitly on first paint and on every theme change. The EQ window follows along
while it is open, because it reads the same tokens.

In Settings:

- **Accent** — six presets (Champagne, Bronze, Terracotta, Sage, Verdigris,
  Iris) or any hex you type. Hover, pressed, dim and ink variants are *derived*
  from whatever you choose, so a custom accent does not break states that were
  hand-tuned for champagne. Bad hex is rejected visibly, in place, rather than
  silently falling back to the default.
- **UI font** — System UI, Grotesk, Humanist, Neutral. **Numeric font** — System
  mono, SF Mono, Menlo, Consolas, Courier: monospaced only, because the
  read-outs are tabular and must stay column-stable while a number changes.
  These are *tokens*, mapped in `tokens.css` to stacks of faces the platform
  already ships. Nothing is ever fetched — the CSP forbids remote origins and
  this is an offline tool.
- **Size scale** — compact / normal / large, one root factor. `large` still fits
  the 420 px minimum; `scripts/shots.mjs` photographs and measures that case.
- **Reset to defaults**, with a marker when your appearance differs from stock.

The whole appearance lives in `settings.json` and is applied by one function,
`applyAppearance()` in `src/lib/theme.ts` — the settings panel calls it and owns
nothing else about colour. On launch it paints from a `localStorage` mirror
*before* first paint so there is no white flash, then reconciles with what Rust
reports. What to call and what never to hard-code is
[THEMING.md](THEMING.md).

### Theme code — hand the look to a model and paste it back

Six pickers are six decisions. If what you want is *a different look*, picking
colours one at a time is the wrong tool, so Settings → Appearance has a second
face, **Theme code**, and it is the one that opens by default. The **Simple**
tab beside it is the six controls above, unchanged.

The workflow is three copies and a paste:

1. **Copy for agent** puts the current appearance on the clipboard as a
   commented JSONC document, with a short contract in front of it explaining
   what Onyx is and what the value types are.
2. Paste that into whatever model you use, with one sentence: *make it a cold
   graphite studio look*, *warmer paper, less contrast*, *match this album
   cover*.
3. Paste the reply into the box and press **Apply** (`⌘/Ctrl+Enter`). The
   window is wearing it before the keystroke finishes.

What you copy is generated from `tokens.css` itself, so it is *exactly* what
you are looking at — 172 tokens, grouped and commented, with the two designed
themes side by side. Every key is editable, every key is optional: delete the
ones you do not want to change and the designed value stays. Comments and
trailing commas are fine, so is a reply still wrapped in a ` ```json ` fence.

It fails usefully, which is the part that makes it worth using:

- **One error rejects the document and changes nothing.** There is no
  half-applied theme to un-paste.
- Errors carry a **line number** and a *did you mean* — `"txt-hi"` is told
  about `"text-hi"` rather than silently ignored. Click a problem and the
  editor selects that line.
- Out-of-range numbers are **clamped and reported**, not refused: a 400 px
  panel radius becomes 48 px and says so.
- **Contrast is measured before the theme lands**, for both themes, on the
  fourteen pairs that decide whether the app is readable — composited, because
  most inks here are translucent. Onyx warns; it does not refuse. Working dark
  and quiet is a legitimate choice, being surprised by it is not.
- Nothing in a theme is CSS. Values are parsed into typed data and re-emitted,
  so a `;` or a `url(https://…)` is a parse error rather than a stylesheet.

**Revert** drops the document and leaves your accent and fonts alone. **Open in
a window** (also `Appearance ▸ Theme Editor…`) puts the same editor in its own
window — which deliberately never wears the theme it is editing, so a theme
that has made everything one colour cannot eat the place you fix it from.

And if it has: **`⌘/Ctrl + ⌥/Alt + ⇧ + R`** resets the appearance from any
window, at any time. It is bound before React mounts and cannot be swallowed by
the theme or by a focused text box, and the native **Appearance ▸ Reset
Appearance** menu item does the same thing from outside the webview entirely. A
theme that no longer parses at launch — hand-edited `settings.json`, a file
copied from a newer build — is dropped with a message; the app comes up looking
like Onyx rather than not coming up.

The document is stored verbatim in `settings.json` (`themeDoc`, schema 4,
256 KB ceiling), comments and all, because those notes are half of what makes
the next edit work. Rust checks that it is bounded text and nothing else; the
schema lives in one place, the front end.

The document you want to give an agent — every key, every type and range, what
each token visibly controls, a worked example and the failure modes — is
[THEMING.md](THEMING.md).

**The native frame is not the webview's to paint.** The macOS traffic lights and
their title bar, the Windows caption, the scrollbars: those belong to the window
manager, so Onyx sets the *window* theme from your choice at startup and on every
change, for both windows — and pins it nowhere. `system` means "no override", not
"dark", which is why neither `tauri.conf.json` nor `tauri.macos.conf.json`
carries a `theme` key any more; a test asserts they do not.

**And neither is the window's own background.** A native window has a background
colour underneath the webview, and an untold one keeps the system's — light grey
on macOS, never obsidian and never alabaster. It is invisible where the webview
covers it and visible where it does not: around the antialiased rounded corners
of a decorated window, and for the frames between "the window is on screen" and
"the webview has painted". That is a pale rim around a dark app, and it is the
one thing a CSS `background` cannot reach. So the window surface is set too,
from the **resolved** theme's `--ink-900` — including whatever a theme document
moved it to ([Theme code](#theme-code--hand-the-look-to-a-model-and-paste-it-back)),
which is why the webview reports the colour it is really painting and Rust does
not guess. Every window: the main one at creation from `tauri.conf.json` and then
at runtime, the EQ and editor windows at creation, all three on every theme
change, OS appearance change, applied document and reset. The editor deliberately
gets the *designed* colour rather than the document's, for the same reason its
webview ignores the document.

What that does **not** remove is AppKit's own 1 px window stroke, which macOS
draws around every decorated window — Finder included — and which no background
colour affects. Dropping it means dropping native decorations, and with them the
traffic lights and native edge-resize; Onyx keeps them. See
[Known limitations](#known-limitations).

---

## Window sizes

The player opens at **1180×760** and will go down to **420×560** — narrow enough
to live in a strip beside a DAW, on the same screen, without becoming a toy. It
is not simply a layout that refuses to overflow: it sheds things in a deliberate
order.

At 1100 the meter column narrows without shrinking a single number. At 960 the
whole meter cluster moves *under* the lanes and lays itself out in a row, giving
the waveforms the full width. At 680 the title bar drops the format badge and
the one-sample nudges leave the alignment bar. At 600 the transport becomes an
explicit two-row grid rather than a wrap lottery. At 480 the playlist keeps
title, deck and time and drops the rest. Nothing that disappears is unreachable:
the nudges are `,` and `.` with `⇧` and `⌥`, and the format is in the file-name
tooltip and in Settings.

Every hit target stays at least 22px and no label drops below 8.5px at any
size — `scripts/shots.mjs` measures both, plus overflow, at 1180 / 900 / 640 /
480 / 420 px on every run, and fails if either slips.

The EQ window is separate and has its own minimum, 620×360.

## Keyboard map

The authoritative list is `SHORTCUTS` in `src/lib/keys.ts`, which also drives the
`?` overlay. Every key is ignored while focus is in a text field or a `<select>`.

Every key is also ignored **while an input method is composing**. A Pinyin, Kana
or Hangul candidate window owns `Space`, `Tab`, `Enter`, `Escape` and the digits
until it commits one, so nothing here claims them while it is open — including the
theme editor's `Tab`, which used to indent the box mid-composition and lose the
characters that were pending. The single exception is the appearance reset chord
(`Ctrl`/`Cmd`+`Alt`+`Shift`+`R`): three modifiers are no part of a composition,
and that chord is the way out of a theme you cannot read.

| Key | Action |
|---|---|
| `Space` | Play / pause |
| `←` / `→` | Nudge ∓5 s (with `⇧`, ∓1 s) |
| `↑` / `↓` | Volume ±1 dB |
| `Home` | Seek to start |
| `A` / `B` | **Listen to** deck A / deck B — during an ABX test, switch to slot A / B |
| `⇧A` / `⇧B` | **Assign** the selected playlist row to deck A / deck B (assigning B turns A/B on) |
| `Tab` | Toggle A/B deck (ignored while a blind test is running) |
| `X` / `Y` | Blind slot switch (`X` only, in ABX) |
| `1` / `2` | Blind vote — 2AFC: "X is A" / "Y is A"; ABX: "X = A" / "X = B" |
| `L` | Loop on / off |
| `M` | Mute |
| `E` | Open the EQ window — or close it, from either window |
| `⇧E` | EQ bypass, without opening anything |
| `⌘/Ctrl` + drag on graph | EQ band-solo sweep (X = frequency, Y = Q) |
| `⌥` + click node | Bypass that EQ band |
| `G` | Level match on / off (locked during a blind test) |
| `,` / `.` | Nudge deck B earlier / later — 10 ms, `⇧` 100 ms, `⌥` 1 sample |
| `⌥` + drag lane B | Slide the A/B time offset instead of seeking |
| `O` | Monitor mono `(L+R)/2` — again for stereo |
| `S` | Monitor side `(L−R)/2` — again for stereo |
| `[` / `]` | Monitor left only / right only — again for stereo |
| `\` | Monitor channels swapped — again for stereo |
| `P` | Monitor polarity-flip right — again for stereo |
| `Delete` / `Backspace` | Remove the selected playlist row |
| `Esc` | Close the top-most overlay — in the EQ window, close that window |
| `⌘/Ctrl+O` | Open files (replaces the playlist, plays immediately) |
| `⌘/Ctrl+⇧+O` | Add files (appends) |
| `⌘/Ctrl+K` | Clear the playlist |
| `?` | Shortcut overlay |

One key is deliberately *not* in that table, because it has to work when the UI
does not: **`⌘/Ctrl + ⌥/Alt + ⇧ + R`** resets the appearance — theme document
included — from any window. It is bound at the capture phase before React
mounts, so a focused text box, an overlay or a theme that has painted everything
the same colour cannot swallow it.

Other accelerators (`⌘Q`, `⌘W`, `⌘,` …) are deliberately left to the OS and the
webview.

### Non-US keyboards

The legends above are the US ones. Onyx binds two kinds of key differently, so
the map stays usable on every layout:

* **Mnemonic keys — `A B X Y L M E G O S P`** are matched on the *character*.
  `M` is the key legended M whatever the layout, because the mnemonic is the
  whole point. `A` and `B` are the one pair matched by position (`KeyA` /
  `KeyB`) *first* and by character second, so the deck keys stay under the same
  two fingers on AZERTY and QWERTZ as well as on the key legended A or B.
* **Positional keys — `[ ] \ , . 1 2`** are matched on the *physical position*
  (`KeyboardEvent.code`). `[`, `]` and `\` need AltGr on German, French and the
  Nordic layouts and sit elsewhere entirely on JIS; the digits need Shift on
  AZERTY. Bound by position they are always one unmodified keypress, in the
  same place under the hand, on every keyboard — and they no longer break when
  a modifier rewrites the character, which is why `⇧,` (100 ms) and `⌥,`
  (one sample, `≤` on macOS) now work at all.

The `?` overlay prints the legends of *your* keyboard for the positional rows,
via the Keyboard Map API on Windows and Linux. WKWebView on macOS does not
implement that API, so there the overlay starts with the US legends and
corrects itself as soon as you press the key in question.

---

## Logs

Onyx writes one rotating log file. A packaged app has no terminal and no
devtools, so this is the only place a failure survives — it is the thing to
attach to a bug report.

| Platform | Log file |
|---|---|
| macOS | `~/Library/Logs/com.onyxaudio.player/onyx.log` |
| Windows | `%LOCALAPPDATA%\com.onyxaudio.player\logs\onyx.log` |
| Linux | `$XDG_DATA_HOME/com.onyxaudio.player/logs/onyx.log` |

It rotates at 4 MiB and keeps two older files, so it cannot grow without bound.
Timestamps are UTC — a log is usually read next to a wall-clock report from
another machine. Debug builds also write to stderr; release builds do not.

The default level is **`warn`**: a player should not write a line per frame to
your disk for ever, and everything above `warn` is something worth asking you
about. Raise it with `ONYX_LOG` — `info` adds the lifecycle (device selection,
sample-rate changes, loads, cache state), `debug` and `trace` add the detail.
Set it in the shell you launch from, and launch the executable directly, because
`open -a` and the Start-menu shortcut do not inherit your environment:

```bash
ONYX_LOG=info /Applications/Onyx.app/Contents/MacOS/Onyx      # macOS
```

```powershell
$env:ONYX_LOG = "info"          # Windows: then start Onyx.exe from this shell
```

A value that is not a level is not fatal; Onyx falls back to `warn` and says so
in the first lines of the file.

What you should expect to find in it:

- **Real-time faults, counted and coalesced.** The audio callback never logs —
  formatting a message allocates, and that is exactly what it is not allowed to
  do. It increments counters, and the 60 Hz frame thread drains them four times
  a second and writes at most one line every five seconds with a count: dropped
  engine commands (a fader move that never reached the callback) and output
  stream errors, each with the device name and rate. So a device that fails
  continuously produces a handful of lines, not a gigabyte. A disconnected
  output also raises one toast per episode, because only you can fix it.
- **`[webview]` lines.** Front-end errors, unhandled promise rejections and
  React error-boundary failures are forwarded into the same file under a
  `webview` target, sanitised, truncated and rate-limited so a render loop that
  is throwing cannot flood it.
- **At `info`, the file *name* of each load — never the directory**, so the log
  stays safe to attach. Full paths appear only at `debug`, and only for a file
  that could not be read.

---

## Security posture

Onyx opens files other people made, in a webview. Both of those are treated as
untrusted, and the boundary is narrow on purpose.

- **The renderer gets eight permissions, and none of them are plugin
  permissions.** `src-tauri/capabilities/default.json` grants
  `core:event:allow-listen|unlisten|emit` and five window verbs used by the
  custom title bar (`start-dragging`, `internal-toggle-maximize`,
  `toggle-maximize`, `minimize`, `close`). It does **not** grant `core:default`.
  There is no `fs:`, `dialog:`, `opener:` or `shell:` permission at all — the
  dialog, opener, log, single-instance and window-state plugins are driven from
  Rust only, so a compromised renderer gets no file picker, no URL opener and no
  second window. Everything the app can do goes through the `#[tauri::command]`
  surface of SPEC §3.1, which the webview cannot widen.
- **`tauri-plugin-fs` is not registered.** It was dropped as a direct dependency
  entirely; Onyx reads files with `std::fs` from Rust, so there is no filesystem
  API in the webview to scope. (It still exists in the lockfile as a transitive
  dependency of `tauri-plugin-dialog`, but it is never initialised.)
- **The CSP is set, not `null`** — `default-src 'self'`, `script-src 'self'` (no
  `unsafe-inline`, no `unsafe-eval`), `object-src`/`frame-src`/`worker-src`/
  `child-src 'none'`, `form-action 'none'`, `frame-ancestors 'none'`,
  `connect-src` limited to `'self'` plus the IPC origin. `img-src` allows
  `data:` for the generated waveform/analyser images. The looser `devCsp` is
  dev-server only. `freezePrototype` is on and the asset protocol is disabled
  with an empty scope.
- **OS-facing commands are guarded.** `reveal_in_finder` refuses any path that
  is not already an entry in the playlist and logs the refusal; the file dialog
  is opened from Rust and returns paths the Rust side then owns.
- **Third-party parsers run inside `catch_unwind`.** `symphonia` asserts rather
  than erroring on some malformed input (a WAV declaring `sample_rate = 0`
  panics in `TimeBase::new`); `src-tauri/src/safe_decode.rs` turns that into an
  ordinary `Err`, discards non-finite/absurd header claims, and — because a
  header is a claim, not a fact — bounds the pre-allocated PCM buffer by what
  the file's own size *could* decode to, so a 456-byte WAV claiming a 4 GiB
  `data` chunk can no longer commit the whole deck budget. A hostile-input
  corpus (`src-tauri/tests/malformed_input.rs`, 14 tests) exercises it.
- **A zip is treated as an attack surface, not a folder.** Zip slip, absolute and
  UNC entry paths, symlink entries, a compression bomb, an entry-count flood,
  absurd directory nesting, and bytes that merely claim to be a zip each have a
  test (`src-tauri/tests/hostile_archives.rs`, 9 tests, plus the unit tests in
  `archive.rs`); the limits are listed under [What it plays](#what-it-plays).
- **`argv` is read as `OsString`**, so a filename that is not valid UTF-8 is
  still opened, and a relative path is resolved against the *launching*
  instance's working directory before it is forwarded to the running one.

`cargo audit` reported **0 vulnerabilities** and 17 warnings across 552
dependencies: 16 `unmaintained`, 1 `unsound`. Twelve of them (`atk`, `gdk`,
`gtk`, `gdkwayland`/`gdkx11` and their `-sys` crates, `gtk3-macros`,
`proc-macro-error`, and the `glib` `VariantStrIter` unsoundness) are reachable
**only** through Tauri's Linux backend — `cargo tree --target` for
`x86_64-apple-darwin`, `aarch64-apple-darwin` and `x86_64-pc-windows-msvc`
shows none of them on the macOS or Windows graphs, and Linux is not a shipping
target. The remaining five are the `unic-*` Unicode-table crates
(`unic-char-property`, `unic-char-range`, `unic-common`, `unic-ucd-ident`,
`unic-ucd-version`), which *do* build on macOS and Windows via `urlpattern` →
`tauri-utils`; they are flagged unmaintained only, carry no known vulnerability,
and cannot be dropped from this tree before Tauri drops them.

That run predates the v3 dependencies (`rustysynth`, `opus-decoder`, `zip`,
which take the tree to 557 crates) and **could not be repeated here**: this
sandbox cannot reach the RustSec advisory database. Re-run `cargo audit` on a
networked machine before shipping. All three new crates are pure Rust with no
`build.rs` and no C, the archive reader is fed only untrusted input behind the
limits in [What it plays](#what-it-plays), and the MIDI and SoundFont parsers
run inside `catch_unwind` like every other third-party parser here.

---

## Development

New to this repository? Read [CONTRIBUTING.md](CONTRIBUTING.md) first — it is
the orientation: which concern lives in which layer, every check command and
what each one guards, the invariants that must not break and the tests that
enforce them, recipes for adding an IPC command or an audio format, and an
honest list of what has never been observed running.

Prerequisites: **Node 20.19+ or 22.12+** (Vite 7's floor, and what both
build scripts check), **Rust stable** (via rustup), and the platform
webview toolchain — Xcode command line tools on macOS, the VS Build Tools
"Desktop development with C++" workload plus the WebView2 runtime on Windows.
On Debian/Ubuntu: `libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev
libayatana-appindicator3-dev libasound2-dev pkg-config build-essential`.

```bash
npm install
npm run tauri dev          # vite on :1420 + the Rust app, both hot-reloading
```

Useful:

```bash
cargo test --workspace                        # the whole suite; no audio device needed
                                              # (one is #[ignore]d: the wide alignment sweep)
cargo clippy --workspace --all-targets        # kept warning-free
cargo fmt --all
npm run typecheck                             # tsc --noEmit
npm run check:eq                              # drawn EQ curve vs the engine's coefficients
npm run check:ab                              # deck assignment: the mock backend vs the engine
npm run check:theme                           # the theme contract: catalogue, parser,
                                              # sanitiser, contrast, storage parity
npm run check:ipc                             # the command surface four ways: SPEC §3.1,
                                              # generate_handler!, api.ts, the mock's switch
npm run check:version                         # the version in package.json, both lockfiles,
                                              # Cargo.toml and tauri.conf.json — and, given a
                                              # tag argument, that tag too
npm run build                                 # tsc + check:version + check:ipc + check:eq
                                              # + check:ab + check:theme + vite bundle -> dist/
npm run build:mock                            # the same, mock IPC -> dist-mock/
npx tauri build --no-bundle                   # link the real app, skip installers
npx tauri build --bundles deb                 # Linux smoke test of the bundle config
VITE_ONYX_MOCK=1 npm run dev                  # UI in a plain browser, mock IPC
```

The three screenshot harnesses need a mock build being served, and take the URL
of it (default `http://localhost:4173`):

```bash
npm run build:mock
npx vite preview --outDir dist-mock --port 4173 &
npm run shots -- <url>                        # Playwright screenshots of a mock build,
                                              # and the layout / gesture assertions
                                              # that go with them, including the
                                              # four routes onto deck B
                                              #                  (shots/01-30, 37-44)
npm run shots:theme -- <url>                  # both themes, custom accents, canvas
                                              # repaint and the appearance-persistence
                                              # seam, and the deck chips and lane
                                              # drop highlight on paper
                                              #                  (shots/31-36, 45)
npm run shots:themedoc -- <url>               # the theme document end to end: the
                                              # editor window, a model's theme applied
                                              # to every window, the error and contrast
                                              # diagnostics, and the keyboard recovery
                                              #                  (shots/46-55)
```

One numbered set across the three, no gaps and no collisions; each one exits
non-zero if anything it asserted was untrue.

`scripts/shots.mjs` is not only a camera. It drives the mock build through the
detached EQ window (a browser popup stands in for the `WebviewWindow`), the
lane modifier gestures, the blind-test masking, the settings surface, an archive
playlist, a MIDI row, five viewport widths and all four routes onto deck B —
dragging a row onto a lane, the row chips, `⇧B`, and a file dropped on a lane —
and it *fails* on overflow, sub-22px targets, sub-8.5px text, a non-integral
canvas backing store, a band row that has ellipsed away its own frequency, an
Alt-drag that seeks instead of aligning, a lane that takes a drop without saying
so first, or an assignment to deck B that does not turn A/B on.
`scripts/shots-theme.mjs` does the same job for appearance: it resolves
the tokens in both themes, checks every canvas actually *repainted* rather than
kept a stale bitmap, drives a custom accent, follows a simulated OS theme change,
and walks the whole persistence seam — settings panel → `applyAppearance()` → the
backend → reload with the front-end cache wiped → the EQ window.
`scripts/shots-themedoc.mjs` drives the theme document of §20 as a user does it,
and asserts the state behind each picture. The browser, the window sizes, the
toast-clearing shutter, the console watch, the popup adoption and the exit
verdict are shared by all three through `scripts/lib/shots-base.mjs`, so the
harnesses hold only their arguments and cannot drift apart on the plumbing. If
any of them prints anything other than `CONSOLE: clean`, the screenshots are
evidence of a bug, not a release.

`SPEC.md` is the single authoritative contract: the IPC surface (command names,
argument names, payload shapes), the behaviour rules, §0–§5 for the core
contract, §6–§12 for the v2 features, §13 for diagnostics, §14–§19 for the v3
features (theming, customisation, engine source, formats, MIDI, archives) and
§20 for the theme document. The v2 and v3 addenda have both been merged into it
and retired; no other spec file exists. `src/lib/api.ts` and `src/lib/types.ts` are the TypeScript mirror. The
icon is generated by `python3 scripts/make-icon.py` (needs Pillow) into
`src-tauri/icons/source.png` and expanded with `npx @tauri-apps/cli icon`.

This is a cargo **workspace**, so build output lands in `./target`, *not*
`src-tauri/target`. `target/`, `dist/`, `dist-mock/`, `artifacts/`, `shots/`
(including `shots/diff/`) and `.tmp/` are all regenerable and all matched by
`.gitignore`; nothing under them should ever be committed.

---

## Building releases

### macOS — the primary target (universal `.app` + `.dmg`)

Prerequisites, all of which the script verifies before it starts — it reports
every missing one at once rather than dying on the first:

| | |
|---|---|
| macOS 11+ | the bundle's own `minimumSystemVersion` |
| Xcode command line tools | `xcode-select --install` — provides `xcrun`, `lipo`, `codesign` |
| Rust (stable, via rustup) | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` |
| Node 20.19+ or 22.12+ | Vite 7's floor; older runtimes fail with an opaque syntax error |
| ~15 GB free | two architectures of a release build; the script warns below 20 GB |

No C toolchain, Homebrew formula or system audio library is needed beyond that:
every dependency including the FLAC/MP3/AAC/ALAC/Vorbis/Opus decoders and the
SoundFont synthesiser is pure Rust, and the output device comes from CoreAudio
through CPAL.

```bash
./scripts/build-mac.sh
```

Must be run **on macOS**; cross-compiling a Cocoa/WKWebView app from Linux is not
supported by Tauri, and the script refuses to try. It installs the
`aarch64-apple-darwin` and `x86_64-apple-darwin` targets if missing, then runs
`npm run tauri build -- --target universal-apple-darwin --bundles app,dmg` and
prints the discovered artifacts plus `lipo -archs` for the bundled executable.
Expect 15–25 minutes cold, since both architectures compile from scratch.

```
target/universal-apple-darwin/release/bundle/macos/Onyx.app
target/universal-apple-darwin/release/bundle/dmg/Onyx_<version>_universal.dmg
```

Minimum system version 11.0, category Music. Signing and notarisation are opt-in
through the `APPLE_*` environment variables documented at the top of the script;
unsigned builds run locally (right-click → Open the first time) but are
quarantined when downloaded.

### Windows (NSIS installer)

Prerequisites, all of which the script verifies before it starts — like the mac
script, it reports every missing one at once rather than dying on the first:

| | |
|---|---|
| Rust (stable, via rustup) | with the **MSVC** host toolchain |
| VS Build Tools, "Desktop development with C++" | `link.exe` and the Windows SDK; a missing `link.exe` is a warning, since `cargo` can still find the toolchain itself |
| Edge WebView2 runtime | ships with Windows 11 and Windows 10 21H2+; a warning if absent, because the build would succeed and the app would then open no window |
| Node 20.19+ or 22.12+ | Vite 7's floor; older runtimes fail with an opaque syntax error |
| ~10 GB free | one triple of a release build; the script warns below 12 GB |

It also checks what the build assumes is on disk: the bundled General MIDI bank
and its licence text (both linked or bundled, and both invisible failures
otherwise), the three HTML entry points and the three capability files.

```powershell
.\scripts\build-windows.ps1        # add -Msi for an .msi as well
```

```
target\x86_64-pc-windows-msvc\release\onyx.exe
target\x86_64-pc-windows-msvc\release\bundle\nsis\Onyx_<version>_x64-setup.exe
```

The installer is per-user by default and registers the file associations, so
"Open with → Onyx" works; running the bare `.exe` does not register anything. The
script prints the associated types, read out of `tauri.conf.json`, at the end of
a build — that config is the only list the installer reads, and a test compares
it against `playlist::openable_extensions()`, the same list behind the file
dialog and the drag-and-drop filter, so every claimed extension is one Onyx can
really open.

The reverse is deliberately not true. `mkv`, `m4v`, `webm` and `adpcm` decode and
can be dropped on the window, but Onyx does not claim them: `mka` is Matroska
*audio* and is claimed, while a video container in an audio player's "Open with"
menu is noise for the person who owns those files, and `adpcm` is a codec-shaped
extension no platform associates. `mp4`, `mov` and `zip` *are* claimed as a
deliberate trade — they belong to other applications in most users' mental
models, so Onyx registers as *an* opener, never as the default, and the
association exists for when you want the audio out of a picture-lock bounce or a
zip of mixes.

Windows and Linux windows have no decorations (the app draws its own 36 px
title bar), so the title bar carries its own minimise / maximise / close
buttons at the right-hand edge. macOS keeps the native traffic lights instead.

### Linux (`.deb`, development smoke test only)

`bundle.targets` is `["app", "dmg", "nsis", "deb"]`; `deb` is in the list so this
check is one command anywhere:

```bash
npx tauri build --bundles deb      # -> target/release/bundle/deb/
```

This is what proves that `tauri.conf.json`, the icons, the `dist/` wiring, the
file associations, the bundled SoundFont licence and the linking are all valid.
It is not a shipping target. The package comes out at roughly **33 MiB** — the
exact byte count moves with every dependency bump and nothing in the tree
asserts it, so read it off the artifact (`ls -l
target/release/bundle/deb/*.deb`) rather than trusting a number in prose. The
General MIDI bank is inside `usr/bin/onyx` (linked with `include_bytes!`, so
there is no loose `.sf2` to lose) and the licence lands at
`/usr/lib/Onyx/GeneralUser-GS-LICENSE.txt`. The generated `.desktop` file
carries the MIME types for audio, `audio/midi`, `video/mp4`, `video/quicktime`
and `application/zip`.

All three bundles declare the same file types, generated from one list in
`tauri.conf.json`.

### Cutting a release

The two scripts above are for building on your own machine. A *release* is cut
by a tag, and [`.github/workflows/release.yml`](.github/workflows/release.yml)
does the rest — a macOS runner for the universal `.app` + `.dmg`, a Windows
runner for the NSIS installer, both attached to one draft release.

```bash
# 1. bump the version — it lives in four files and the two lockfiles
#    (package.json · package-lock.json · Cargo.toml [workspace.package] ·
#     src-tauri/tauri.conf.json, then Cargo.lock via any cargo command)
npm run check:version          # prints all seven and fails if they disagree

# 2. write the entry in CHANGELOG.md

# 3. tag and push
git tag -a v1.0.0 -m "Onyx 1.0.0"
git push origin main v1.0.0
```

The workflow then:

1. **Verifies before it builds.** The tag has to match every version string in
   the tree (`node scripts/check-version.mjs v1.0.0`), and `cargo test
   --workspace` plus the four contract checks have to pass. A mismatch stops the
   run before a single bundle exists, because the alternative is an installer on
   someone's disk claiming a version that was never released.
2. **Builds both platforms in parallel** — `--target universal-apple-darwin
   --bundles app,dmg` on macOS, `--bundles nsis` on Windows. `tauri.conf.json`'s
   `beforeBuildCommand` is `npm run build`, so every check runs again inside the
   release build itself.
3. **Leaves the release as a draft.** Nothing is published until you have
   downloaded both artifacts, opened them, and pressed Publish. Paste the
   `CHANGELOG.md` entry over the placeholder body while you are there.

Signing is opt-in and every secret is optional — with none of them set the build
succeeds and produces unsigned artifacts. To sign and notarise, set these as
repository secrets:

| Secret | For |
|---|---|
| `APPLE_CERTIFICATE` | base64 of a Developer ID Application `.p12` |
| `APPLE_CERTIFICATE_PASSWORD` | its password |
| `APPLE_SIGNING_IDENTITY` | `Developer ID Application: Name (TEAMID)` |
| `KEYCHAIN_PASSWORD` | any string — the temporary CI keychain's password |
| `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` | notarisation; `APPLE_PASSWORD` is an app-specific password |

The other workflows: [`ci.yml`](.github/workflows/ci.yml) on every push and pull
request (the contract checks and both bundles on Linux, `cargo fmt`, clippy and
the test suite on macOS, Windows and Linux, and a `.deb` to prove the bundle
configuration is valid), [`audit.yml`](.github/workflows/audit.yml) weekly and
on any lockfile change (`cargo audit`, which the environment this app was
written in could not reach the advisory database to run), and
[`shots.yml`](.github/workflows/shots.yml) on demand — the three Playwright
harnesses, kept out of CI because they assert *rendered* geometry and a runner's
font stack is not a reviewer's.

---

## Known limitations

**Unverified in this environment — read this before trusting a build.**

CI narrows this list but does not empty it. Every push compiles, links, clippies
and tests the workspace on macOS, Windows and Linux runners, and a tag builds
the `.dmg` and the NSIS installer — so "does it build on a real macOS toolchain"
stops being a question the first time the release workflow runs green. Every
item below is about what happens *after* that, on a machine with a display and a
sound card, which no runner has.

- The **macOS** path has never been executed: no `.app`, no `.dmg`, no universal
  binary, no `lipo`, no code signing, no notarisation, no `spctl` check.
  `scripts/build-mac.sh` has been re-read against the current
  `tauri.conf.json` and workspace layout and its paths corrected, but it has not
  been *run*. Anything macOS-specific — `titleBarStyle: Overlay` and the
  traffic-light inset, `RunEvent::Opened` for "Open with", the
  `public.app-category.music` registration, CoreAudio device switching and
  rate-changing — is written to spec and untested on hardware. In particular
  the window chrome now comes from `src-tauri/tauri.macos.conf.json`
  (`decorations: true` so the traffic lights the 78 px inset reserves space for
  actually exist); the pixel result of that has not been seen. That file no
  longer pins `"theme": "Dark"` — the frame theme is set at runtime from your
  choice, `system` meaning "leave it to the OS" — but **what the traffic lights
  and the native title bar actually look like in Light and in Auto has not been
  observed**, and neither has an OS appearance switch while the app is running.
- **The window surface is written to spec and unseen.** Every window is now given
  a background colour taken from the resolved theme (see
  [Appearance](#appearance)), because an untold `NSWindow` keeps the system's
  light grey and that is the most likely source of a pale hairline around the
  outer edge of a dark window. What the pinned dependencies support was read out
  of the locked sources rather than assumed: `tao 0.35.3` calls
  `NSWindow.setBackgroundColor:` both at creation and on an existing window, so
  the runtime repaint is real; `wry 0.55.1`'s matching *webview* setter is
  implemented for iOS only and is a silent no-op on macOS, which nothing here
  relies on because the webview paints its own background in CSS. The main window
  is declared in `tauri.conf.json` and therefore exists before any Rust runs, so
  its creation-time colour is a literal in both config files, pinned to
  `tokens.css` by a test; the resolved colour reaches it from `setup`, before the
  run loop turns. None of this has been *seen*: there is no macOS and no display
  in this environment, so whether the reported hairline is gone is unverified.
  If a hairline remains, it is AppKit's own 1 px stroke around a decorated
  window — shared with Finder and every native app — and the only way to remove
  that is `decorations: false`, which costs the traffic lights and native
  edge-resize and would need custom resize zones written from scratch. That
  trade-off has deliberately not been taken.
- The **Windows** path is equally unbuilt: no MSVC link, no NSIS installer, no
  WebView2, and no WASAPI. Sample-rate following in particular depends on how a
  given endpoint behaves in shared vs exclusive mode, which cannot be guessed.
- **Real multi-window behaviour is unproven.** The detached EQ window is
  exercised end to end in the mock preview — where a browser popup stands in for
  the `WebviewWindow` — so the state machine, the analyser lifetime, the
  persistence and the cross-window indicators are all tested. What a popup
  cannot stand in for is the operating system: window restoration on launch,
  `always_on_top` pinning, Dock and Mission Control behaviour, focus stealing
  when the EQ opens, and multi-monitor placement have not been observed on
  macOS. Treat the `Float` toggle in particular as written-to-spec.
- The app has never been **run**, on any platform: this sandbox has no display
  and no audio device. Everything above is backed by unit/integration tests and
  a successful release build and `.deb` bundle, not by listening.
- **No audio device means the engine-source panel is untested against real
  hardware.** Host and device enumeration returns an empty list here, so the
  "no devices found" path is the *only* one that has been exercised; picking a
  device, fixing a rate, changing the buffer and reading back a real latency
  figure, and rebuilding the stream while playing, are covered by tests and by
  the mock preview but have never touched CoreAudio, WASAPI or ALSA.
- **MIDI has been rendered and measured, never heard.** The bundled bank loads,
  the render is decoded, its length, loudness and true peak are asserted, and
  the tail is checked for decay — but nobody has listened to a General MIDI
  file play, and the perceptual quality of the bank is not something a test can
  speak to.
- On **Linux**, the generated `.desktop` entry carries the MIME types but its
  `Exec=onyx` line has no `%U`, so a file manager's "Open with" will launch Onyx
  without passing the file. This is a Tauri bundler limitation and is not fixed,
  because Linux is not a shipping target.

**Design limits that are real and will not change soon.**

- **Whole files are decoded into RAM.** Decoded audio is held as interleaved f32
  (8 bytes per stereo frame), which makes very long files memory-hungry: the
  default budget is **1 GiB per deck** — roughly 50 min at 44.1 kHz, 46 min at
  48 kHz, 23 min at 96 kHz, 11 min at 192 kHz. Longer files are **truncated** at
  the budget and Onyx says so in a toast; the remainder is not playable. (The
  budget is a setting, clamped to 64 MiB … 8 GiB.) There is no disk-streaming
  mode, and adding one would break the things this app exists for: instant A/B
  switching at a shared position, sample-accurate offset alignment and instant
  seeking anywhere all assume the audio is resident.
- **A/B alignment is offset-only** — one constant delay, no time-stretching. It
  will not align versions that differ in tempo or edit structure. See
  [Time alignment](#time-alignment).
- **Analysis needs a complete decode.** Integrated LUFS and true peak are only
  available once a file has been decoded to the end, so level matching engages a
  moment after a long track is loaded — unless the loudness cache already has it.
- **A rate change re-decodes.** Changing device or loading a file at a different
  rate invalidates the *other* deck's PCM, and Onyx re-runs the decoder for it
  rather than resampling in place. Costs a few seconds of background work on a
  long file; the deck you are listening to is not interrupted.
- Only mono and stereo are played; more than two channels are folded to stereo.
- Meters describe the programme, not the monitor fold, by design — so while a
  fold is engaged they are not telling you what the speakers are doing. See
  [Monitor matrix](#monitor-matrix).
- No ReplayGain tag reading, no gapless playback across playlist rows, no
  cue-sheet support, no library/database. Onyx is a player and a comparison
  tool, not a music manager.
- The blind-test RNG is a clock-seeded xorshift64\*, not a CSPRNG. Fine for
  listening tests, not suitable if you need an adversary-proof sequence.

---

## Repository layout

```
onyx/
  .github/workflows/    ci.yml (every check, three platforms) · release.yml
                        (a tag -> the .dmg and the NSIS installer, on a draft
                        release) · audit.yml (cargo audit) · shots.yml (the
                        Playwright harnesses, on demand)
  .github/              issue forms, the pull-request template, dependabot
  crates/onyx-core/     audio engine: align, decode, pcm, waveform, dsp, engine
  src-tauri/            Tauri 2 app layer: state, playlist, loader, blind,
                        cache, settings, persist, archive, frame pump,
                        commands, window surface, bundle config, icons
  src/                  React + TypeScript front end (index.html entry)
  src/eq/               the detached EQ window's own entry (eq.html)
  src/theme/            the theme editor window's own entry (theme.html) — §20
  src/styles/           tokens.css (both themes, the only colours in src/) ·
                        app.css · eq.css · theme-editor.css
  crates/onyx-core/assets/gm/  the bundled General MIDI bank + its licence
  scripts/              build-mac.sh · build-windows.ps1 (the two release
                        scripts; same preflight, same order) · make-icon.py ·
                        make-format-fixtures.sh (the per-format test tones) ·
                        lib/shots-base.mjs (the browser, the shutter and the
                        exit verdict the three harnesses share) ·
                        shots.mjs (screenshots + layout/gesture assertions) ·
                        shots-theme.mjs (themes, accents, canvas-staleness,
                        the appearance-persistence seam) ·
                        shots-themedoc.mjs (the theme-document workflow,
                        end to end) ·
                        lib/mock-host.mjs (the browser stand-in and the
                        transpile step the two mock-driving checks share) ·
                        check-eq-curve.mjs (TS↔Rust curve contract) ·
                        check-ab-parity.mjs (TS↔Rust deck-assignment contract) ·
                        check-theme.mjs (the theme contract: catalogue, parser,
                        sanitiser, contrast, mock↔Rust storage parity) ·
                        check-ipc.mjs (the command surface: SPEC §3.1 ·
                        generate_handler! · api.ts · the mock's dispatch) ·
                        check-version.mjs (one version in seven places, and
                        the release gate against the tag)
  SPEC.md               the single authoritative spec: §0–§5 core contract,
                        §6–§12 v2 features, §13 diagnostics,
                        §14–§19 v3 features, §20 the theme document
  CONTRIBUTING.md       orientation for a new contributor: the layer map, the
                        checks and what they guard, the invariants and their
                        tests, the IPC / format / window-surface recipes, and
                        what is deliberately unverified
  THEMING.md            the theme engine's interface and the theme-document
                        contract: data attributes, the functions the settings
                        UI calls, the canvas palette, every themeable key
  THIRD-PARTY.md        what Onyx redistributes and under which licence
  CHANGELOG.md          what shipped in each release
  SECURITY.md           how to report a vulnerability, and what counts as one
```

Two designed themes, dark and light, plus `system`; one accent hex derives every
variant. What to call, and what not to hard-code, is in
[THEMING.md](THEMING.md).

## License

MIT — see [LICENSE](LICENSE).
