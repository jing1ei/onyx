//! Opus decoding (SPEC §17).
//!
//! Symphonia 0.5 *demuxes* Opus — the Ogg mapping reads `OpusHead`, exposes
//! `CODEC_TYPE_OPUS`, sets the pre-skip as the codec delay and computes packet
//! durations — but it ships no Opus decoder. This module supplies the missing
//! codec as an ordinary [`symphonia::core::codecs::Decoder`] so it can be
//! registered alongside the built-in ones. Nothing outside
//! [`crate::decode::codecs`] knows Opus is handled specially: the decode loop
//! pulls packets and gets `AudioBufferRef`s back exactly as it does for Vorbis.
//!
//! Two details are worth stating because they are easy to get silently wrong:
//!
//! * **Pre-skip and end trimming.** Opus prepends encoder delay and, in Ogg,
//!   ends on a granule position that usually falls mid-packet. Where
//!   Symphonia's gapless machinery turns those into `Packet::trim_start` /
//!   `trim_end`, this decoder applies them to the rendered buffer exactly as
//!   the Vorbis decoder does. It does *not* always manage to: the packets of
//!   the first audio page are queued before the demuxer has worked out the
//!   stream's start bound, so the pre-skip on page one arrives untrimmed. That
//!   leftover is finished off in [`crate::decode`], which knows the whole
//!   timeline; see [`pre_skip`]. Without both halves an Opus file starts
//!   ~6.5 ms early and ends with a few milliseconds of encoder padding, which
//!   is exactly the sort of offset the A/B alignment tests exist to catch.
//! * **Panics are contained.** The Opus implementation we depend on is pure
//!   Rust and correct on valid streams, but fuzzing it with random packet
//!   bytes produces an out-of-range slice index roughly once in twenty
//!   thousand packets. A panic on the decode thread is not something the crate
//!   should rely on the app layer to mop up, so each packet is decoded inside
//!   `catch_unwind` and a panic is reported as an ordinary decode error — the
//!   same thing the decode loop already does with a corrupt packet: count it,
//!   conceal it, keep playing. Catching it is not enough on its own, though:
//!   the panic *hook* still runs first and writes a backtrace header to
//!   stderr, so a file with one bad packet looks like a crash in the log and a
//!   fuzz corpus buries the test output. See [`quieten_decoder_panics`].

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Once;

use symphonia::core::audio::{AsAudioBufferRef, AudioBuffer, AudioBufferRef, Signal, SignalSpec};
use symphonia::core::codecs::{
    CodecDescriptor, CodecParameters, Decoder, DecoderOptions, FinalizeResult, CODEC_TYPE_OPUS,
};
use symphonia::core::errors::{decode_error, unsupported_error, Result as SymResult};
use symphonia::core::formats::Packet;
use symphonia::core::support_codec;

/// Opus always decodes at 48 kHz here. The format allows 8/12/16/24/48 kHz
/// output, but the *stream* is always 48 kHz internally and Symphonia's Ogg
/// mapper reports 48 kHz in the codec parameters, so decoding at anything else
/// would make the timestamps lie.
const OPUS_RATE: u32 = 48_000;

/// Longest Opus frame: 120 ms at 48 kHz.
const MAX_FRAME: usize = 5_760;

/// The pre-skip declared by an `OpusHead` identification header, in 48 kHz
/// samples.
///
/// RFC 7845 §5.1 puts this at bytes 10..12 of the header, little-endian, and
/// §4.2 makes discarding it the *decoder's* job. Symphonia's Ogg mapper reads
/// it too, but it then overwrites `CodecParameters::delay` with a value derived
/// from the granule position of the first audio page — which is the pre-skip
/// plus the end padding when the whole stream fits in one page (anything under
/// about a second, as written by ffmpeg). `OpusHead` is the authority, so
/// [`crate::decode`] asks for it directly rather than believing the derived
/// number.
pub(crate) fn pre_skip(extra_data: Option<&[u8]>) -> Option<u32> {
    let head = extra_data?;
    if head.len() < 12 || &head[..8] != b"OpusHead" {
        return None;
    }
    Some(u32::from(u16::from_le_bytes([head[10], head[11]])))
}

/// Frames of audio a packet carries, read from its TOC byte (RFC 6716 §3.1),
/// at 48 kHz. `None` for an empty or self-contradictory packet.
///
/// This is the length the *timeline* expects, whether or not the payload can
/// be decoded, which is what makes concealment possible.
fn packet_frames(data: &[u8]) -> Option<usize> {
    let toc = *data.first()?;
    let config = usize::from(toc >> 3);
    // 48 kHz samples per frame, by configuration number. SILK modes (0..=11)
    // run 10/20/40/60 ms, hybrid (12..=15) 10/20 ms, CELT (16..=31)
    // 2.5/5/10/20 ms.
    let frame = match config {
        0..=11 => [480, 960, 1_920, 2_880][config % 4],
        12..=15 => [480, 960][config % 2],
        _ => [120, 240, 480, 960][config % 4],
    };
    let count = match toc & 0b11 {
        0 => 1,
        1 | 2 => 2,
        // Code 3: an arbitrary frame count in the low six bits of byte 1.
        _ => usize::from(*data.get(1)? & 0x3F),
    };
    let total = frame * count;
    // RFC 6716 §3.1: a packet may not exceed 120 ms.
    (total > 0 && total <= MAX_FRAME).then_some(total)
}

/// Suppressed panics, for the tests and for anyone wondering whether the hook
/// is really firing.
static SUPPRESSED: AtomicU64 = AtomicU64::new(0);
static HOOK: Once = Once::new();

/// Stop a caught panic from printing.
///
/// `catch_unwind` turns the third-party decoder's occasional out-of-range index
/// into a decode error, but the panic hook runs *before* the unwind is caught,
/// so the default hook has already written `thread '...' panicked at
/// opus-decoder-0.1.1/src/...: attempt to shift left with overflow` to stderr.
/// A dropout that the pipeline handles and reports properly (see
/// `Damage`/`decode`) must not also look like a crash: users read stderr in the
/// log window, and the malformed-input corpus would otherwise print thousands
/// of them.
///
/// So the first `OpusCodec` installs a hook that drops panics raised *inside
/// the decoder crate* — matched on the panic location's file, which is the only
/// thing that reliably identifies them — and delegates everything else to the
/// hook that was already there, whatever it was. Installing it lazily and once
/// keeps it behind the app's own hook rather than in front of it, and counting
/// the suppressed ones means nothing disappears without trace.
fn quieten_decoder_panics() {
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let inside = info
                .location()
                .map(|l| is_decoder_source(l.file()))
                .unwrap_or(false);
            if inside {
                SUPPRESSED.fetch_add(1, Ordering::Relaxed);
                return;
            }
            previous(info);
        }));
    });
}

/// Whether a panic location belongs to the third-party Opus decoder.
///
/// Registry checkouts, vendored trees and git checkouts all spell the path
/// differently, so match the crate name in either of the two spellings it can
/// appear in and nothing else — a panic in *this* file must still be reported.
fn is_decoder_source(file: &str) -> bool {
    file.contains("opus-decoder") || file.contains("opus_decoder")
}

/// How many decoder panics have been suppressed this process (see
/// [`quieten_decoder_panics`]).
#[cfg(test)]
pub(crate) fn suppressed_panics() -> u64 {
    SUPPRESSED.load(Ordering::Relaxed)
}

/// A Symphonia-compatible Opus decoder.
pub struct OpusCodec {
    params: CodecParameters,
    inner: opus_decoder::OpusDecoder,
    /// Interleaved scratch the third-party decoder writes into.
    scratch: Vec<f32>,
    /// Planar buffer handed back to Symphonia.
    buf: AudioBuffer<f32>,
    channels: usize,
}

impl OpusCodec {
    /// Decode one packet into `scratch`, returning frames per channel.
    ///
    /// Wrapped in `catch_unwind` (see the module docs): a panic is downgraded
    /// to a decode error and the decoder is reset, because its internal state
    /// is not trustworthy after unwinding out of the middle of it.
    fn decode_packet(&mut self, data: &[u8]) -> Outcome {
        let inner = &mut self.inner;
        let scratch = &mut self.scratch;
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            inner.decode_float(data, scratch, false)
        }));
        match outcome {
            Ok(Ok(frames)) => Outcome::Decoded(frames),
            Ok(Err(_)) => Outcome::Unreadable,
            Err(_) => {
                self.inner.reset();
                Outcome::Panicked
            }
        }
    }
}

/// What one call into the third-party decoder came back with.
enum Outcome {
    Decoded(usize),
    /// The payload was rejected — a corrupt or truncated packet.
    Unreadable,
    /// The decoder unwound. See [`OpusCodec::decode`].
    Panicked,
}

impl Decoder for OpusCodec {
    fn try_new(params: &CodecParameters, _options: &DecoderOptions) -> SymResult<Self> {
        // Before the first packet can panic. Idempotent.
        quieten_decoder_panics();
        let channels = match params.channels {
            Some(ch) => ch.count(),
            None => 2,
        };
        // RFC 6716 single-stream Opus is mono or stereo. Surround Opus uses the
        // multistream mapping, which the Ogg demuxer already refuses, but a
        // hand-built MP4/Matroska track could still claim more — say so rather
        // than decode the wrong thing.
        if channels == 0 || channels > 2 {
            return unsupported_error("opus: only mono and stereo streams are supported");
        }
        let inner = opus_decoder::OpusDecoder::new(OPUS_RATE, channels)
            .map_err(|_| symphonia::core::errors::Error::Unsupported("opus: bad stream setup"))?;

        let spec = SignalSpec::new(
            OPUS_RATE,
            params.channels.unwrap_or(match channels {
                1 => symphonia::core::audio::Channels::FRONT_LEFT,
                _ => {
                    symphonia::core::audio::Channels::FRONT_LEFT
                        | symphonia::core::audio::Channels::FRONT_RIGHT
                }
            }),
        );

        let mut out = params.clone();
        out.with_sample_rate(OPUS_RATE);

        Ok(OpusCodec {
            params: out,
            inner,
            scratch: vec![0.0; MAX_FRAME * channels],
            buf: AudioBuffer::new(MAX_FRAME as u64, spec),
            channels,
        })
    }

    fn supported_codecs() -> &'static [CodecDescriptor] {
        &[support_codec!(CODEC_TYPE_OPUS, "opus", "Opus")]
    }

    fn reset(&mut self) {
        self.inner.reset();
    }

    fn codec_params(&self) -> &CodecParameters {
        &self.params
    }

    fn decode(&mut self, packet: &Packet) -> SymResult<AudioBufferRef<'_>> {
        self.buf.clear();

        let frames = match self.decode_packet(packet.buf()) {
            Outcome::Decoded(f) => f.min(MAX_FRAME),
            // The trait requires the internal buffer to be empty after an
            // error, which `clear()` above already guarantees.
            Outcome::Unreadable => return decode_error("opus: unreadable packet"),
            // The decoder fell over on a packet the stream says is real. Losing
            // it outright would shorten the file and shift everything after it,
            // so stand in for it: the TOC byte still says how long the packet
            // is, and silence of the right length is a dropout rather than a
            // different master. This is what the reference implementation calls
            // packet loss concealment, minus the extrapolation.
            Outcome::Panicked => match packet_frames(packet.buf()) {
                Some(f) => {
                    self.scratch[..f * self.channels].fill(0.0);
                    f
                }
                None => return decode_error("opus: unreadable packet"),
            },
        };

        self.buf.render_reserved(Some(frames));
        for c in 0..self.channels {
            let dst = self.buf.chan_mut(c);
            for (f, sample) in dst.iter_mut().enumerate().take(frames) {
                *sample = self.scratch[f * self.channels + c];
            }
        }

        // Encoder delay at the head and the granule-position cut at the tail.
        // Symphonia computed both for us when `enable_gapless` is set.
        self.buf.trim(
            packet.trim_start() as usize,
            packet.trim_end().min(frames as u32) as usize,
        );

        Ok(self.buf.as_audio_buffer_ref())
    }

    fn finalize(&mut self) -> FinalizeResult {
        FinalizeResult::default()
    }

    fn last_decoded(&self) -> AudioBufferRef<'_> {
        self.buf.as_audio_buffer_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use symphonia::core::audio::Channels;

    fn stereo_params() -> CodecParameters {
        let mut p = CodecParameters::new();
        p.for_codec(CODEC_TYPE_OPUS)
            .with_sample_rate(OPUS_RATE)
            .with_channels(Channels::FRONT_LEFT | Channels::FRONT_RIGHT);
        p
    }

    #[test]
    fn advertises_opus_to_the_registry() {
        let descs = OpusCodec::supported_codecs();
        assert_eq!(descs.len(), 1);
        assert_eq!(descs[0].codec, CODEC_TYPE_OPUS);
        assert_eq!(descs[0].short_name, "opus");
    }

    #[test]
    fn refuses_channel_counts_opus_cannot_have() {
        let mut p = CodecParameters::new();
        p.for_codec(CODEC_TYPE_OPUS)
            .with_sample_rate(OPUS_RATE)
            .with_channels(
                Channels::FRONT_LEFT
                    | Channels::FRONT_RIGHT
                    | Channels::FRONT_CENTRE
                    | Channels::LFE1
                    | Channels::REAR_LEFT
                    | Channels::REAR_RIGHT,
            );
        assert!(OpusCodec::try_new(&p, &DecoderOptions::default()).is_err());
    }

    /// The real proof that Opus works is `tests/format_coverage.rs`, which
    /// decodes an actual `.opus` file. This is the other half: garbage packets
    /// must come back as decode errors — never as a panic on the decode thread,
    /// and never as a buffer full of stale samples from the previous packet.
    #[test]
    fn hostile_packets_are_errors_not_panics() {
        let mut dec = OpusCodec::try_new(&stereo_params(), &DecoderOptions::default()).unwrap();
        let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut errors = 0usize;
        for _ in 0..4_000 {
            let len = (rnd() % 400) as usize;
            let data: Vec<u8> = (0..len).map(|_| (rnd() & 0xFF) as u8).collect();
            let packet = Packet::new_from_slice(0, 0, 960, &data);
            if dec.decode(&packet).is_err() {
                errors += 1;
                // A failed decode must leave nothing behind for the caller to
                // mistake for audio.
                assert_eq!(dec.last_decoded().frames(), 0);
            }
        }
        assert!(errors > 0, "random bytes should not all decode cleanly");
    }

    /// The fuzz above provokes real panics inside the decoder crate. None of
    /// them may reach stderr: a concealed dropout is not a crash, and the
    /// malformed-input corpus would print thousands of backtrace headers.
    ///
    /// Stderr cannot be inspected from inside the process, so this asserts the
    /// two halves that make silence certain: the hook counted the panics it
    /// swallowed, and the predicate it swallows them on accepts the decoder's
    /// own source while rejecting everything else, this file included.
    #[test]
    fn decoder_panics_are_swallowed_rather_than_printed() {
        let mut dec = OpusCodec::try_new(&stereo_params(), &DecoderOptions::default()).unwrap();
        let before = suppressed_panics();
        let mut seed: u64 = 0x1234_5678_9abc_def0;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..4_000 {
            let len = (rnd() % 400) as usize;
            let data: Vec<u8> = (0..len).map(|_| (rnd() & 0xFF) as u8).collect();
            let _ = dec.decode(&Packet::new_from_slice(0, 0, 960, &data));
        }
        assert!(
            suppressed_panics() > before,
            "4000 hostile packets provoked no decoder panic at all — if the \
             dependency has been fixed, this suppression can go"
        );

        assert!(is_decoder_source(
            "/root/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/\
             opus-decoder-0.1.1/src/silk/decoder.rs"
        ));
        assert!(is_decoder_source("vendor/opus_decoder/src/lib.rs"));
        assert!(!is_decoder_source("crates/onyx-core/src/opus.rs"));
        assert!(!is_decoder_source(
            "/root/.cargo/registry/src/x/symphonia-core-0.5.4/src/audio.rs"
        ));
    }
}
