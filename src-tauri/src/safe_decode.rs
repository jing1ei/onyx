//! The boundary between Onyx and untrusted files.
//!
//! Everything the user opens — double-clicked in Finder, dropped on the window,
//! picked from the dialog, or walked out of a folder — is parsed by
//! `symphonia`, a third-party demuxer/codec stack being fed data it has never
//! seen. Two things follow from that, and both are handled here rather than
//! sprinkled through the load path:
//!
//! 1. **A parser panic must not take a thread with it.** `symphonia` asserts on
//!    some malformed input instead of returning an error — a 44-byte WAV header
//!    that declares `sample_rate = 0` reaches
//!    `TimeBase::new(1, 0)` and panics `TimeBase cannot have 0 numerator or
//!    denominator` on whichever thread called it. That thread is a probe worker
//!    (permanently one worker down, silently) or the thread servicing an IPC
//!    command. [`probe`] and [`open`] turn that into the ordinary `Err` the rest
//!    of the app already knows how to report.
//!
//! 2. **A header is a claim, not a fact.** See
//!    [`plausible_budget`] for the memory side of the same
//!    problem, and [`sanitise`] for the numbers that go on to be serialised to
//!    the webview.
//!
//! This module is `pub` so that `tests/malformed_input.rs` — the hostile-input
//! corpus — can exercise exactly the entry points the app uses.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

use onyx_core::decode::{self, DecodeHandle, DecodeOptions};
use onyx_core::TrackInfo;

/// Longest duration a container is allowed to claim, in seconds (48 hours).
///
/// Beyond this the header is not describing audio, and a playlist row reading
/// "34 years" (which a `sample_rate = 1` WAV really does produce) is worse than
/// no duration at all: it makes the seek bar and the waveform lane useless.
/// The audio itself is still played — only the claim is discarded.
///
/// This is only the outer bound; the file's own size gives a much tighter one,
/// see [`max_plausible_secs`].
const MAX_CLAIMED_SECS: f64 = 48.0 * 3600.0;

/// Ceiling on how many output frames one byte of source file may become.
///
/// `decode::open` pre-allocates the whole PCM buffer up front — that is what
/// makes playback and seeking allocation-free afterwards — and it sizes that
/// buffer from the duration the *container header* claims. A header can lie: a
/// 456-byte WAV that declares a four-gigabyte `data` chunk makes the decoder
/// commit the entire deck budget (1 GiB by default) before it has read a single
/// packet, and a dropped folder of them is an out-of-memory kill one
/// double-click wide.
///
/// So the budget handed to the decoder is bounded by what the file *could*
/// contain as well as by what the user allowed. 8192 output frames per source
/// byte is far past the densest thing Onyx can decode — a FLAC block of digital
/// silence is roughly 256 frames/byte at 16-bit and ~4700 at 8-bit with the
/// largest legal block size, Opus at its lowest bit rate about 1000 — so no
/// real file is ever capped by this. Only files whose header is fiction are.
const MAX_FRAMES_PER_SOURCE_BYTE: u64 = 8_192;

/// Bytes one stored frame costs: the decoder stores at most stereo f32.
const BYTES_PER_STORED_FRAME: u64 = 2 * 4;

/// Clamp the configured deck budget to what a `file_bytes`-long file could
/// plausibly decode to. Pure, so the arithmetic can be tested directly.
pub fn plausible_budget(configured: usize, file_bytes: u64) -> usize {
    let ceiling = file_bytes
        .saturating_mul(MAX_FRAMES_PER_SOURCE_BYTE)
        .saturating_mul(BYTES_PER_STORED_FRAME);
    configured.min(usize::try_from(ceiling).unwrap_or(usize::MAX))
}

/// Size of the file being read, or `None` if it cannot be stated. `None` means
/// "no idea", and every plausibility check falls back to its loosest bound.
fn size_of(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok().map(|m| m.len())
}

/// Read tags and stream parameters. Never panics, whatever the file contains.
///
/// Uses the default decode options; call [`probe_with`] from anywhere that has
/// the user's settings, because a `.mid` is described differently depending on
/// which SoundFont is in force (SPEC §18).
pub fn probe(path: &Path) -> Result<TrackInfo, String> {
    probe_with(path, &DecodeOptions::default())
}

/// [`probe`] with the app's decode options (the user's SoundFont).
pub fn probe_with(path: &Path, options: &DecodeOptions) -> Result<TrackInfo, String> {
    match catch_unwind(AssertUnwindSafe(|| decode::probe_with(path, options))) {
        Ok(Ok(mut info)) => {
            sanitise(&mut info, size_of(path));
            Ok(info)
        }
        Ok(Err(e)) => Err(e.to_string()),
        Err(payload) => Err(panicked(path, "probe", payload)),
    }
}

/// Start a background decode. Never panics, whatever the file contains.
///
/// Note that this only contains a panic raised while the header is being
/// parsed, which is all of `open`'s own work. A panic on the decode thread
/// itself cannot be caught from here; it is detected instead by the load
/// watcher, which treats "the PCM buffer closed but the decoder never reported
/// finished" as a failed decode (see `loader::spawn_watcher`).
pub fn open(path: &Path, target_rate: u32, budget_bytes: usize) -> Result<DecodeHandle, String> {
    open_with(path, target_rate, budget_bytes, &DecodeOptions::default())
}

/// [`open`] with the app's decode options.
///
/// For a MIDI file this is what selects the bank the render is produced
/// through, and therefore what `TrackInfo::render_key` — the loudness cache's
/// bank component — ends up being.
pub fn open_with(
    path: &Path,
    target_rate: u32,
    budget_bytes: usize,
    options: &DecodeOptions,
) -> Result<DecodeHandle, String> {
    match catch_unwind(AssertUnwindSafe(|| {
        decode::open_with(path, target_rate, budget_bytes, options)
    })) {
        Ok(Ok(mut handle)) => {
            sanitise(&mut handle.info, size_of(path));
            Ok(handle)
        }
        Ok(Err(e)) => Err(e.to_string()),
        Err(payload) => Err(panicked(path, "open", payload)),
    }
}

/// Turn a caught panic into a log line and a message for the user.
fn panicked(path: &Path, what: &str, payload: Box<dyn std::any::Any + Send>) -> String {
    let detail = payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown".into());
    let name = path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string());
    // `error`, not `warn`: a panic in the decoder is a bug somewhere, even if
    // the file that triggered it is malformed, and a bug report needs it.
    log::error!("the decoder panicked while trying to {what} \"{name}\": {detail}");
    log::debug!("panicking file was {}", path.display());
    format!("that file is malformed and could not be read ({detail})")
}

/// The longest duration a file of `file_bytes` bytes could honestly contain.
///
/// Same reasoning and same constant as [`plausible_budget`], expressed in
/// seconds: at most `MAX_FRAMES_PER_SOURCE_BYTE` decoded frames per byte on
/// disk, played at the rate the header declares. A 456-byte WAV claiming 6.2
/// hours is caught here even though 6.2 hours is, in the abstract, a length a
/// master could have.
fn max_plausible_secs(sample_rate: u32, file_bytes: Option<u64>) -> f64 {
    match (sample_rate, file_bytes) {
        (0, _) | (_, None) => MAX_CLAIMED_SECS,
        (rate, Some(bytes)) => {
            let frames = bytes.saturating_mul(MAX_FRAMES_PER_SOURCE_BYTE) as f64;
            (frames / f64::from(rate)).min(MAX_CLAIMED_SECS)
        }
    }
}

/// Make a `TrackInfo` safe to serialise and safe to display.
///
/// Every field here crosses the IPC boundary as JSON. `serde_json` writes a
/// non-finite float as `null`, which arrives in TypeScript as `null` in a slot
/// the `TrackInfo` interface declares as `number` — the compiler cannot catch
/// it and the UI renders `NaN`. A container that declares `sample_rate = 1`
/// and a four-gigabyte data chunk is enough to produce one.
///
/// `file_bytes` is the size on disk, used to bound the duration claim.
fn sanitise(info: &mut TrackInfo, file_bytes: Option<u64>) {
    let limit = max_plausible_secs(info.sample_rate, file_bytes);
    if !info.duration_secs.is_finite() || info.duration_secs < 0.0 {
        info.duration_secs = 0.0;
    } else if info.duration_secs > limit {
        log::warn!(
            "\"{}\" claims to be {:.0} s long, which {} bytes cannot contain; ignoring the claim",
            info.file_name,
            info.duration_secs,
            file_bytes.map_or_else(|| "?".to_string(), |b| b.to_string())
        );
        info.duration_secs = 0.0;
    }
    if let Some(kbps) = info.bitrate_kbps {
        if kbps == 0 {
            info.bitrate_kbps = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /* ── the memory budget against a lying header (SPEC §9.3) ─────────── */

    #[test]
    fn a_tiny_file_cannot_commit_the_whole_deck_budget() {
        let gib = 1024 * 1024 * 1024usize;
        // The real case: a 456-byte WAV whose `data` chunk claims 4 GiB. Before
        // this clamp, `decode::open` pre-allocated the full gigabyte for it.
        let budget = plausible_budget(gib, 456);
        assert!(
            budget <= 32 * 1024 * 1024,
            "456 bytes were allowed a {budget} byte buffer"
        );
        // An empty file gets nothing; `decode::open` still floors the buffer at
        // one second, which is all a zero-byte file could ever need.
        assert_eq!(plausible_budget(gib, 0), 0);
    }

    #[test]
    fn a_real_file_is_never_capped_by_the_plausibility_clamp() {
        let gib = 1024 * 1024 * 1024usize;
        // The densest real inputs Onyx sees, sized as they actually arrive:
        // a 3-minute 128 kbps MP3, a 4-minute 24/96 FLAC, a 60-minute 24/96
        // WAV master. None of them may lose a sample to this clamp.
        for (what, bytes) in [
            ("128 kbps MP3, 3 min", 2_880_000u64),
            ("24/96 FLAC, 4 min", 100_000_000),
            ("24/96 WAV master, 60 min", 2_073_600_000),
        ] {
            assert_eq!(
                plausible_budget(gib, bytes),
                gib,
                "{what} was clamped below the configured budget"
            );
        }
        // The clamp only ever lowers, never raises.
        assert_eq!(
            plausible_budget(64 * 1024 * 1024, u64::MAX),
            64 * 1024 * 1024
        );
    }

    #[test]
    fn the_plausibility_clamp_cannot_overflow() {
        // `u64::MAX` bytes is not a file, but `fs::metadata` failing is how the
        // load path says "I have no idea how big this is", and the arithmetic
        // has to survive it on a 32-bit target too.
        assert_eq!(plausible_budget(usize::MAX, u64::MAX), usize::MAX);
        assert_eq!(plausible_budget(0, u64::MAX), 0);
    }

    fn info(duration_secs: f64) -> TrackInfo {
        TrackInfo {
            duration_secs,
            sample_rate: 48_000,
            channels: 2,
            ..TrackInfo::default()
        }
    }

    #[test]
    fn non_finite_durations_never_reach_the_webview() {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0] {
            let mut i = info(bad);
            sanitise(&mut i, None);
            assert_eq!(i.duration_secs, 0.0, "{bad} survived");
        }
        // The concrete failure this prevents: serde_json turns a non-finite
        // float into `null`, and `durationSecs` is a `number` in
        // src/lib/types.ts.
        let json = serde_json::to_string(&info(f64::INFINITY)).unwrap();
        assert!(json.contains("\"durationSecs\":null"), "{json}");
        let mut clean = info(f64::INFINITY);
        sanitise(&mut clean, None);
        let json = serde_json::to_string(&clean).unwrap();
        assert!(json.contains("\"durationSecs\":0.0"), "{json}");
    }

    #[test]
    fn an_impossible_duration_claim_is_discarded_not_displayed() {
        // A `sample_rate = 1` WAV with a 4 GiB data chunk really does probe as
        // 1 073 741 807 seconds — 34 years.
        let mut i = info(1_073_741_807.0);
        sanitise(&mut i, None);
        assert_eq!(i.duration_secs, 0.0);
        // Anything a human could actually be mastering is left alone.
        let mut i = info(3.5 * 3600.0);
        sanitise(&mut i, None);
        assert_eq!(i.duration_secs, 3.5 * 3600.0);
    }

    #[test]
    fn a_duration_longer_than_the_file_could_hold_is_discarded() {
        // The 456-byte WAV with a four-gigabyte `data` chunk: symphonia
        // believes the header and reports 6 h 12 m, which is a perfectly
        // ordinary length — only the file size gives it away.
        let mut i = info(22_369.6);
        sanitise(&mut i, Some(456));
        assert_eq!(i.duration_secs, 0.0);
        // A real 3-minute 128 kbps MP3 keeps its duration.
        let mut i = info(180.0);
        sanitise(&mut i, Some(2_880_000));
        assert_eq!(i.duration_secs, 180.0);
        // So does a heavily compressed one: 8 kbps Opus, 3 minutes, 180 KB.
        let mut i = info(180.0);
        sanitise(&mut i, Some(180_000));
        assert_eq!(i.duration_secs, 180.0);
        // An unknown size falls back to the outer bound only.
        let mut i = info(6.0 * 3600.0);
        sanitise(&mut i, None);
        assert_eq!(i.duration_secs, 6.0 * 3600.0);
    }

    #[test]
    fn a_zero_bitrate_is_no_bitrate() {
        let mut i = TrackInfo {
            bitrate_kbps: Some(0),
            ..info(10.0)
        };
        sanitise(&mut i, None);
        assert_eq!(
            i.bitrate_kbps, None,
            "`0 kbps` is a badge that means nothing"
        );
    }

    /// The regression this module exists for: `symphonia` 0.5.5 panics rather
    /// than returning an error when a WAV declares a zero sample rate, and that
    /// panic used to travel up whichever thread was probing.
    #[test]
    fn a_zero_sample_rate_header_is_an_error_not_a_panic() {
        let mut wav: Vec<u8> = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&40u32.to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&2u16.to_le_bytes()); // stereo
        wav.extend_from_slice(&0u32.to_le_bytes()); // 0 Hz
        wav.extend_from_slice(&0u32.to_le_bytes());
        wav.extend_from_slice(&4u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&4u32.to_le_bytes());
        wav.extend_from_slice(&[0u8; 4]);

        let mut path = std::env::temp_dir();
        path.push(format!("onyx-zero-rate-{}.wav", std::process::id()));
        std::fs::write(&path, &wav).unwrap();

        let e = probe(&path).expect_err("a 0 Hz header cannot describe audio");
        assert!(e.contains("malformed"), "{e}");
        let e = match open(&path, 48_000, 1024 * 1024) {
            Ok(_) => panic!("a 0 Hz header cannot be opened"),
            Err(e) => e,
        };
        assert!(e.contains("malformed"), "{e}");
        let _ = std::fs::remove_file(&path);
    }
}
