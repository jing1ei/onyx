//! Gapless boundaries: exact lengths and no timing shift (SPEC §17).
//!
//! Lossy formats carry encoder priming at the head and padding at the tail, and
//! the container is supposed to say how much of each to throw away. Get it
//! wrong and a file is a few milliseconds long, starts a few milliseconds late,
//! and every loop point, A/B comparison and gapless transition inherits the
//! error. None of that is subtle to a mastering engineer: it is a click at the
//! loop and a flam against the other deck.
//!
//! The fixtures are 440 Hz (left) and 660 Hz (right) sine waves starting at
//! phase zero, so both halves of "gapless" can be checked exactly: the frame
//! count must be the nominal duration to the sample, and the decoded waveform
//! must line up with a sine generated from scratch with no offset at all.

use std::f64::consts::TAU;
use std::path::{Path, PathBuf};

use onyx_core::decode::{open, probe, DecodeHandle, DEFAULT_DECK_BUDGET_BYTES};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn decode(name: &str) -> DecodeHandle {
    let path = fixture(name);
    assert!(path.exists(), "missing fixture {name}");
    let h = open(&path, 48_000, DEFAULT_DECK_BUDGET_BYTES).expect("open");
    for _ in 0..2_000 {
        if h.status.is_finished() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(h.status.is_finished(), "{name}: decode thread hung");
    assert!(h.status.error().is_none(), "{name}: {:?}", h.status.error());
    h
}

/// Integer offset, in frames, that best lines the decoded left channel up with
/// a sine of `hz` starting at phase zero.
///
/// The search window is under half a period, so the answer is unambiguous: a
/// periodic signal correlates with itself at every whole period, and a window
/// this size cannot reach the next one.
fn best_offset(h: &DecodeHandle, hz: f64) -> i64 {
    let frames = h.pcm.frames_ready().min(9_600);
    let window = 48 / 2 - 4;
    let mut best = (f64::NEG_INFINITY, i64::MAX);
    for shift in -window..=window {
        let mut acc = 0.0;
        for i in 0..frames {
            let idx = i as i64 + shift;
            if idx < 0 || idx as usize >= h.pcm.frames_ready() {
                continue;
            }
            let s = h.pcm.frame_stereo(idx as usize)[0] as f64;
            acc += s * (TAU * hz * i as f64 / 48_000.0).sin();
        }
        if acc > best.0 {
            best = (acc, shift);
        }
    }
    best.1
}

/// Ogg Opus, one page. The Opus pre-skip (312 frames, 6.5 ms) lives in
/// `OpusHead`; the demuxer never trims it because the only audio page is also
/// the first page, and its own arithmetic mistakes the end padding for the
/// start delay. Without honouring the header the file is 6.5 ms long and every
/// sample in it is 6.5 ms early.
///
/// This fixture also carries the one packet the third-party Opus
/// implementation cannot decode, so an exact length here doubles as proof that
/// concealment replaced it with silence of the right duration instead of
/// dropping it and pulling the rest of the file forward.
#[test]
fn ogg_opus_is_sample_exact_and_starts_on_time() {
    let info = probe(&fixture("tone.opus")).unwrap();
    assert!(
        (info.duration_secs - 0.5).abs() < 1e-9,
        "probe says {} s, not 0.5",
        info.duration_secs
    );

    let h = decode("tone.opus");
    assert_eq!(
        h.pcm.frames_ready(),
        24_000,
        "0.5 s at 48 kHz to the sample"
    );
    assert_eq!(best_offset(&h, 440.0), 0, "the pre-skip was not removed");
    assert!(
        (h.duration_secs() - 0.5).abs() < 1e-9,
        "decoded length {} s",
        h.duration_secs()
    );
}

/// The same, on a stream spread over several Ogg pages: there the demuxer
/// *does* mark the trims, and the decoder has to apply them rather than double
/// up with the pre-skip handling above.
#[test]
fn multi_page_opus_is_sample_exact() {
    let h = decode("gapless-multipage.opus");
    assert_eq!(h.pcm.frames_ready(), 120_000, "2.5 s at 48 kHz");
    assert_eq!(best_offset(&h, 440.0), 0);
}

/// Ogg Vorbis states no pre-skip in band, so the demuxer's derived `delay` is
/// the only clue — and on a single-page stream it is the *end* padding wearing
/// the wrong hat. Trimming the head on that guess would eat 128 frames of real
/// audio, so only the declared length is applied. This pins both halves: the
/// exact length, and a head that still begins at phase zero.
#[test]
fn ogg_vorbis_keeps_its_first_frames() {
    let h = decode("tone.ogg");
    assert_eq!(h.pcm.frames_ready(), 24_000);
    assert_eq!(best_offset(&h, 440.0), 0, "the head was trimmed away");
}

/// MP3's delay and padding are marked packet by packet by the demuxer and
/// applied by its decoder. Nothing here changed that — this is the control
/// that proves the new bounds do not double-trim a format that was already
/// right.
#[test]
fn mp3_gapless_is_unchanged() {
    let h = decode("tone.mp3");
    assert_eq!(h.pcm.frames_ready(), 24_000);
    assert_eq!(best_offset(&h, 440.0), 0);
}

/// The lossless reference: no priming, no padding, no bounds to apply.
#[test]
fn lossless_is_the_reference() {
    for name in ["tone.flac", "tone-alac.m4a"] {
        let h = decode(name);
        assert_eq!(h.pcm.frames_ready(), 24_000, "{name}");
        assert_eq!(best_offset(&h, 440.0), 0, "{name}");
    }
}

/// Root-mean-square of one channel over `frames` frames from `start`.
fn rms(h: &DecodeHandle, channel: usize, start: usize, frames: usize) -> f64 {
    let end = (start + frames).min(h.pcm.frames_ready());
    assert!(end > start, "no frames to measure");
    let sum: f64 = (start..end)
        .map(|i| {
            let s = h.pcm.frame_stereo(i)[channel] as f64;
            s * s
        })
        .sum();
    (sum / (end - start) as f64).sqrt()
}

/// AAC in MP4/MOV: the priming is in the edit list, and it has to be honoured.
///
/// An AAC encoder emits 1024 frames of lead-in before the first real sample and
/// the muxer does not remove them; it writes `edts/elst` on the track saying
/// "start 1024 samples in and last 0.5 s". Ignore it and the tone starts 21 ms
/// late and the file is 33 ms long — an A/B against the same master in another
/// container is then a flam, and the level match is measured over 33 ms of the
/// wrong thing.
#[test]
fn aac_in_mp4_honours_the_edit_list() {
    for name in ["tone-aac.m4a", "tone-video.mp4", "tone-video.mov"] {
        let info = probe(&fixture(name)).unwrap();
        assert!(
            (info.duration_secs - 0.5).abs() < 1e-9,
            "{name}: probe says {} s, not 0.5",
            info.duration_secs
        );
        let h = decode(name);
        assert_eq!(h.pcm.frames_ready(), 24_000, "{name}: 0.5 s at 48 kHz");
        // The priming frames are silent, so the head being as loud as the
        // middle is what proves they were dropped rather than played.
        let head = rms(&h, 0, 0, 512);
        let middle = rms(&h, 0, 12_000, 512);
        assert!(
            head > middle * 0.5,
            "{name}: head is {head:.4} against {middle:.4} in the middle — \
             the encoder lead-in is still there"
        );
        // Fine alignment, within a frame: AAC's window overlap smears the first
        // samples of the tone, so a lossy codec framed in 1024-frame blocks is
        // allowed to land one sample either side. The 1024-frame priming this
        // test is really about is well outside the ±20 frame search window
        // above; what catches *that* is the exact frame count and the head
        // level.
        let offset = best_offset(&h, 440.0);
        assert!(offset.abs() <= 1, "{name}: {offset} frames off phase");
    }
}

/// Matroska and WebM keep a little encoder padding, and this pins how much.
///
/// Both containers state their trims per block, in `DiscardPadding`, and
/// Symphonia's Matroska reader does not surface it — there is no `trim_end` on
/// the packets and no `padding` in the codec parameters, so the only bound
/// available is the segment duration, which *includes* the padding. What is
/// left is the encoder's tail: 72 frames (1.5 ms) of Opus in WebM and 128
/// frames (2.7 ms) of Vorbis in MKA, both of them the decayed end of the tone
/// rather than a timing shift — the heads are still exactly on phase, which is
/// what a comparison depends on.
///
/// Written down as an exact number rather than a tolerance on purpose: if a
/// future Symphonia starts reporting `DiscardPadding`, or someone teaches the
/// decode path to read it, this test fails and says so instead of quietly
/// changing the length of everybody's WebM files.
#[test]
fn matroska_tail_padding_is_a_known_bound() {
    for (name, frames) in [("tone.webm", 24_072), ("tone.mka", 24_128)] {
        let h = decode(name);
        assert_eq!(
            h.pcm.frames_ready(),
            frames,
            "{name}: known padding bound changed"
        );
        assert_eq!(best_offset(&h, 440.0), 0, "{name}: the head moved");
        // Whatever is left over is the tail of the tone, not silence appended
        // to it and not a second copy of anything.
        let overhang = h.pcm.frames_ready() - 24_000;
        assert!(
            overhang < 240,
            "{name}: {overhang} frames is more than 5 ms"
        );
    }
}
