# Changelog

All notable changes to Onyx are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and Onyx uses
[semantic versioning](https://semver.org/spec/v2.0.0.html) — for a desktop
application that means the major number moves when a session, a saved theme or
the persisted settings from an older version stop being read.

## [Unreleased]

Nothing yet.

## [1.0.0] — 2026-09-06

The first public release. Everything below is what Onyx does, not what changed
— there is no earlier version to compare it against.

### Playback

- Bit-transparent playback: no resampling, no dither, no hidden gain. When the
  device can be asked for the file's own sample rate, it is; the badge tells you
  when the path is transparent and when it is not.
- WAV/BWF, FLAC, MP3, AAC/M4A, ALAC, Vorbis, Opus, AIFF, CAF, Matroska (MKA),
  WebM and the audio track of MOV/MP4, decoded by `symphonia` — pure Rust, no
  ffmpeg to ship or license.
- `.mid` / `.midi` rendered through a bundled General MIDI SoundFont
  (GeneralUser GS) and then travelling the ordinary decoded-audio path.
- A `.zip` opens as a playlist: guarded extraction with limits on total size,
  entry count, nesting and compression ratio.
- Sample-rate following, with the host, device, rate and buffer size selectable
  and the achieved latency reported back.

### A/B comparison

- Two decks sharing one playhead: switching is instant and does not move the
  position.
- Four routes onto deck B — drag a row onto a lane, the row chips, `⇧B`, or a
  file dropped on a lane.
- Sample-accurate offset alignment, automatic or by Alt-drag, for versions that
  do not start together.
- LUFS-based level matching that only ever attenuates, so a match can never
  make the louder side clip.
- Blind testing: 2AFC and ABX, with a per-trial randomised mapping and an exact
  binomial p-value.

### Metering and analysis

- BS.1770-4 / EBU R128 loudness — momentary, short-term and integrated — with
  true-peak metering to the standard's oversampling.
- Correlation, a monitor matrix (mono, side, L/R solo, polarity), and a spectrum
  analyser.
- A persistent, schema-versioned loudness cache so a file measured once is
  measured once.
- Interactive EQ in a window of its own: RBJ biquads, an analyser backdrop, and
  band solo.

### Appearance

- Two designed themes, dark and light, plus `system`; one accent colour derives
  every variant.
- A theme *document*: hand the whole look to a language model as JSONC, paste
  the answer back, and get contrast diagnostics on what it wrote.

### Platforms

- macOS 11+ universal (Apple silicon + Intel) — the primary target.
- Windows 10/11 x64 — NSIS installer, per-user, registering the file
  associations.
- Linux builds and runs for development; the `.deb` is a smoke test of the
  bundle configuration, not a shipping target.

### Known at release

The engine and the app layer are covered by the test suite in this tree and by
a full release build. The **macOS `.dmg` / universal / notarisation path and the
Windows NSIS / WASAPI path had never been built or run** when this version was
tagged, and the application had never been *run* on any platform: the
environment it was written in has no display and no audio device. Read the
README's [Known limitations](README.md#known-limitations) before trusting a
build, and treat this release as what it is — a first one.

[Unreleased]: https://github.com/jing1ei/onyx/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/jing1ei/onyx/releases/tag/v1.0.0
