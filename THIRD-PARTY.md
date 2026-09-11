# Third-party components bundled with Onyx

## FFmpeg 8.0 / FFprobe — Windows audio conversion tools

Windows installer and portable distributions include Gyan's unmodified GPLv3 FFmpeg 8.0 full-build executables. See [FFmpeg notice](vendor/ffmpeg/NOTICE.md), [license](vendor/ffmpeg/LICENSE-GPLv3.txt), and [upstream build information](vendor/ffmpeg/UPSTREAM-README.txt). Onyx invokes these as external processes. A matching FFmpeg source archive is available alongside the binary downloads in Releases.

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

**Why this bank.** It is the standard *compact* General MIDI set: complete
melodic coverage (all 128 GM programs in bank 0) plus 13 drum kits, in ~31 MB.
The obvious alternatives are either far larger (FreePats' GM set is 307 MiB
uncompressed, and incomplete), GPL-3.0 (incompatible with shipping inside a
permissively licensed app), or too small to be usable — the 4 MB `hl4mgm.sf2`
was tried first and fails `rustysynth`'s SoundFont sanity check outright.

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

**Bundle cost.** The bank is linked into the executable with `include_bytes!`,
so it is not a loose file in the installer: it adds its own ~30.8 MiB to the
binary, which takes the Linux release binary to roughly **45 MiB** (about 13 MiB
without the bank) and the `.deb` to roughly **33 MiB** (about 5 MiB without it).
Those four figures are rounded on purpose. The only exact size in this file is
the asset's own, because the asset is *in the repository* and can be checked in
one command:

```bash
shasum -a 256 crates/onyx-core/assets/gm/GeneralUser-GS.sf2   # or sha256sum
wc -c        crates/onyx-core/assets/gm/GeneralUser-GS.sf2
```

Binary and package sizes are build outputs: they move with every dependency
bump and no test in this tree asserts them, so a byte count for them in prose is
a claim that goes stale silently. Read them off the artifact instead —
`ls -l target/release/bundle/deb/*.deb` after `npx tauri build --bundles deb`,
which is also the check that the bundle configuration still parses.

The licence text ships alongside as a bundle resource
(`/usr/lib/Onyx/GeneralUser-GS-LICENSE.txt` on Linux). Verified in the produced
package: the RIFF/`sfbk` blob inside `usr/bin/onyx` is byte-identical to the
asset above (same 32,319,396 bytes, same SHA-256), with the bank's own `INAM`
reading `GeneralUser GS 2.0.3`. Nothing else in the app changes size.

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
