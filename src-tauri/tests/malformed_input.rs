//! Hostile-input tests for the open / probe / decode path (SPEC §9.3, §9.6).
//!
//! Onyx opens whatever a user double-clicks, so `symphonia` is handed untrusted
//! container and codec data on the very first interaction with the app. This
//! file is the adversary: random bytes, truncated files, headers that lie about
//! rate / channels / duration, files whose extension lies about their contents,
//! metadata bombs, and a file that is replaced underneath a running decode.
//!
//! The contract every case is measured against is the same one the user sees:
//!
//! * **no panic** — including on the decode thread, which is a detached thread
//!   whose panic would otherwise be invisible (see [`PanicWatch`]);
//! * **no hang** — every decode has to reach `finished` inside a deadline;
//! * **no unbounded allocation** — the pre-allocated PCM buffer is capped by the
//!   memory budget *and* by what the file could plausibly contain, so a 200-byte
//!   file that claims to be ten hours long may not commit a gigabyte;
//! * **a clean outcome** — either an `Err` from `open`, or a handle that reaches
//!   `is_complete()` with an honest frame count. Never a half-open deck.
//!
//! It lives in `src-tauri` rather than in `onyx-core` because it is an
//! application-level guarantee about the files this application accepts, and it
//! exercises `onyx-core` exactly the way `loader::load` does.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Once;
use std::time::{Duration, Instant};

use onyx_core::decode::{DecodeHandle, DEFAULT_DECK_BUDGET_BYTES, SUPPORTED_EXTENSIONS};
// The app-layer entry points, not `onyx_core::decode` directly: `safe_decode`
// is what every load path in Onyx actually calls, and containing a third-party
// parser's panic is part of the behaviour under test.
use onyx_lib::safe_decode::{open, plausible_budget, probe};

/// Long enough for a slow CI box, short enough that a genuine hang fails the
/// run instead of wedging it.
const DECODE_DEADLINE: Duration = Duration::from_secs(20);

/* ── panic accounting ────────────────────────────────────────────────────── */

/// `symphonia` 0.5.5 asserts rather than returning an error on some malformed
/// input — a WAV header declaring `sample_rate = 0` reaches `TimeBase::new(1, 0)`
/// and panics. `safe_decode` catches those, which is the behaviour under test,
/// but the panic hook still runs and would bury the test output under hundreds
/// of identical backtrace headers.
///
/// So: expected panics (anything raised inside the `symphonia` crates) are
/// counted and silenced; everything else — including this file's own assertion
/// failures — goes to the normal hook so the harness still reports it.
///
/// A panic on the *decode thread* cannot be caught at all. It is detected
/// instead by [`settled`], which times out because a panicked decode thread
/// never sets `finished`.
struct PanicWatch;

static CAUGHT: AtomicUsize = AtomicUsize::new(0);
static HOOK: Once = Once::new();

impl PanicWatch {
    fn install() -> PanicWatch {
        HOOK.call_once(|| {
            let previous = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                let in_symphonia = info
                    .location()
                    .map(|l| l.file().contains("symphonia"))
                    .unwrap_or(false);
                if in_symphonia {
                    CAUGHT.fetch_add(1, Ordering::Relaxed);
                    return;
                }
                previous(info);
            }));
        });
        PanicWatch
    }

    /// How many third-party panics the corpus has provoked so far. Shared
    /// across tests, so only ever asserted as "more than none".
    fn caught_in_symphonia(&self) -> usize {
        CAUGHT.load(Ordering::Relaxed)
    }
}

/* ── scratch files ───────────────────────────────────────────────────────── */

static SEQ: AtomicUsize = AtomicUsize::new(0);

/// A scratch file that deletes itself, so a failing assertion cannot leave a
/// few hundred forged files in the temp directory.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let mut p = std::env::temp_dir();
        p.push(format!("onyx-fuzz-{}-{n}-{name}", std::process::id()));
        Scratch(p)
    }

    fn with(name: &str, bytes: &[u8]) -> Scratch {
        let s = Scratch::new(name);
        std::fs::write(&s.0, bytes).expect("write scratch file");
        s
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/* ── forged WAV files ────────────────────────────────────────────────────── */

/// Everything a WAV header can lie about. Real writers cannot produce most of
/// these; a hostile or damaged file can produce all of them.
#[derive(Clone, Copy)]
struct WavHeader {
    format_tag: u16,
    channels: u16,
    sample_rate: u32,
    bits: u16,
    /// `data` chunk length to *declare*, independent of what is written.
    declared_data_len: Option<u32>,
    /// `RIFF` length to declare, independent of the real file length.
    declared_riff_len: Option<u32>,
}

impl Default for WavHeader {
    fn default() -> Self {
        WavHeader {
            format_tag: 1,
            channels: 2,
            sample_rate: 48_000,
            bits: 16,
            declared_data_len: None,
            declared_riff_len: None,
        }
    }
}

/// Build a RIFF/WAVE file with an arbitrary header, arbitrary extra chunks
/// before `data`, and an arbitrary payload.
fn forge_wav(header: WavHeader, extra_chunks: &[(&[u8; 4], Vec<u8>)], payload: &[u8]) -> Vec<u8> {
    let mut chunks: Vec<u8> = Vec::new();
    chunks.extend_from_slice(b"fmt ");
    chunks.extend_from_slice(&16u32.to_le_bytes());
    chunks.extend_from_slice(&header.format_tag.to_le_bytes());
    chunks.extend_from_slice(&header.channels.to_le_bytes());
    chunks.extend_from_slice(&header.sample_rate.to_le_bytes());
    let block_align = header.channels.saturating_mul(header.bits / 8).max(1);
    let byte_rate = header
        .sample_rate
        .saturating_mul(u32::from(block_align))
        .to_le_bytes();
    chunks.extend_from_slice(&byte_rate);
    chunks.extend_from_slice(&block_align.to_le_bytes());
    chunks.extend_from_slice(&header.bits.to_le_bytes());

    for (id, body) in extra_chunks {
        chunks.extend_from_slice(*id);
        chunks.extend_from_slice(&(body.len() as u32).to_le_bytes());
        chunks.extend_from_slice(body);
        if body.len() % 2 == 1 {
            chunks.push(0);
        }
    }

    chunks.extend_from_slice(b"data");
    let declared = header.declared_data_len.unwrap_or(payload.len() as u32);
    chunks.extend_from_slice(&declared.to_le_bytes());
    chunks.extend_from_slice(payload);

    let mut out = Vec::with_capacity(chunks.len() + 12);
    out.extend_from_slice(b"RIFF");
    let riff_len = header
        .declared_riff_len
        .unwrap_or((chunks.len() + 4) as u32);
    out.extend_from_slice(&riff_len.to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(&chunks);
    out
}

/// `secs` of 16-bit stereo silence-with-a-tone, as raw `data` bytes.
fn pcm16(sample_rate: u32, channels: u16, secs: f32) -> Vec<u8> {
    let frames = (sample_rate as f32 * secs) as usize;
    let mut out = Vec::with_capacity(frames * channels as usize * 2);
    for i in 0..frames {
        let s = (0.4 * (i as f32 * 0.05).sin() * 32_767.0) as i16;
        for _ in 0..channels {
            out.extend_from_slice(&s.to_le_bytes());
        }
    }
    out
}

fn healthy_wav(secs: f32) -> Vec<u8> {
    forge_wav(WavHeader::default(), &[], &pcm16(48_000, 2, secs))
}

/* ── the contract ────────────────────────────────────────────────────────── */

/// Wait for a decode to settle. Returns `false` on timeout (a hang, or a
/// panicked decode thread).
fn settled(handle: &DecodeHandle) -> bool {
    let deadline = Instant::now() + DECODE_DEADLINE;
    while Instant::now() < deadline {
        if handle.status.is_finished() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

/// Open `path`, and if it opens, hold the whole decode to the contract.
///
/// `configured` is the user's deck budget; the effective budget is derived from
/// it exactly the way `loader::load` derives it, so the bound asserted here is
/// the bound the shipping app applies.
///
/// Returns the handle when one was produced, so a caller can make extra
/// assertions about it.
fn must_survive(
    path: &Path,
    target_rate: u32,
    configured: usize,
    what: &str,
) -> Option<DecodeHandle> {
    let file_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(u64::MAX);
    let budget = plausible_budget(configured, file_bytes);
    let handle = match open(path, target_rate, budget) {
        Ok(h) => h,
        // A refusal is a perfectly good outcome — that is the clean error the
        // user is meant to see.
        Err(e) => {
            assert!(!e.is_empty(), "{what}: refused with an empty message");
            return None;
        }
    };

    // The allocation is committed by `open`, before a single packet has been
    // read, so it is bounded here rather than after the decode.
    let committed =
        handle.pcm.capacity_frames() * handle.pcm.channels() * std::mem::size_of::<f32>();
    // `decode::open` floors the buffer at one second so that a tiny budget
    // still produces a playable deck; that floor is the only thing allowed
    // above the budget.
    let floor = target_rate as usize * 2 * std::mem::size_of::<f32>();
    assert!(
        committed <= budget.max(floor),
        "{what}: committed {committed} bytes against a {budget} byte budget"
    );

    assert!(settled(&handle), "{what}: decode never finished");
    assert!(
        handle.pcm.is_complete(),
        "{what}: decode finished but the buffer was left open"
    );
    assert!(
        handle.pcm.frames_ready() <= handle.pcm.capacity_frames(),
        "{what}: published more frames than were allocated"
    );
    // Whatever happened, the deck is either playable or carries an error — it
    // may not be silently half-populated.
    if handle.status.failed() {
        assert!(
            handle.status.error().is_some(),
            "{what}: failed with no message for the user"
        );
    }
    // Reading past the end is silence, not undefined behaviour.
    assert_eq!(handle.pcm.frame_stereo(usize::MAX), [0.0, 0.0]);
    assert!(
        handle.duration_secs().is_finite() && handle.duration_secs() >= 0.0,
        "{what}: duration is {}",
        handle.duration_secs()
    );
    assert!(
        handle.info.duration_secs.is_finite(),
        "{what}: TrackInfo.duration_secs is {} — it is serialised straight to \
         the webview, where a non-finite float becomes JSON `null`",
        handle.info.duration_secs
    );
    Some(handle)
}

/* ── 1. random bytes ─────────────────────────────────────────────────────── */

/// SplitMix64 — deterministic, so a failure is reproducible from the seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

#[test]
fn random_bytes_under_every_supported_extension_are_refused_cleanly() {
    let _watch = PanicWatch::install();
    let mut rng = Rng(0x0117_5EED_D15C_A11E);
    for case in 0..400 {
        let ext = SUPPORTED_EXTENSIONS[rng.below(SUPPORTED_EXTENSIONS.len())];
        let len = rng.below(4_096);
        let mut bytes = vec![0u8; len];
        for b in bytes.iter_mut() {
            *b = rng.next() as u8;
        }
        // Half the corpus gets a real container magic in front, which is what
        // actually gets past the probe and into a codec.
        if case % 2 == 0 && len >= 16 {
            let magic: &[u8] = match case % 8 {
                0 => b"RIFF",
                2 => b"fLaC",
                4 => b"OggS",
                _ => b"\xFF\xFB\x90\x00",
            };
            bytes[..magic.len()].copy_from_slice(magic);
            if case % 8 == 0 {
                bytes[8..12].copy_from_slice(b"WAVE");
            }
        }
        let f = Scratch::with(&format!("random.{ext}"), &bytes);
        // `probe` is what the playlist runs on every dropped file.
        let _ = probe(f.path());
        must_survive(
            f.path(),
            48_000,
            8 * 1024 * 1024,
            &format!("random case {case}"),
        );
    }
}

/* ── 2. truncation ───────────────────────────────────────────────────────── */

#[test]
fn a_valid_file_truncated_anywhere_yields_what_survived() {
    let _watch = PanicWatch::install();
    let whole = healthy_wav(1.0);
    for cut in (0..whole.len()).step_by(whole.len() / 24) {
        let f = Scratch::with("cut.wav", &whole[..cut]);
        if let Some(h) = must_survive(f.path(), 48_000, 64 * 1024 * 1024, &format!("cut at {cut}"))
        {
            // Never more audio than the bytes that are actually there.
            let max_frames = cut / 4 + 1;
            assert!(
                h.pcm.frames_ready() <= max_frames,
                "cut at {cut} produced {} frames out of {max_frames} possible",
                h.pcm.frames_ready()
            );
        }
    }
    // ... and the same for the byte *after* every chunk boundary, which is
    // where a container parser is most likely to trust a length it has just
    // read and then read past the end of the file.
    for cut in [4, 8, 12, 16, 20, 36, 40, 43, 44, 45] {
        let f = Scratch::with("cut-boundary.wav", &whole[..cut.min(whole.len())]);
        must_survive(
            f.path(),
            48_000,
            64 * 1024 * 1024,
            &format!("boundary cut {cut}"),
        );
    }
}

#[test]
fn a_zero_length_file_is_a_clean_error() {
    let _watch = PanicWatch::install();
    for ext in SUPPORTED_EXTENSIONS {
        let f = Scratch::with(&format!("empty.{ext}"), b"");
        assert!(
            probe(f.path()).is_err(),
            ".{ext}: an empty file must not probe"
        );
        assert!(
            open(f.path(), 48_000, 64 * 1024 * 1024).is_err(),
            ".{ext}: an empty file must not open"
        );
    }
}

/* ── 3. headers that lie ─────────────────────────────────────────────────── */

#[test]
fn absurd_rates_and_channel_counts_are_refused_or_survived() {
    let watch = PanicWatch::install();
    let rates = [
        0u32,
        1,
        2,
        7,
        8_000,
        192_000,
        3_000_000,
        u32::MAX / 2,
        u32::MAX,
    ];
    let channels = [0u16, 1, 2, 3, 8, 64, 255, 4_096, u16::MAX];
    for rate in rates {
        for ch in channels {
            let header = WavHeader {
                channels: ch,
                sample_rate: rate,
                ..WavHeader::default()
            };
            // A few frames' worth of payload, whatever the declared geometry.
            let f = Scratch::with("geometry.wav", &forge_wav(header, &[], &[0u8; 512]));
            let what = format!("{rate} Hz / {ch} ch");
            let _ = probe(f.path());
            // A small budget on purpose: a 1 Hz header resampled to 48 kHz
            // turns 512 bytes into millions of frames, and the point of the
            // case is that it is bounded, not that it is fast.
            must_survive(f.path(), 48_000, 8 * 1024 * 1024, &what);
            // ... and again where the engine has to resample by a
            // non-integer ratio, which is the path that turns a nonsense rate
            // into a nonsense ratio.
            must_survive(f.path(), 44_100, 8 * 1024 * 1024, &what);
        }
    }
    // `rate = 0` reaches `TimeBase::new(1, 0)` inside symphonia, which asserts.
    // Surviving the loop above therefore proves `safe_decode` is containing a
    // real third-party panic, not that the corpus missed it.
    assert!(
        watch.caught_in_symphonia() > 0,
        "no parser panic was provoked; the 0 Hz case may have stopped reaching symphonia"
    );
}

#[test]
fn a_header_that_claims_an_absurd_duration_cannot_commit_the_whole_budget() {
    let _watch = PanicWatch::install();
    // 456 bytes of file, a `data` chunk claiming four gigabytes: 6.2 hours of
    // 48 kHz stereo. Symphonia only accepts the claim if the enclosing RIFF
    // chunk backs it up, so the RIFF length lies too — which is exactly what a
    // hand-forged file does.
    let bytes = forge_wav(
        WavHeader {
            declared_data_len: Some(u32::MAX - 64),
            declared_riff_len: Some(u32::MAX),
            ..WavHeader::default()
        },
        &[],
        &pcm16(48_000, 2, 0.001),
    );
    let f = Scratch::with("liar.wav", &bytes);
    let real_len = bytes.len();

    let gib = 1024 * 1024 * 1024;
    let handle = must_survive(f.path(), 48_000, gib, "duration liar").expect("a WAV opens");
    let committed =
        handle.pcm.capacity_frames() * handle.pcm.channels() * std::mem::size_of::<f32>();
    // Before `plausible_budget` this committed the whole gigabyte, from a file
    // that fits in one disk block — and two decks made it two gigabytes. The
    // budget is the ceiling of last resort; the real bound is the file.
    assert!(
        committed <= 64 * 1024 * 1024,
        "a {real_len} byte file committed {committed} bytes of PCM"
    );
    assert!(
        handle.pcm.frames_ready() < 4_096,
        "the liar produced {} frames of real audio",
        handle.pcm.frames_ready()
    );
    // ... and the 6.2-hour claim never reaches the playlist row.
    assert_eq!(
        handle.info.duration_secs, 0.0,
        "an impossible duration claim must be discarded, not displayed"
    );

    // The same trick with a 1 Hz sample rate, which multiplies the claim by
    // 48 000 on the way to the resampler.
    let slow = forge_wav(
        WavHeader {
            sample_rate: 1,
            declared_data_len: Some(u32::MAX - 64),
            declared_riff_len: Some(u32::MAX),
            ..WavHeader::default()
        },
        &[],
        &pcm16(48_000, 2, 0.001),
    );
    let f = Scratch::with("slow-liar.wav", &slow);
    let handle = must_survive(f.path(), 48_000, gib, "1 Hz liar").expect("a WAV opens");
    let committed =
        handle.pcm.capacity_frames() * handle.pcm.channels() * std::mem::size_of::<f32>();
    assert!(
        committed <= 64 * 1024 * 1024,
        "a 1 Hz header committed {committed} bytes of PCM"
    );
}

#[test]
fn the_memory_budget_is_enforced_and_reported() {
    let _watch = PanicWatch::install();
    // A genuinely long file, against a budget that cannot hold it. This is the
    // README's "whole file into RAM" limitation: it must degrade to a truncated
    // deck that says so, never to an OOM.
    let f = Scratch::with("long.wav", &healthy_wav(4.0));
    let budget = 1024 * 1024; // 131 072 frames at 2ch f32
    let h = must_survive(f.path(), 48_000, budget, "budget").expect("a WAV opens");
    assert!(h.status.is_truncated(), "over-budget decode must say so");
    assert!(h.pcm.frames_ready() <= 131_072);
    assert!(
        h.pcm.capacity_frames() * 2 * std::mem::size_of::<f32>() <= budget,
        "capacity escaped the budget"
    );

    // The same file inside its budget is not truncated, so the flag means
    // something.
    let h = must_survive(f.path(), 48_000, 64 * 1024 * 1024, "in budget").unwrap();
    assert!(!h.status.is_truncated());
    assert!(h.pcm.frames_ready() > 150_000);
}

/// A real 60-minute 96 kHz master is ~1.3 GB of f32 stereo, which is over the
/// 1 GiB default budget. Writing one here would need 1.3 GB of disk, so the
/// arithmetic is checked instead — the decode path itself is covered by
/// `the_memory_budget_is_enforced_and_reported`.
#[test]
fn a_sixty_minute_ninety_six_kilohertz_master_is_truncated_not_fatal() {
    let frames = 60 * 60 * 96_000usize;
    let needed = frames * 2 * std::mem::size_of::<f32>();
    assert!(
        needed > DEFAULT_DECK_BUDGET_BYTES,
        "the README's worked example is no longer over budget: {needed} bytes"
    );
    let allowed = DEFAULT_DECK_BUDGET_BYTES / (2 * std::mem::size_of::<f32>());
    let minutes = allowed as f64 / 96_000.0 / 60.0;
    // What `loader::load` puts in the toast. If this stops being ~23 minutes
    // the README's number is wrong.
    assert!(
        (22.0..24.0).contains(&minutes),
        "a 1 GiB budget holds {minutes:.1} min at 96 kHz stereo"
    );
}

/* ── 4. the extension lies ───────────────────────────────────────────────── */

#[test]
fn an_extension_that_lies_about_the_contents_still_resolves() {
    let _watch = PanicWatch::install();
    let wav = healthy_wav(0.2);
    // Symphonia takes the extension as a *hint* and must fall back to sniffing
    // the magic; a hint that points at the wrong demuxer may not become a
    // panic, a hang, or a plausible-looking wrong answer.
    for ext in ["flac", "mp3", "ogg", "caf", "m4a", "mka", "opus", "aiff"] {
        let f = Scratch::with(&format!("liar.{ext}"), &wav);
        let what = format!("wav bytes as .{ext}");
        if let Ok(info) = probe(f.path()) {
            assert_eq!(info.sample_rate, 48_000, "{what}: wrong rate");
            assert_eq!(info.channels, 2, "{what}: wrong channel count");
        }
        if let Some(h) = must_survive(f.path(), 48_000, 32 * 1024 * 1024, &what) {
            assert!(
                h.pcm.frames_ready() > 4_000,
                "{what}: only {} frames decoded",
                h.pcm.frames_ready()
            );
        }
    }
    // ... and the other way round: a text file wearing a .wav extension.
    let f = Scratch::with("prose.wav", b"Dear Onyx, this is not a wave file.\n");
    assert!(probe(f.path()).is_err());
    assert!(open(f.path(), 48_000, 1024 * 1024).is_err());
}

/* ── 5. metadata ─────────────────────────────────────────────────────────── */

#[test]
fn metadata_bombs_do_not_exhaust_memory_or_time() {
    let _watch = PanicWatch::install();
    let payload = pcm16(48_000, 2, 0.1);

    // 4 000 junk chunks in front of `data`.
    let many: Vec<(&[u8; 4], Vec<u8>)> = (0..4_000).map(|_| (b"JUNK", vec![0x41; 8])).collect();
    let f = Scratch::with(
        "many-chunks.wav",
        &forge_wav(WavHeader::default(), &many, &payload),
    );
    let started = Instant::now();
    must_survive(f.path(), 48_000, 32 * 1024 * 1024, "4000 chunks");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "4000 chunks took {:?}",
        started.elapsed()
    );

    // One LIST/INFO chunk of 4 MiB of tag text.
    let mut list = b"INFO".to_vec();
    list.extend_from_slice(b"ICMT");
    list.extend_from_slice(&(4u32 * 1024 * 1024).to_le_bytes());
    list.extend(std::iter::repeat_n(b'A', 4 * 1024 * 1024));
    let f = Scratch::with(
        "huge-tag.wav",
        &forge_wav(WavHeader::default(), &[(b"LIST", list)], &payload),
    );
    must_survive(f.path(), 48_000, 32 * 1024 * 1024, "4 MiB tag");

    // A chunk whose declared length runs past the end of the file.
    let f = Scratch::with(
        "overlong-chunk.wav",
        &forge_wav(
            WavHeader::default(),
            &[(b"JUNK", vec![0u8; 8])],
            &payload[..64],
        )[..80],
    );
    must_survive(f.path(), 48_000, 32 * 1024 * 1024, "overlong chunk");

    // An ID3v2 header claiming a 256 MiB tag, in front of nothing at all.
    let mut id3 = b"ID3\x04\x00\x00".to_vec();
    // syncsafe 0x08000000 == 256 MiB
    id3.extend_from_slice(&[0x7F, 0x7F, 0x7F, 0x7F]);
    id3.extend_from_slice(&[0u8; 256]);
    let f = Scratch::with("bomb.mp3", &id3);
    let _ = probe(f.path());
    must_survive(f.path(), 48_000, 32 * 1024 * 1024, "id3 bomb");
}

/* ── 6. the file moves underneath us ─────────────────────────────────────── */

#[test]
fn a_file_that_is_truncated_mid_decode_ends_cleanly() {
    let _watch = PanicWatch::install();
    // Long enough that the decode is still running when the file changes.
    let f = Scratch::with("shrinking.wav", &healthy_wav(30.0));
    let handle = open(f.path(), 48_000, 512 * 1024 * 1024).expect("opens");
    std::thread::sleep(Duration::from_millis(15));
    // Same inode, far fewer bytes: the reader's next read hits EOF early.
    File::create(f.path())
        .expect("truncate")
        .write_all(&healthy_wav(0.01))
        .expect("rewrite");
    assert!(
        settled(&handle),
        "decode of a shrinking file never finished"
    );
    assert!(handle.pcm.is_complete());
    assert!(handle.pcm.frames_ready() <= handle.pcm.capacity_frames());
}

#[test]
fn a_file_replaced_by_garbage_mid_decode_ends_cleanly() {
    let _watch = PanicWatch::install();
    let f = Scratch::with("swapped.wav", &healthy_wav(30.0));
    let handle = open(f.path(), 44_100, 512 * 1024 * 1024).expect("opens");
    std::thread::sleep(Duration::from_millis(15));
    let mut junk = vec![0u8; 512 * 1024];
    let mut rng = Rng(99);
    for b in junk.iter_mut() {
        *b = rng.next() as u8;
    }
    std::fs::write(f.path(), &junk).expect("replace");
    assert!(settled(&handle), "decode of a replaced file never finished");
    assert!(handle.pcm.is_complete());
    // Either it errored or it kept what it had; both are clean, a half-open
    // buffer is not.
    if handle.status.failed() {
        assert!(handle.status.error().is_some());
    }
}

#[test]
fn a_file_deleted_mid_decode_ends_cleanly() {
    let _watch = PanicWatch::install();
    let f = Scratch::with("vanishing.wav", &healthy_wav(30.0));
    let handle = open(f.path(), 48_000, 512 * 1024 * 1024).expect("opens");
    std::thread::sleep(Duration::from_millis(15));
    // POSIX keeps the inode alive for the open descriptor, so this is mostly a
    // no-op on macOS/Linux and a hard failure on Windows. Both must be clean.
    let _ = std::fs::remove_file(f.path());
    assert!(settled(&handle), "decode of a deleted file never finished");
    assert!(handle.pcm.is_complete());
}

/* ── 7. paths ────────────────────────────────────────────────────────────── */

#[test]
fn non_ascii_emoji_and_spaced_file_names_open_normally() {
    let _watch = PanicWatch::install();
    let wav = healthy_wav(0.1);
    let mut names = vec![
        "space in name.wav",
        "Ünïcøde-Mästering.wav",
        "日本語のマスター.wav",
        "🎧 final mix 🎛️.wav",
        "Ω≈ç√∫˜µ≤≥÷.wav",
        // NFD vs NFC: macOS hands back decomposed names from the file system
        // and the dialog, and the two must resolve to the same file.
        "cafe\u{0301}.wav",
        "café.wav",
        "trailing dot .wav",
        "semi;colon&amp.wav",
    ];
    // A double quote is an ordinary character in a POSIX file name and one of
    // the nine Windows forbids (`< > : " / \\ | ? *`), so it cannot be created
    // there at all — asking for it is asking the file system to fail, not the
    // decoder. The apostrophes and the space are legal everywhere and stay in
    // both spellings.
    names.push(if cfg!(windows) {
        "'quoted' name.wav"
    } else {
        "'quoted' \"name\".wav"
    });
    for name in names {
        let f = Scratch::with(name, &wav);
        let info = probe(f.path()).unwrap_or_else(|e| panic!("{name} did not probe: {e}"));
        assert_eq!(info.sample_rate, 48_000, "{name}");
        assert!(
            !info.file_name.is_empty(),
            "{name}: TrackInfo lost the file name"
        );
        assert!(
            info.path.contains(&info.file_name),
            "{name}: path {:?} does not contain file name {:?}",
            info.path,
            info.file_name
        );
        must_survive(f.path(), 48_000, 8 * 1024 * 1024, name);
    }
}

#[test]
fn a_very_long_file_name_is_handled_or_refused_but_never_panics() {
    let _watch = PanicWatch::install();
    // 250 bytes is under the usual 255-byte component limit on APFS/NTFS/ext4;
    // 5 000 is over every one of them and must come back as a clean IO error.
    for len in [250usize, 5_000] {
        let name = format!("{}.wav", "x".repeat(len - 4));
        let s = Scratch::new(&name);
        match std::fs::write(s.path(), healthy_wav(0.05)) {
            Ok(()) => {
                must_survive(
                    s.path(),
                    48_000,
                    8 * 1024 * 1024,
                    &format!("{len} byte name"),
                );
            }
            Err(_) => {
                // The file system refused it, which is exactly what a 5 000
                // character name should do. `open` has to say so, not panic.
                assert!(open(s.path(), 48_000, 1024 * 1024).is_err());
            }
        }
    }
}
