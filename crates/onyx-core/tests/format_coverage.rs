//! Happy-path decode of every format Onyx claims to support (SPEC §17),
//! and MIDI rendering through the same pipeline (SPEC §18).
//!
//! Every fixture in `tests/fixtures/` is the same half-second programme:
//! 440 Hz on the left leg, 660 Hz on the right, at a rate chosen per format.
//! That lets one test assert sample rate, channel count, duration *and* that
//! the two channels did not get swapped or collapsed. `scripts/make-format-
//! fixtures.sh` regenerates them.
//!
//! The hostile side of this is covered elsewhere (the malformed-input corpus);
//! what is here is "the formats we promise actually decode", plus the two
//! rules that are easy to regress: detection is by content, not by extension,
//! and a video container plays its first audio track.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use onyx_core::decode::{
    self, open, open_with, probe, probe_with, DecodeHandle, DecodeOptions,
    DEFAULT_DECK_BUDGET_BYTES,
};
use onyx_core::midi::MidiOptions;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("onyx-formats-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn wait_for(h: &DecodeHandle) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        if h.status.is_finished() {
            assert!(
                h.status.error().is_none(),
                "decode failed: {:?}",
                h.status.error()
            );
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("decode did not finish in time");
}

/// Rough power of one channel over the decoded audio.
fn channel_rms(pcm: &Arc<onyx_core::pcm::SharedPcm>, channel: usize) -> f32 {
    let frames = pcm.frames_ready();
    assert!(frames > 0, "no audio was produced");
    let sum: f64 = (0..frames)
        .map(|i| {
            let s = pcm.frame_stereo(i)[channel] as f64;
            s * s
        })
        .sum();
    (sum / frames as f64).sqrt() as f32
}

/// Dominant frequency by zero-crossing rate. Crude, but these fixtures are
/// single sine tones, and it is enough to tell 440 Hz from 660 Hz — i.e. to
/// prove the channels did not get swapped or folded together.
fn dominant_hz(pcm: &Arc<onyx_core::pcm::SharedPcm>, channel: usize, rate: u32) -> f32 {
    let frames = pcm.frames_ready();
    // Skip the encoder's lead-in, which is silent for lossy formats and would
    // otherwise contribute crossings from dither alone.
    let start = frames / 8;
    let mut crossings = 0usize;
    let mut prev = pcm.frame_stereo(start)[channel];
    for i in start + 1..frames {
        let s = pcm.frame_stereo(i)[channel];
        if (prev <= 0.0 && s > 0.0) || (prev >= 0.0 && s < 0.0) {
            crossings += 1;
        }
        prev = s;
    }
    let secs = (frames - start) as f32 / rate as f32;
    crossings as f32 / (2.0 * secs)
}

struct Expect {
    rate: u32,
    channels: u16,
    container: &'static str,
    codec: &'static str,
    lossless: bool,
}

/// Probe, decode at the source rate, and check what came out.
fn check(name: &str, want: Expect) -> DecodeHandle {
    let path = fixture(name);
    assert!(path.exists(), "missing fixture {name}");

    let info = probe(&path).unwrap_or_else(|e| panic!("{name}: probe failed: {e}"));
    assert_eq!(info.sample_rate, want.rate, "{name}: sample rate");
    assert_eq!(info.channels, want.channels, "{name}: channels");
    assert_eq!(info.container, want.container, "{name}: container");
    assert_eq!(info.codec, want.codec, "{name}: codec");
    assert_eq!(info.is_lossless, want.lossless, "{name}: lossless flag");
    assert!(
        (info.duration_secs - 0.5).abs() < 0.2,
        "{name}: duration {} s, expected ~0.5",
        info.duration_secs
    );
    assert!(
        info.synth_bank.is_none(),
        "{name}: not a synthesised source"
    );

    // Decode at the file's own rate: the bit-transparent path.
    let h = open(&path, want.rate, DEFAULT_DECK_BUDGET_BYTES)
        .unwrap_or_else(|e| panic!("{name}: open failed: {e}"));
    assert!(h.bit_transparent, "{name}: should not need resampling");
    wait_for(&h);

    let frames = h.pcm.frames_ready();
    let secs = frames as f32 / want.rate as f32;
    assert!(
        (0.35..0.75).contains(&secs),
        "{name}: decoded {secs} s ({frames} frames)"
    );
    assert!(
        channel_rms(&h.pcm, 0) > 0.05 && channel_rms(&h.pcm, 1) > 0.05,
        "{name}: one of the channels is silent"
    );
    assert!(h.waveform.len() > 8, "{name}: no waveform peaks");
    assert!(
        h.status.analysis().is_some(),
        "{name}: no loudness analysis"
    );

    // Left is 440 Hz, right is 660 Hz. A ±8% window absorbs the zero-crossing
    // estimator and the lossy codecs' ringing.
    let left = dominant_hz(&h.pcm, 0, want.rate);
    let right = dominant_hz(&h.pcm, 1, want.rate);
    assert!(
        (left - 440.0).abs() < 40.0,
        "{name}: left leg reads {left} Hz, expected 440"
    );
    assert!(
        (right - 660.0).abs() < 55.0,
        "{name}: right leg reads {right} Hz, expected 660"
    );
    h
}

// ---------------------------------------------------------------------------
// §17 one test per format
// ---------------------------------------------------------------------------

#[test]
fn wav_pcm() {
    // The only fixture generated rather than checked in: a WAV writer is four
    // lines and the bytes would be the largest file in the repository.
    let path = tmp("tone.wav");
    write_wav(&path, 44_100, &stereo_tone(44_100, 0.5));
    let info = probe(&path).unwrap();
    assert_eq!(info.sample_rate, 44_100);
    assert_eq!(info.channels, 2);
    assert_eq!(info.container, "WAV");
    assert!(info.codec.starts_with("pcm"));
    assert!(info.is_lossless);
    let h = open(&path, 44_100, DEFAULT_DECK_BUDGET_BYTES).unwrap();
    wait_for(&h);
    assert!((dominant_hz(&h.pcm, 0, 44_100) - 440.0).abs() < 20.0);
    assert!((dominant_hz(&h.pcm, 1, 44_100) - 660.0).abs() < 20.0);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn flac() {
    check(
        "tone.flac",
        Expect {
            rate: 44_100,
            channels: 2,
            container: "FLAC",
            codec: "flac",
            lossless: true,
        },
    );
}

#[test]
fn mp3() {
    check(
        "tone.mp3",
        Expect {
            rate: 48_000,
            channels: 2,
            container: "MPEG",
            codec: "mp3",
            lossless: false,
        },
    );
}

#[test]
fn aac_in_mp4() {
    check(
        "tone-aac.m4a",
        Expect {
            rate: 48_000,
            channels: 2,
            container: "MP4",
            codec: "aac",
            lossless: false,
        },
    );
}

#[test]
fn alac_in_mp4() {
    check(
        "tone-alac.m4a",
        Expect {
            rate: 44_100,
            channels: 2,
            container: "MP4",
            codec: "alac",
            lossless: true,
        },
    );
}

#[test]
fn ogg_vorbis() {
    check(
        "tone.ogg",
        Expect {
            rate: 48_000,
            channels: 2,
            container: "OGG",
            codec: "vorbis",
            lossless: false,
        },
    );
}

/// Opus is the one codec Symphonia demuxes but does not decode; Onyx registers
/// its own decoder for it (see `decode::codecs`).
#[test]
fn ogg_opus() {
    check(
        "tone.opus",
        Expect {
            rate: 48_000,
            channels: 2,
            container: "OGG",
            codec: "opus",
            lossless: false,
        },
    );
}

#[test]
fn aiff() {
    check(
        "tone.aiff",
        Expect {
            rate: 16_000,
            channels: 2,
            container: "AIFF",
            codec: "pcm_s16be",
            lossless: true,
        },
    );
}

#[test]
fn caf() {
    check(
        "tone.caf",
        Expect {
            rate: 16_000,
            channels: 2,
            container: "CAF",
            codec: "pcm_s16le",
            lossless: true,
        },
    );
}

#[test]
fn matroska_audio() {
    check(
        "tone.mka",
        Expect {
            rate: 48_000,
            channels: 2,
            container: "MKV",
            codec: "vorbis",
            lossless: false,
        },
    );
}

#[test]
fn webm_opus() {
    check(
        "tone.webm",
        Expect {
            rate: 48_000,
            channels: 2,
            container: "WEBM",
            codec: "opus",
            lossless: false,
        },
    );
}

/// SPEC §17: a video container plays its first audio track, and the picture
/// is ignored rather than being an error.
#[test]
fn mp4_video_plays_its_audio_track() {
    check(
        "tone-video.mp4",
        Expect {
            rate: 48_000,
            channels: 2,
            container: "MP4",
            codec: "aac",
            lossless: false,
        },
    );
}

#[test]
fn mov_video_plays_its_audio_track() {
    check(
        "tone-video.mov",
        Expect {
            rate: 48_000,
            channels: 2,
            container: "MOV",
            codec: "aac",
            lossless: false,
        },
    );
}

// ---------------------------------------------------------------------------
// §17 detection is by content
// ---------------------------------------------------------------------------

/// When an MP4/MOV sample entry and the bitstream inside it disagree about the
/// sample rate, the bitstream wins.
///
/// The header is a number a muxer wrote down; the AAC `AudioSpecificConfig` and
/// the ALAC magic cookie are what the decoder actually decodes at. Believe the
/// header and the file plays at the wrong speed — here half of it, an octave
/// down, which is the loudest possible way to be wrong in a tool people use to
/// judge masters.
///
/// The fixtures agree with themselves, so the disagreement is manufactured:
/// the 16.16 sample rate in the audio sample entry (32 bytes into the `mp4a` /
/// `alac` box) is halved, and nothing else is touched. Everything must come out
/// exactly as it does from the unpatched file.
#[test]
fn the_bitstream_rate_beats_a_container_that_disagrees() {
    for (name, rate) in [("tone-aac.m4a", 48_000u32), ("tone-alac.m4a", 44_100)] {
        let truth = probe(&fixture(name)).unwrap();
        assert_eq!(truth.sample_rate, rate, "{name}: fixture rate");

        let mut bytes = std::fs::read(fixture(name)).unwrap();
        let tag: &[u8] = if name.contains("alac") {
            b"alac"
        } else {
            b"mp4a"
        };
        let at = bytes
            .windows(4)
            .position(|w| w == tag)
            .unwrap_or_else(|| panic!("{name}: no audio sample entry"));
        let field = at + 28;
        assert_eq!(
            u32::from_be_bytes(bytes[field..field + 4].try_into().unwrap()) >> 16,
            rate,
            "{name}: sample entry is not where it is expected to be"
        );
        bytes[field..field + 4].copy_from_slice(&((rate / 2) << 16).to_be_bytes());

        let path = tmp(&format!("halved-rate-{name}"));
        std::fs::write(&path, &bytes).unwrap();
        let lied = probe(&path).unwrap_or_else(|e| panic!("{name}: probe failed: {e}"));
        assert_eq!(
            lied.sample_rate,
            rate,
            "{name}: took the container's {} Hz over the bitstream's {rate} Hz",
            rate / 2
        );
        // The declared length is counted in units of the rate we just
        // overruled, so it has to be re-expressed at the real one.
        assert!(
            (lied.duration_secs - truth.duration_secs).abs() < 1e-9,
            "{name}: duration {} s, expected {} s",
            lied.duration_secs,
            truth.duration_secs
        );

        // And it decodes at that rate: same length, no resampling.
        let h = open(&path, rate, DEFAULT_DECK_BUDGET_BYTES).unwrap();
        wait_for(&h);
        assert!(h.bit_transparent, "{name}: resampled after all");
        assert_eq!(h.stored_rate, rate, "{name}: stored rate");
        let reference = open(&fixture(name), rate, DEFAULT_DECK_BUDGET_BYTES).unwrap();
        wait_for(&reference);
        assert_eq!(
            h.pcm.frames_ready(),
            reference.pcm.frames_ready(),
            "{name}: decoded a different length than the unpatched fixture"
        );
        let _ = std::fs::remove_file(&path);
    }
}

/// A `.wav` that is really an MP3 must play, and must be labelled honestly.
#[test]
fn a_lying_extension_still_plays_and_is_labelled_by_content() {
    for (source, alias) in [
        ("tone.mp3", "actually-mp3.wav"),
        ("tone.flac", "actually-flac.mp3"),
        ("tone.opus", "actually-opus.aiff"),
        ("tone-aac.m4a", "actually-mp4.flac"),
    ] {
        let path = tmp(alias);
        std::fs::copy(fixture(source), &path).unwrap();
        let truth = probe(&fixture(source)).unwrap();
        let lied = probe(&path).unwrap_or_else(|e| panic!("{alias}: probe failed: {e}"));
        assert_eq!(lied.container, truth.container, "{alias}: container label");
        assert_eq!(lied.codec, truth.codec, "{alias}: codec");
        assert_eq!(lied.sample_rate, truth.sample_rate, "{alias}: rate");

        let h = open(&path, lied.sample_rate, DEFAULT_DECK_BUDGET_BYTES)
            .unwrap_or_else(|e| panic!("{alias}: open failed: {e}"));
        wait_for(&h);
        assert!(h.pcm.frames_ready() > 1_000, "{alias}: no audio decoded");
        let _ = std::fs::remove_file(&path);
    }
}

/// The opposite direction: bytes that are *not* the format the name promises,
/// and are not any format at all. An error is fine; a panic is not.
#[test]
fn a_lying_extension_over_rubbish_is_an_error_not_a_panic() {
    for (name, bytes) in [
        ("empty.wav", vec![]),
        ("truncated.flac", b"fLaC".to_vec()),
        ("text.mp3", b"this is not audio, it is a sentence".to_vec()),
        ("zeros.m4a", vec![0u8; 4_096]),
        ("ff.opus", vec![0xFFu8; 4_096]),
        ("header-only.mid", b"MThd".to_vec()),
        (
            "riff-lies.wav",
            b"RIFF\x24\x00\x00\x00WAVEfmt not really".to_vec(),
        ),
    ] {
        let path = tmp(name);
        std::fs::write(&path, &bytes).unwrap();
        // Both entry points, because the app layer calls both.
        let probed = std::panic::catch_unwind(|| probe(&path));
        assert!(probed.is_ok(), "{name}: probe panicked");
        let opened =
            std::panic::catch_unwind(|| open(&path, 48_000, DEFAULT_DECK_BUDGET_BYTES).is_ok());
        assert!(opened.is_ok(), "{name}: open panicked");
        // If it did open, the decode thread must also survive it.
        if let Ok(true) = opened {
            if let Ok(h) = open(&path, 48_000, DEFAULT_DECK_BUDGET_BYTES) {
                let deadline = Instant::now() + Duration::from_secs(20);
                while !h.status.is_finished() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(5));
                }
                assert!(h.status.is_finished(), "{name}: decode thread hung");
            }
        }
        let _ = std::fs::remove_file(&path);
    }
}

/// Everything the open dialog offers must be something the engine can name.
#[test]
fn every_advertised_extension_is_accepted() {
    for ext in decode::SUPPORTED_EXTENSIONS {
        let path = PathBuf::from(format!("track.{ext}"));
        assert!(
            decode::is_supported_path(&path),
            "{ext} is advertised but not accepted"
        );
        // Case does not matter: files come from Finder and Explorer.
        let upper = PathBuf::from(format!("TRACK.{}", ext.to_uppercase()));
        assert!(decode::is_supported_path(&upper), "{ext} in upper case");
    }
    assert!(!decode::is_supported_path(Path::new("notes.txt")));
    assert!(!decode::is_supported_path(Path::new("no-extension")));
}

// ---------------------------------------------------------------------------
// §18 MIDI through the ordinary pipeline
// ---------------------------------------------------------------------------

/// Two tracks, a tempo change, and a final chord that must be allowed to ring.
fn write_midi(path: &Path) {
    fn var_len(mut v: u32, out: &mut Vec<u8>) {
        let mut buf = vec![v as u8 & 0x7F];
        v >>= 7;
        while v > 0 {
            buf.push((v as u8 & 0x7F) | 0x80);
            v >>= 7;
        }
        buf.reverse();
        out.extend_from_slice(&buf);
    }
    fn track(events: &[(u32, Vec<u8>)]) -> Vec<u8> {
        let mut body = Vec::new();
        for (delta, bytes) in events {
            var_len(*delta, &mut body);
            body.extend_from_slice(bytes);
        }
        var_len(0, &mut body);
        body.extend_from_slice(&[0xFF, 0x2F, 0x00]);
        let mut out = b"MTrk".to_vec();
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(&body);
        out
    }

    // 480 ticks per quarter note, 120 bpm: one beat is half a second.
    let us_per_beat = 500_000u32.to_be_bytes();
    let conductor = track(&[(
        0,
        vec![
            0xFF,
            0x51,
            0x03,
            us_per_beat[1],
            us_per_beat[2],
            us_per_beat[3],
        ],
    )]);
    // Piano on channel 0, a note per beat for two beats, held to the end.
    let melody = track(&[
        (0, vec![0xC0, 0x00]),
        (0, vec![0x90, 60, 100]),
        (480, vec![0x80, 60, 0]),
        (0, vec![0x90, 67, 100]),
        (480, vec![0x80, 67, 0]),
    ]);
    // Strings on channel 1 — long release, which is what the tail is for.
    let pad = track(&[
        (0, vec![0xC1, 48]),
        (0, vec![0x91, 55, 90]),
        (960, vec![0x81, 55, 0]),
    ]);

    let mut out = b"MThd".to_vec();
    out.extend_from_slice(&6u32.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes()); // format 1: multi-track
    out.extend_from_slice(&3u16.to_be_bytes());
    out.extend_from_slice(&480u16.to_be_bytes());
    for t in [conductor, melody, pad] {
        out.extend_from_slice(&t);
    }
    std::fs::write(path, out).unwrap();
}

/// A `.mid` arrives as ordinary PCM: same handle, same waveform, same loudness
/// analysis, same bit-transparency rules. Nothing downstream knows it is MIDI.
#[test]
fn midi_renders_into_the_normal_pipeline() {
    let path = tmp("song.mid");
    write_midi(&path);

    let info = probe(&path).unwrap();
    assert_eq!(info.container, "MIDI");
    assert_eq!(info.codec, "gm");
    assert!(!info.is_lossless, "a synthesised render is not lossless");
    assert_eq!(info.channels, 2);
    let bank = info.synth_bank.clone().expect("bank name reported");
    assert!(bank.to_lowercase().contains("generaluser"), "bank: {bank}");
    assert!(
        info.render_key.is_some(),
        "render key for the loudness cache"
    );
    // Two beats of music plus the release allowance.
    assert!(
        info.duration_secs > 1.0 && info.duration_secs < 6.0,
        "probe duration {} s",
        info.duration_secs
    );
    assert!(
        info.format_badge().contains("MIDI"),
        "{}",
        info.format_badge()
    );

    let h = open(&path, 48_000, DEFAULT_DECK_BUDGET_BYTES).unwrap();
    assert_eq!(h.stored_rate, 48_000);
    assert!(
        h.bit_transparent,
        "the render is produced at the engine rate, so nothing is resampled"
    );
    wait_for(&h);

    let frames = h.pcm.frames_ready();
    let secs = frames as f64 / 48_000.0;
    // The last note-off is at 1.0 s; the render must not stop dead there.
    assert!(
        secs > 1.05,
        "render is {secs} s — the release tail was cut off"
    );
    assert!(secs < 5.0, "render is {secs} s — the tail never decayed");
    // What the probe promised has to resemble what the render produced. The
    // tail cap (`midi::TAIL_MAX_SECS`, ten seconds) is a memory bound, not a
    // duration: reporting it would have shown this file as eleven seconds long
    // in the playlist and then played it for two.
    assert!(
        (info.duration_secs - secs).abs() < 1.5,
        "probe said {} s and the render came to {secs} s",
        info.duration_secs
    );
    assert!(
        info.duration_secs < onyx_core::midi::TAIL_MAX_SECS,
        "the probe reported the worst case ({} s), not an expectation",
        info.duration_secs
    );
    assert!(channel_rms(&h.pcm, 0) > 0.001, "the render is silent");
    assert!(h.waveform.len() > 8, "no waveform peaks for MIDI");
    let analysis = h.status.analysis().expect("loudness measured for MIDI too");
    assert!(
        analysis.integrated_lufs > -60.0 && analysis.integrated_lufs < 0.0,
        "implausible loudness {analysis:?}"
    );
    let _ = std::fs::remove_file(&path);
}

/// A MIDI file at an engine rate that is not the synthesiser's takes the
/// ordinary resampler, exactly as a 44.1 kHz WAV would on a 48 kHz device.
#[test]
fn midi_at_another_engine_rate_goes_through_the_resampler() {
    let path = tmp("song-44k.mid");
    write_midi(&path);
    let h = open(&path, 44_100, DEFAULT_DECK_BUDGET_BYTES).unwrap();
    wait_for(&h);
    let secs = h.pcm.frames_ready() as f64 / 44_100.0;
    assert!(secs > 1.05 && secs < 5.0, "render is {secs} s at 44.1 kHz");
    let _ = std::fs::remove_file(&path);
}

/// A user SoundFont that is missing or not a SoundFont falls back to the
/// bundled bank; the file still plays.
#[test]
fn a_bad_user_soundfont_falls_back_to_the_bundled_bank() {
    let path = tmp("fallback.mid");
    write_midi(&path);

    let missing = DecodeOptions {
        midi: MidiOptions {
            soundfont: Some(tmp("nowhere.sf2")),
        },
    };
    let rubbish_path = tmp("not-a-bank.sf2");
    std::fs::write(&rubbish_path, vec![0u8; 2_048]).unwrap();
    let rubbish = DecodeOptions {
        midi: MidiOptions {
            soundfont: Some(rubbish_path.clone()),
        },
    };

    for opts in [missing, rubbish] {
        let info = probe_with(&path, &opts).unwrap();
        let bank = info.synth_bank.clone().unwrap();
        assert!(
            bank.to_lowercase().contains("generaluser"),
            "should have fallen back, got {bank}"
        );
        let h = open_with(&path, 48_000, DEFAULT_DECK_BUDGET_BYTES, &opts).unwrap();
        wait_for(&h);
        assert!(h.pcm.frames_ready() > 1_000);
    }
    let _ = std::fs::remove_file(&rubbish_path);
    let _ = std::fs::remove_file(&path);
}

/// SPEC §18: the loudness cache is keyed on file identity, so the *bank*
/// has to be part of the key. `render_key` is what the app layer folds in, and
/// it must change when the bank does — here the same bank loaded from a user
/// path, which is a different identity even though it sounds the same.
#[test]
fn the_render_key_changes_with_the_bank() {
    let path = tmp("keyed.mid");
    write_midi(&path);

    let bundled = probe(&path).unwrap();
    let user_bank = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/gm/GeneralUser-GS.sf2");
    let user = probe_with(
        &path,
        &DecodeOptions {
            midi: MidiOptions {
                soundfont: Some(user_bank),
            },
        },
    )
    .unwrap();

    let a = bundled.render_key.unwrap();
    let b = user.render_key.unwrap();
    assert_ne!(a, b, "a bank change must invalidate the cached measurement");
    assert!(!a.is_empty() && !b.is_empty());
    let _ = std::fs::remove_file(&path);
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// 440 Hz left, 660 Hz right — the same programme as the checked-in fixtures.
fn stereo_tone(rate: u32, secs: f32) -> Vec<f32> {
    let frames = (rate as f32 * secs) as usize;
    let mut out = Vec::with_capacity(frames * 2);
    for i in 0..frames {
        let t = i as f32 / rate as f32;
        out.push(0.5 * (2.0 * std::f32::consts::PI * 440.0 * t).sin());
        out.push(0.5 * (2.0 * std::f32::consts::PI * 660.0 * t).sin());
    }
    out
}

fn write_wav(path: &Path, rate: u32, samples: &[f32]) {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&2u16.to_le_bytes()); // stereo
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 4).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32_767.0) as i16).to_le_bytes());
    }
    std::fs::write(path, out).unwrap();
}
