# Third-party components bundled with Onyx

Everything Onyx ships that was not written for Onyx, with the licence it
travels under. Dependencies pulled from crates.io / npm are recorded in
`Cargo.lock` and `package-lock.json`; this file covers the ones that need a
human decision, i.e. redistributed **data** and anything whose licence imposes
an obligation on the binary we ship.

## GeneralUser GS v2.0.3 — bundled General MIDI bank

| | |
|---|---|
| File | `crates/onyx-core/assets/gm/GeneralUser-GS.sf2` |
| Size | 32,319,396 bytes (30.8 MiB) |
| SHA-256 | `9575028c7a1f589f5770fccc8cff2734566af40cd26ed836944e9a5152688cfe` |
| Author | S. Christian Collins |
| Source | <https://github.com/mrbumpy409/GeneralUser-GS> |
| Licence | GeneralUser GS License v2.0 — `crates/onyx-core/assets/gm/GeneralUser-GS-LICENSE.txt` |

Used by SPEC §18: `.mid` / `.midi` files are rendered through this bank by
`rustysynth` and then travel the ordinary decoded-audio path.

The bank provides compact General MIDI coverage and can be replaced with a
user-supplied `.sf2` in Settings.

**Licence terms, in short.** Unrestricted use in music creation, private or
commercial; explicit permission to use it in software projects and to modify
the bank or its packaging. No attribution is demanded, no copyleft, no fee.
This notice is here because it is good practice, not because the licence
requires it.

**The caveat, stated plainly.** The author records that GeneralUser GS began as
a personal project, that many samples are original but some were taken from
banks freely available on the web in the 1990s, and that he "cannot be 100%
sure where all of the samples originated" — while also stating that none came
from commercially published SoundFont packages or sample CDs, and that no
ownership complaint has been received since the bank's publication in 2000.
That residual uncertainty is inherited by anything that bundles it, Onyx
included. It is accepted here as the best available trade-off between coverage,
size and licence clarity; if it is ever not acceptable, the bank is one file
and one constant (`midi::BUNDLED_BANK_ID`) away from being replaced, and users
can already point Onyx at their own `.sf2` in Settings.

The bank is embedded in the binary. Its license is included as a bundle
resource. Preserve both the asset and its license when redistributing Onyx.

## Rust crates worth naming

| Crate | Licence | Why it is here |
|---|---|---|
| `rustysynth` 1.3 | MIT | Pure-Rust SoundFont synthesiser — SPEC §18 |
| `opus-decoder` 0.1 | MIT OR Apache-2.0 | Pure-Rust Opus decoder; Symphonia demuxes Ogg/Opus but ships no Opus decoder — SPEC §17 |
| `symphonia` 0.5 | MPL-2.0 | Demuxing and decoding for every other format. Used unmodified, as a library; MPL-2.0's obligations attach to modified Symphonia source files, of which there are none |
| `cpal` 0.15 | Apache-2.0 | Output device access |
| `rubato` 0.16 | MIT | Sample-rate conversion |

## Test fixtures

`crates/onyx-core/tests/fixtures/tone.*` are half-second sine tones generated
by `scripts/make-format-fixtures.sh` with ffmpeg. They contain no third-party
material.
