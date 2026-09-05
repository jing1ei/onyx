//! File probing and background decoding.
//!
//! One pass over the file does everything the player needs:
//! * decode to f32,
//! * sample-rate convert to the engine rate (only when required),
//! * build the waveform peaks,
//! * measure integrated loudness / true peak for A/B level matching.
//!
//! Audio is published to a [`SharedPcm`] as it is produced, so the engine can
//! start playing after the first few milliseconds.
//!
//! **Two kinds of source, one pipeline** (SPEC §17/§18). Coded files go
//! through Symphonia; MIDI files are rendered by the SoundFont synthesiser in
//! [`crate::midi`] *on this same decode thread*. Past [`SourceReader`] the two
//! are indistinguishable — the fold, resample, waveform, loudness and publish
//! stages are shared — which is what lets MIDI inherit seeking, A/B, EQ and
//! metering without a second playback path.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;
use rubato::{
    calculate_cutoff, Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{
    CodecParameters, CodecRegistry, DecoderOptions, CODEC_TYPE_AAC, CODEC_TYPE_ALAC,
    CODEC_TYPE_NULL, CODEC_TYPE_OPUS,
};
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::{MetadataOptions, StandardTagKey};
use symphonia::core::probe::Hint;

use crate::container::{self, Container};
use crate::dsp::loudness::LoudnessMeter;
use crate::dsp::truepeak::TruePeak;
use crate::error::{Error, Result};
use crate::midi::{self, MidiOptions};
use crate::pcm::{PcmWriter, SharedPcm};
use crate::types::{LoudnessAnalysis, TrackInfo};
use crate::waveform::{Waveform, WaveformBuilder};

/// Default per-deck memory ceiling for decoded audio (f32). 1 GiB covers ~49
/// minutes of 44.1 kHz stereo or ~22 minutes at 96 kHz.
pub const DEFAULT_DECK_BUDGET_BYTES: usize = 1_024 * 1_024 * 1_024;

/// Version of *what this module does to the samples* (SPEC §8).
///
/// # Bump this when decoded output changes
///
/// The loudness / true-peak numbers in `src-tauri/src/cache.rs` describe the
/// PCM this module produced, not the bytes on disk, so every cached measurement
/// is keyed on this constant as well as on the file. Anything that moves a
/// sample changes the measurement, and a stale measurement is worse than no
/// measurement: A/B level matching would attenuate by a number nobody can
/// reproduce, and the playlist would show a LUFS value the file no longer has.
///
/// Concretely, **bump it** when any of these changes:
///
/// * **Opus pre-skip** — `OpusHead`'s declared encoder delay is trimmed from
///   the head and subtracted from the declared length (see `head_delay` in
///   `start_decode` and [`crate::opus::pre_skip`]).
/// * **AAC / MP4 edit-list priming** — the `edts` box is read here because
///   Symphonia ignores it, and the priming it states is dropped from the head
///   (see `IsoEdit`).
/// * **The declared-length bound** — where a track is cut when the container
///   says it is shorter than the last packet.
/// * **Channel folding, resampling or dithering** — the fold to stereo, the
///   sinc resampler's parameters, or the rule that a decode at the source rate
///   is bit-transparent.
/// * **The loudness or true-peak maths** in `crate::dsp` (`LoudnessMeter`,
///   `TruePeak`) — the same PCM measured differently is still a different
///   number.
///
/// Bumping it costs one re-measure per file, on the next play, and nothing else:
/// old records simply stop matching. Not bumping it costs a silently wrong
/// number in a mastering tool, for as long as the cache file survives.
///
/// `cache::tests::a_decode_semantics_bump_misses_and_an_unrelated_change_still_hits`
/// (in `src-tauri`) and
/// `decode::tests::changing_what_a_decode_produces_must_bump_decode_semantics`
/// are the tripwires that make this constant impossible to miss.
pub const DECODE_SEMANTICS: u32 = 1;

/// Resampler chunk size, in input frames.
const RESAMPLE_CHUNK: usize = 1_024;

/// Extensions we advertise in the open dialog / accept on drop.
///
/// This is a *filter*, not a format decision: what a file actually is gets
/// settled by [`crate::container::sniff`] and by Symphonia's own probe, so a
/// `.wav` containing an MP3 still plays (SPEC §17).
pub const SUPPORTED_EXTENSIONS: &[&str] = &[
    "wav", "wave", "bwf", "flac", "mp3", "m4a", "mp4", "m4v", "mov", "aac", "alac", "ogg", "oga",
    "opus", "aiff", "aif", "aifc", "caf", "mka", "mkv", "webm", "adpcm", "mid", "midi",
];

/// Codec registry: everything Symphonia was built with, plus our own Opus
/// decoder (SPEC §17 — Symphonia demuxes Opus but does not decode it).
///
/// Public because it is the honest answer to "what can this build play?", and
/// because a caller that wants to construct a decoder itself must use the same
/// registry the decode path does, not `symphonia::default::get_codecs()`.
pub fn codecs() -> &'static CodecRegistry {
    static REGISTRY: OnceLock<CodecRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut registry = CodecRegistry::new();
        symphonia::default::register_enabled_codecs(&mut registry);
        registry.register_all::<crate::opus::OpusCodec>();
        registry
    })
}

/// Everything the decoder needs that is not the file itself.
///
/// Passed to [`probe_with`] / [`open_with`] by the app layer, which builds it
/// from `settings.json`. [`probe`] and [`open`] use the defaults.
#[derive(Clone, Debug, Default)]
pub struct DecodeOptions {
    /// MIDI rendering settings — the user's `.sf2`, if they picked one.
    pub midi: MidiOptions,
}

/// Is this a file Onyx will try to open?
pub fn is_supported_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let e = e.to_ascii_lowercase();
            SUPPORTED_EXTENSIONS.contains(&e.as_str())
        })
        .unwrap_or(false)
}

/// Decode lifecycle, shared between the decode thread and everyone else.
pub struct DecodeStatus {
    finished: AtomicBool,
    truncated: AtomicBool,
    cancel: AtomicBool,
    /// 0 = running, 1 = ok, 2 = error
    outcome: AtomicU8,
    error: Mutex<Option<String>>,
    analysis: Mutex<Option<LoudnessAnalysis>>,
}

impl DecodeStatus {
    fn new() -> Self {
        DecodeStatus {
            finished: AtomicBool::new(false),
            truncated: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            outcome: AtomicU8::new(0),
            error: Mutex::new(None),
            analysis: Mutex::new(None),
        }
    }

    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    /// True when the file was longer than the memory budget and had to be cut.
    pub fn is_truncated(&self) -> bool {
        self.truncated.load(Ordering::Acquire)
    }

    /// Ask the decode thread to stop (used when the user moves on).
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }

    pub fn error(&self) -> Option<String> {
        self.error.lock().clone()
    }

    /// True once the decode thread gave up with an error.
    pub fn failed(&self) -> bool {
        self.outcome.load(Ordering::Acquire) == 2
    }

    /// Loudness / peak analysis. `None` until decoding completes.
    pub fn analysis(&self) -> Option<LoudnessAnalysis> {
        *self.analysis.lock()
    }
}

/// Handle to a track that is loading or loaded.
#[derive(Clone)]
pub struct DecodeHandle {
    pub info: TrackInfo,
    pub pcm: Arc<SharedPcm>,
    pub waveform: Arc<Waveform>,
    pub status: Arc<DecodeStatus>,
    /// Sample rate the PCM is actually stored at (== engine rate).
    pub stored_rate: u32,
    /// True when no sample-rate conversion was applied.
    pub bit_transparent: bool,
}

impl DecodeHandle {
    /// Length in seconds of the audio that will eventually be available.
    pub fn duration_secs(&self) -> f64 {
        if self.pcm.is_complete() {
            self.pcm.frames_ready() as f64 / self.stored_rate as f64
        } else {
            let expected = self.pcm.expected_frames().max(self.pcm.frames_ready());
            expected as f64 / self.stored_rate as f64
        }
    }
}

/// Read tags and stream parameters without decoding audio.
pub fn probe(path: &Path) -> Result<TrackInfo> {
    probe_with(path, &DecodeOptions::default())
}

/// [`probe`] with explicit options. MIDI needs them: which SoundFont is in use
/// determines the bank name shown and the render key the loudness cache needs.
pub fn probe_with(path: &Path, options: &DecodeOptions) -> Result<TrackInfo> {
    let (info, _) = probe_inner(path, options, 0)?;
    Ok(info)
}

/// Anything that can hand the pipeline the next chunk of interleaved f32 at
/// the source sample rate.
///
/// The two variants are the *only* place MIDI differs from a coded file.
enum SourceReader {
    /// Symphonia: demux a packet, decode it, fold it.
    Coded(Coded),
    /// Already-rendered PCM sitting in memory (SPEC §18).
    Rendered(Rendered),
    /// A MIDI file that has not been synthesised yet. Rendering happens on the
    /// decode thread — see [`SourceReader::realise`] — so that `open` returns
    /// immediately for MIDI exactly as it does for everything else.
    PendingRender {
        path: PathBuf,
        options: MidiOptions,
        rate: u32,
    },
}

struct Coded {
    format: Box<dyn symphonia::core::formats::FormatReader>,
    decoder: Box<dyn symphonia::core::codecs::Decoder>,
    track_id: u32,
    sample_buf: Option<SampleBuffer<f32>>,
    last_spec: Option<symphonia::core::audio::SignalSpec>,
    /// Encoder delay still to be discarded from the head of the stream.
    head_remaining: u64,
    /// Exact output length in source frames, when the container states one.
    limit: Option<u64>,
    /// Frames handed to the pipeline so far, counted against `limit`.
    emitted: u64,
}

/// Longest run of silence a single unreadable packet may stand in for.
/// 2 s at any sane rate: a packet claiming more than that is a broken header,
/// not a dropout.
const MAX_CONCEAL_FRAMES: u64 = 96_000 * 2;

struct Rendered {
    samples: Vec<f32>,
    channels: usize,
    pos: usize,
}

/// Outcome of one pull from a [`SourceReader`].
enum Pull {
    /// `n` frames were appended to the output buffer.
    Frames(usize),
    /// Nothing this time, but the stream continues (a non-audio packet, or a
    /// damaged one that was skipped).
    Skipped,
    /// End of stream.
    End,
}

/// Damage counters. Accumulated rather than logged per packet: a file that is
/// corrupt all the way through would otherwise write thousands of identical
/// lines. [`decode_loop`] emits one summary at the end.
#[derive(Default)]
struct Damage {
    packets: u64,
    padded_frames: u64,
}

impl SourceReader {
    /// Turn a planned MIDI render into actual samples. Called once, first
    /// thing on the decode thread; a no-op for every other source.
    ///
    /// `max_frames` is the deck's remaining capacity, so a MIDI file cannot
    /// out-allocate the budget any more than a WAV can.
    fn realise(&mut self, max_frames: usize) -> Result<()> {
        let SourceReader::PendingRender {
            path,
            options,
            rate,
        } = self
        else {
            return Ok(());
        };
        let started = std::time::Instant::now();
        let render = midi::render(path, *rate, options, max_frames)?;
        log::debug!(
            "rendered \"{}\" through {} in {} ms: {:.2} s at {} Hz{}",
            file_name_of(path),
            render.bank.name,
            started.elapsed().as_millis(),
            render.duration_secs(),
            render.rate,
            if render.truncated {
                " (budget capped)"
            } else {
                ""
            }
        );
        *self = SourceReader::Rendered(Rendered {
            samples: render.samples,
            channels: 2,
            pos: 0,
        });
        Ok(())
    }

    /// Append the next chunk of `out_channels`-wide interleaved audio to `dst`.
    fn pull(
        &mut self,
        out_channels: usize,
        dst: &mut Vec<f32>,
        damage: &mut Damage,
        name: &str,
    ) -> Result<Pull> {
        match self {
            SourceReader::Coded(c) => c.pull(out_channels, dst, damage, name),
            SourceReader::Rendered(r) => Ok(r.pull(out_channels, dst)),
            // `realise` runs before the first pull, so this is unreachable in
            // practice; treating it as end-of-stream keeps it un-panicky if a
            // future caller forgets.
            SourceReader::PendingRender { .. } => Ok(Pull::End),
        }
    }
}

impl Rendered {
    fn pull(&mut self, out_channels: usize, dst: &mut Vec<f32>) -> Pull {
        let total_frames = self.samples.len() / self.channels;
        if self.pos >= total_frames {
            return Pull::End;
        }
        let frames = RESAMPLE_CHUNK.min(total_frames - self.pos);
        let from = self.pos * self.channels;
        let to = (self.pos + frames) * self.channels;
        // Reuses the same fold as the coded path, so a mono engine store or a
        // future multichannel render behaves identically either way.
        let _ = fold_interleaved(
            &self.samples[from..to],
            frames,
            self.channels,
            out_channels,
            dst,
        );
        self.pos += frames;
        Pull::Frames(frames)
    }
}

impl Coded {
    /// Frames of this packet that may still be emitted, after the gapless
    /// bounds have had their say. `None` means the stream is over.
    fn room(&self) -> Option<u64> {
        match self.limit {
            Some(limit) if self.emitted >= limit => None,
            Some(limit) => Some(limit - self.emitted),
            None => Some(u64::MAX),
        }
    }

    fn pull(
        &mut self,
        out_channels: usize,
        dst: &mut Vec<f32>,
        damage: &mut Damage,
        name: &str,
    ) -> Result<Pull> {
        // Everything the container said it holds has been emitted; the rest is
        // encoder padding (SPEC §17).
        if self.room().is_none() {
            return Ok(Pull::End);
        }
        let packet = match self.format.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                return Ok(Pull::End)
            }
            Err(symphonia::core::errors::Error::ResetRequired) => {
                self.decoder.reset();
                return Ok(Pull::Skipped);
            }
            Err(e) => return Err(e.into()),
        };
        // Video and any other track we did not choose: skipped here, which is
        // how "ignore video entirely" is implemented (SPEC §17).
        if packet.track_id() != self.track_id {
            return Ok(Pull::Skipped);
        }
        // Whatever the demuxer trimmed off the head is delay we no longer owe.
        self.head_remaining = self
            .head_remaining
            .saturating_sub(u64::from(packet.trim_start()));

        // Copied out before the decoder's buffer is borrowed below.
        let head_remaining = self.head_remaining;
        let room = self.room().unwrap_or(0);

        let decoded = match self.decoder.decode(&packet) {
            Ok(d) => d,
            // Recoverable stream hiccups. A damaged frame must never stop a
            // listening session — but dropping it outright would shorten the
            // file and shift everything after it, so it is *concealed*: the
            // packet's own duration is emitted as silence and the timeline
            // survives. A 20 ms hole is a dropout; a 20 ms shift is a
            // different master.
            Err(symphonia::core::errors::Error::DecodeError(e)) => {
                damage.packets += 1;
                if damage.packets == 1 {
                    // Detail, once: the summary carries the count, this carries
                    // the reason the first one was unreadable.
                    log::debug!("\"{name}\": first unreadable packet: {e}");
                }
                let dur = packet.dur().min(MAX_CONCEAL_FRAMES);
                let skipped = self.head_remaining.min(dur);
                self.head_remaining -= skipped;
                let conceal = self.admit(dur - skipped);
                if conceal == 0 {
                    return Ok(Pull::Skipped);
                }
                damage.padded_frames += conceal as u64;
                dst.resize(dst.len() + conceal * out_channels, 0.0);
                return Ok(Pull::Frames(conceal));
            }
            Err(symphonia::core::errors::Error::IoError(_)) => return Ok(Pull::End),
            Err(symphonia::core::errors::Error::ResetRequired) => {
                self.decoder.reset();
                return Ok(Pull::Skipped);
            }
            Err(e) => return Err(e.into()),
        };

        let spec = *decoded.spec();
        let frames = decoded.frames();
        if frames == 0 {
            return Ok(Pull::Skipped);
        }
        // Trust the *decoded* frame, not the container's header: a file whose
        // codec parameters disagree with its packets (or that changes channel
        // count mid-stream, which happens in the wild) would otherwise index
        // past the end of the sample buffer and panic on the decode thread.
        let packet_channels = spec.channels.count().max(1);
        if self.last_spec != Some(spec) {
            // A new signal spec invalidates the buffer's layout entirely.
            self.last_spec = Some(spec);
            self.sample_buf = None;
        }
        let buf = self
            .sample_buf
            .get_or_insert_with(|| SampleBuffer::<f32>::new(decoded.capacity() as u64, spec));
        // `SampleBuffer::capacity()` counts samples, not frames.
        if buf.capacity() < frames * packet_channels {
            *buf = SampleBuffer::<f32>::new(frames as u64, spec);
        }
        buf.copy_interleaved_ref(decoded);

        // Encoder delay the demuxer left behind, then the tail cut.
        let skip = head_remaining.min(frames as u64) as usize;
        let keep = ((frames - skip) as u64).min(room) as usize;
        let padded = if keep == 0 {
            0
        } else {
            // Fold to the stored channel count (front pair for multichannel).
            fold_interleaved(
                &buf.samples()[skip * packet_channels..],
                keep,
                packet_channels,
                out_channels,
                dst,
            ) as u64
        };
        damage.padded_frames += padded;
        self.head_remaining -= skip as u64;
        self.emitted += keep as u64;
        if keep == 0 {
            return Ok(Pull::Skipped);
        }
        Ok(Pull::Frames(keep))
    }

    /// Book `want` frames against the declared length, returning how many of
    /// them may actually be emitted.
    fn admit(&mut self, want: u64) -> usize {
        let room = self.room().unwrap_or(0);
        let keep = want.min(room);
        self.emitted += keep;
        keep as usize
    }
}

struct Source {
    reader: SourceReader,
    src_rate: u32,
    src_channels: usize,
    expected_frames: u64,
}

/// Sniff the container from the file's own bytes.
///
/// A read failure is not fatal here — Symphonia gets the same file next and
/// will produce a real error — so this reports `None` and the container badge
/// falls back to whatever the demuxer says.
fn sniff_container(path: &Path) -> Option<Container> {
    let mut file = File::open(path).ok()?;
    let mut buf = vec![0u8; container::SNIFF_BYTES];
    let mut filled = 0usize;
    // `read` is allowed to return short; loop until EOF or the buffer is full.
    while filled < buf.len() {
        match file.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    buf.truncate(filled);
    container::sniff(&buf)
}

fn probe_inner(
    path: &Path,
    options: &DecodeOptions,
    render_rate: u32,
) -> Result<(TrackInfo, Source)> {
    let sniffed = sniff_container(path);
    if sniffed.map(Container::is_midi).unwrap_or(false) {
        return probe_midi(path, options, render_rate);
    }
    probe_coded(path, sniffed)
}

/// MIDI: parse the sequence, and render it when a rate is asked for.
///
/// `render_rate` is 0 for a metadata-only probe (adding a folder of files to
/// the playlist must not synthesise anything) and the engine rate when the
/// file is being opened — and even then this only *plans* the render. The
/// synthesis itself happens on the decode thread, in [`decode_loop`], because
/// `open` must return in well under a millisecond.
fn probe_midi(
    path: &Path,
    options: &DecodeOptions,
    render_rate: u32,
) -> Result<(TrackInfo, Source)> {
    let size_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let midi_info = midi::probe(path, &options.midi)?;

    let rate = if render_rate == 0 {
        0
    } else {
        midi::synth_rate_for(render_rate)
    };
    let reader = if render_rate == 0 {
        SourceReader::Rendered(Rendered {
            samples: Vec::new(),
            channels: 2,
            pos: 0,
        })
    } else {
        SourceReader::PendingRender {
            path: path.to_path_buf(),
            options: options.midi.clone(),
            rate,
        }
    };
    // Two different numbers, deliberately. The *buffer* is sized from the upper
    // bound, exactly as the coded path sizes itself from a container's declared
    // duration, because a render that outgrows its deck would be cut off. The
    // *reported* duration is the expectation, because the bound is eleven
    // seconds for a two-bar sketch and a playlist that says so is lying by a
    // factor of eight. Either way it is provisional: once the render is done,
    // `DecodeHandle::duration_secs` reports what was actually produced, and
    // that is what the transport, the waveform and the loop bounds use.
    let expected_frames = (midi_info.max_render_secs * rate.max(1) as f64).ceil() as u64;

    let info = TrackInfo {
        path: path.to_string_lossy().to_string(),
        file_name: file_name_of(path),
        duration_secs: midi_info.expected_secs,
        // Nothing has been rendered yet at probe time, so there is no rate to
        // report; once open, this is the rate the render was produced at.
        sample_rate: rate,
        channels: 2,
        bits_per_sample: None,
        codec: "gm".to_string(),
        container: Container::Midi.label().to_string(),
        bitrate_kbps: None,
        // A synthesised render is not a lossless reproduction of anything.
        is_lossless: false,
        size_bytes,
        title: None,
        artist: None,
        album: None,
        synth_bank: Some(midi_info.bank.name.clone()),
        render_key: Some(midi_info.bank.identity().to_string()),
    };

    if let Some(reason) = midi_info.bank.fallback_reason.as_deref() {
        log::warn!("{reason}");
    }

    Ok((
        info,
        Source {
            reader,
            src_rate: rate.max(1),
            src_channels: 2,
            expected_frames: if render_rate == 0 { 0 } else { expected_frames },
        },
    ))
}

fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Channel count the stored buffer is laid out for.
///
/// `declared` is what the container said, `coded` what the bitstream's own
/// configuration said (see [`BitstreamConfig`]); a container that states
/// nothing is normal — Symphonia's ISO-MP4 reader never fills this in, which is
/// why every `.m4a` depends on the second source.
///
/// When neither states anything, refuse. Nothing downstream would crash on a
/// track with no channel count: the fold trusts the channel count of each
/// decoded packet and `max(1)` guards the arithmetic. What it would do instead
/// is invent a layout — the guess used to be a flat "assume stereo" — and then
/// play a malformed file confidently while labelling it "2 ch" in the UI. For a
/// tool people use to judge masters, a sentence saying what is wrong with the
/// file is worth more than a plausible wrong answer (SPEC §9.6).
fn stored_channels(declared: Option<usize>, coded: Option<usize>) -> Result<usize> {
    match declared.filter(|n| *n > 0).or(coded.filter(|n| *n > 0)) {
        Some(n) => Ok(n),
        None => Err(Error::Unsupported(
            "the audio track does not say how many channels it has".into(),
        )),
    }
}

/// What a codec's own configuration says about the audio, for the codecs that
/// carry one where Symphonia hands it to us verbatim.
///
/// An MP4/MOV audio sample entry carries numbers a muxer wrote down, and they
/// can disagree with the AAC `AudioSpecificConfig` or the ALAC magic cookie in
/// the very same file — remuxers and stream copies get this wrong — or be
/// missing altogether, which is the ordinary case for the channel count. The
/// decoder works from the configuration, so that is what the pipeline has to
/// agree with: believe a sample entry that says half the real rate and the file
/// plays an octave down, which is the loudest possible way to be wrong.
///
/// It has to be read here rather than asked for, because nothing in Symphonia
/// exposes it: `Decoder::codec_params()` hands the container's numbers straight
/// back, and the truth only appears in the `SignalSpec` of a decoded packet — by
/// which point the deck has been sized, the resampler configured and the rate
/// reported to the UI.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct BitstreamConfig {
    rate: Option<u32>,
    channels: Option<usize>,
}

impl BitstreamConfig {
    fn of(params: &CodecParameters) -> BitstreamConfig {
        let Some(extra) = params.extra_data.as_deref() else {
            return BitstreamConfig::default();
        };
        match params.codec {
            CODEC_TYPE_AAC => aac_config(extra),
            CODEC_TYPE_ALAC => alac_config(extra),
            _ => BitstreamConfig::default(),
        }
    }
}

/// Read an AAC `AudioSpecificConfig` (ISO/IEC 14496-3 §1.6.2.1): 5 bits of
/// object type (escaping to 6 more when it is 31), a 4-bit index into the
/// frequency table — or 24 bits of explicit rate when that index is 15 — and a
/// 4-bit channel configuration.
///
/// The *core* rate is deliberately what this reports. A high-efficiency stream
/// carries an extension rate of twice it, but the AAC decoder in use implements
/// neither SBR nor PS, so what comes out of it is the core rate; reporting the
/// extension rate would make it play an octave low. Channel configuration 0
/// means "see the program config element", which is not in the ASC, so that is
/// reported as unknown rather than guessed at.
fn aac_config(asc: &[u8]) -> BitstreamConfig {
    /// ISO/IEC 14496-3 Table 1.18.
    const FREQS: [u32; 13] = [
        96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025,
        8_000, 7_350,
    ];
    /// Table 1.19, by channel configuration: 7 is 7.1 (eight channels).
    const CHANNELS: [usize; 8] = [0, 1, 2, 3, 4, 5, 6, 8];

    let bits = |at: &mut usize, n: usize| -> Option<u32> {
        let mut v = 0u32;
        for _ in 0..n {
            let byte = *asc.get(*at / 8)?;
            v = (v << 1) | u32::from((byte >> (7 - *at % 8)) & 1);
            *at += 1;
        }
        Some(v)
    };
    let read = || -> Option<BitstreamConfig> {
        let mut at = 0usize;
        if bits(&mut at, 5)? == 31 {
            bits(&mut at, 6)?;
        }
        let rate = match bits(&mut at, 4)? {
            15 => bits(&mut at, 24).filter(|r| *r > 0),
            index => FREQS.get(index as usize).copied(),
        };
        let channels = CHANNELS
            .get(bits(&mut at, 4)? as usize)
            .copied()
            .filter(|c| *c > 0);
        Some(BitstreamConfig { rate, channels })
    };
    read().unwrap_or_default()
}

/// Read an ALAC magic cookie (`ALACSpecificConfig`, Apple's
/// `ALACMagicCookieDescription.txt`): a fixed 24-byte layout whose ninth field
/// is the channel count and whose last is the sample rate, big-endian.
fn alac_config(cookie: &[u8]) -> BitstreamConfig {
    if cookie.len() < 24 {
        return BitstreamConfig::default();
    }
    let rate = u32::from_be_bytes([cookie[20], cookie[21], cookie[22], cookie[23]]);
    BitstreamConfig {
        rate: (rate > 0).then_some(rate),
        channels: Some(usize::from(cookie[9])).filter(|c| *c > 0),
    }
}

/// Gapless bounds an ISO-BMFF edit list states for the audio track.
///
/// AAC carries encoder priming — 1024 frames for a plain LC stream — at the
/// head of the *media*, and an MP4 does not remove it from the sample table.
/// It puts an edit list on the track instead: "start 1024 samples in, and last
/// exactly this long". ffmpeg, afconvert and iTunes all write one; the fixture
/// `tone-aac.m4a` has `media_time = 1024`, `segment_duration = 500` in a
/// 1000 Hz movie timescale.
///
/// Symphonia's ISO-MP4 reader ignores `edts` entirely, so without reading it
/// here every AAC file starts 21 ms late and runs 12 ms long. On its own that
/// is inaudible; against the same master in another container — which is the
/// entire point of this application — it is a flam and a false verdict about
/// which encode is tighter (SPEC §17).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct IsoEdit {
    /// Frames of priming to discard from the head, at the track's rate.
    skip: u64,
    /// Frames the edit says the track lasts, at the track's rate.
    length: Option<u64>,
}

/// Header of the box at `at`: its type, and the range of its body.
///
/// Boxes are read with seeks rather than by loading the file: `moov` sits at the
/// end of a streamed MP4 and can be megabytes away from the audio.
fn iso_box(file: &mut File, at: u64, end: u64) -> Option<([u8; 4], u64, u64)> {
    use std::io::{Seek, SeekFrom};
    if at.checked_add(8)? > end {
        return None;
    }
    file.seek(SeekFrom::Start(at)).ok()?;
    let mut header = [0u8; 8];
    file.read_exact(&mut header).ok()?;
    let mut size = u64::from(u32::from_be_bytes([
        header[0], header[1], header[2], header[3],
    ]));
    let kind = [header[4], header[5], header[6], header[7]];
    let mut body = at + 8;
    match size {
        // 1: the real size is a 64-bit field after the type.
        1 => {
            let mut large = [0u8; 8];
            file.read_exact(&mut large).ok()?;
            size = u64::from_be_bytes(large);
            body = at + 16;
        }
        // 0: "to the end of the enclosing box", i.e. everything left.
        0 => size = end - at,
        _ => {}
    }
    let box_end = at.checked_add(size)?.min(end);
    // A box that does not advance would loop forever in `iso_child`.
    (box_end >= body && box_end > at).then_some((kind, body, box_end))
}

/// Body range of the first `kind` box directly inside `[at, end)`.
fn iso_child(file: &mut File, kind: &[u8; 4], at: u64, end: u64) -> Option<(u64, u64)> {
    let mut at = at;
    while let Some((found, body, box_end)) = iso_box(file, at, end) {
        if &found == kind {
            return Some((body, box_end));
        }
        at = box_end;
    }
    None
}

/// Read `N` bytes at `at`, if they are inside `end`.
fn iso_bytes<const N: usize>(file: &mut File, at: u64, end: u64) -> Option<[u8; N]> {
    use std::io::{Seek, SeekFrom};
    if at.checked_add(N as u64)? > end {
        return None;
    }
    file.seek(SeekFrom::Start(at)).ok()?;
    let mut buf = [0u8; N];
    file.read_exact(&mut buf).ok()?;
    Some(buf)
}

/// The timescale of an `mvhd` or `mdhd`: both put it after two timestamps whose
/// width is the box version's (ISO/IEC 14496-12 §8.2.2, §8.4.2).
fn iso_timescale(file: &mut File, body: u64, end: u64) -> Option<u32> {
    let version = iso_bytes::<1>(file, body, end)?[0];
    let at = if version == 1 { body + 20 } else { body + 12 };
    let ts = u32::from_be_bytes(iso_bytes::<4>(file, at, end)?);
    (ts > 0).then_some(ts)
}

/// The single edit an `elst` states, as `(segment_duration, media_time)`.
///
/// Only a one-entry list is honoured. Several entries express things this
/// player does not implement — an empty edit for a silent lead-in, a gap in
/// the middle, playback at a rate other than 1.0 — and acting on the first of
/// them would be a guess. An unhandled edit list is a small length error; a
/// misread one moves the audio.
fn iso_elst(file: &mut File, body: u64, end: u64) -> Option<(u64, i64)> {
    let head = iso_bytes::<8>(file, body, end)?;
    let version = head[0];
    let entries = u32::from_be_bytes([head[4], head[5], head[6], head[7]]);
    if entries != 1 {
        return None;
    }
    let at = body + 8;
    if version == 1 {
        let raw = iso_bytes::<16>(file, at, end)?;
        let duration = u64::from_be_bytes(raw[..8].try_into().ok()?);
        let media_time = i64::from_be_bytes(raw[8..].try_into().ok()?);
        Some((duration, media_time))
    } else {
        let raw = iso_bytes::<8>(file, at, end)?;
        let duration = u64::from(u32::from_be_bytes(raw[..4].try_into().ok()?));
        let media_time = i64::from(i32::from_be_bytes(raw[4..].try_into().ok()?));
        Some((duration, media_time))
    }
}

/// Rescale `value` from `from` units per second to `to`, saturating.
fn rescale(value: u64, from: u32, to: u32) -> u64 {
    if from == to || from == 0 {
        return value;
    }
    let scaled = u128::from(value) * u128::from(to) / u128::from(from);
    u64::try_from(scaled).unwrap_or(u64::MAX)
}

/// Read the edit list of the first audio track of an ISO-BMFF file, expressed
/// in frames at `rate`. See [`IsoEdit`].
fn iso_edit(path: &Path, rate: u32) -> Option<IsoEdit> {
    let mut file = File::open(path).ok()?;
    let end = file.metadata().ok()?.len();
    let (moov, moov_end) = iso_child(&mut file, b"moov", 0, end)?;
    let movie_timescale = iso_child(&mut file, b"mvhd", moov, moov_end)
        .and_then(|(body, box_end)| iso_timescale(&mut file, body, box_end));

    let mut at = moov;
    while let Some((kind, trak, trak_end)) = iso_box(&mut file, at, moov_end) {
        at = trak_end;
        if &kind != b"trak" {
            continue;
        }
        let Some((mdia, mdia_end)) = iso_child(&mut file, b"mdia", trak, trak_end) else {
            continue;
        };
        // `hdlr` is what distinguishes the audio track from the video one in
        // the MOV/MP4 fixtures, which carry both.
        let audio = iso_child(&mut file, b"hdlr", mdia, mdia_end)
            .and_then(|(body, box_end)| iso_bytes::<4>(&mut file, body + 8, box_end))
            .map(|handler| &handler == b"soun")
            .unwrap_or(false);
        if !audio {
            continue;
        }
        let media_timescale = iso_child(&mut file, b"mdhd", mdia, mdia_end)
            .and_then(|(body, box_end)| iso_timescale(&mut file, body, box_end))?;
        let (edts, edts_end) = iso_child(&mut file, b"edts", trak, trak_end)?;
        let (elst, elst_end) = iso_child(&mut file, b"elst", edts, edts_end)?;
        let (segment, media_time) = iso_elst(&mut file, elst, elst_end)?;
        // A negative `media_time` is an empty edit: silence before the media
        // starts. Nothing to trim, and inserting the silence is not this
        // player's job.
        let skip = rescale(u64::try_from(media_time).ok()?, media_timescale, rate);
        // `segment_duration` is in the *movie* timescale, `media_time` in the
        // track's own (ISO/IEC 14496-12 §8.6.6).
        let length = movie_timescale
            .map(|ts| rescale(segment, ts, rate))
            .filter(|frames| *frames > 0);
        return Some(IsoEdit { skip, length });
    }
    None
}

fn probe_coded(path: &Path, sniffed: Option<Container>) -> Result<(TrackInfo, Source)> {
    let file = File::open(path)?;
    let size_bytes = file.metadata().map(|m| m.len()).unwrap_or(0);
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        // Only ever a hint: Symphonia's probe scores every registered format
        // against the file's own bytes, so a lying extension costs a few
        // microseconds and changes nothing else.
        hint.with_extension(ext);
    }

    let mut probed = symphonia::default::get_probe().format(
        &hint,
        mss,
        &FormatOptions {
            enable_gapless: true,
            ..Default::default()
        },
        &MetadataOptions::default(),
    )?;

    let mut format = probed.format;

    // The **first audio track**, which in a video container means skipping the
    // picture entirely (SPEC §17). A track is audio if it declares a sample
    // rate and we hold a decoder for its codec; a video or subtitle track
    // fails both tests, and so does an audio track in a codec this build was
    // not compiled with — in which case saying "no audio track we can play" is
    // more use than failing later with a codec error.
    let track = format
        .tracks()
        .iter()
        .find(|t| {
            t.codec_params.codec != CODEC_TYPE_NULL
                && t.codec_params.sample_rate.is_some()
                && codecs().get_codec(t.codec_params.codec).is_some()
        })
        .or_else(|| {
            format
                .tracks()
                .iter()
                .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        })
        .ok_or_else(|| Error::NoAudioTrack(path.to_path_buf()))?;
    let track_id = track.id;
    let params = track.codec_params.clone();
    let track_count = format.tracks().len();
    if track_count > 1 {
        log::debug!(
            "\"{}\": {track_count} tracks, playing track {track_id} and ignoring the rest",
            file_name_of(path)
        );
    }

    let container_rate = params
        .sample_rate
        .ok_or_else(|| Error::Unsupported("stream does not declare a sample rate".into()))?;
    // What the codec's own configuration says, which for an MP4 is the only
    // place a channel count appears at all and is the authority on the rate
    // when the two disagree (SPEC §17). See `BitstreamConfig`.
    let coded = BitstreamConfig::of(&params);
    let src_channels = stored_channels(
        params
            .channels
            .map(|c| c.count())
            .or_else(|| params.channel_layout.map(|l| l.into_channels().count())),
        coded.channels,
    )?;

    let decoder = codecs().make(&params, &DecoderOptions { verify: false })?;

    let src_rate = match coded.rate {
        Some(rate) if rate != container_rate => {
            log::warn!(
                "\"{}\": the container declares {container_rate} Hz but the bitstream says \
                 {rate} Hz; using {rate} Hz",
                file_name_of(path)
            );
            rate
        }
        _ => container_rate,
    };

    let codec_name = codecs()
        .get_codec(params.codec)
        .map(|d| d.short_name.to_string())
        .unwrap_or_else(|| "pcm".to_string());

    // Length. `n_frames` is *not* always a frame count: Matroska/WebM report
    // the segment duration in time-base units (milliseconds, typically), so a
    // half-second file would otherwise read as 508 frames — 10 ms — and the
    // deck would allocate for 10 ms of audio. Where a time base exists it is
    // authoritative, and for every other container it is 1/sample_rate, which
    // makes this the same arithmetic as before.
    let declared = params.n_frames.unwrap_or(0);
    let declared_secs = if declared == 0 {
        0.0
    } else {
        match params.time_base {
            // A time base of 1/container_rate is not independent evidence: it
            // is the rate we just overruled, wearing a different hat. Counting
            // the declared frames at the rate the bitstream really runs at is
            // what keeps the length right when the two disagree.
            Some(tb) if tb.numer == 1 && tb.denom == container_rate => {
                declared as f64 / src_rate as f64
            }
            Some(tb) => {
                let t = tb.calc_time(declared);
                t.seconds as f64 + t.frac
            }
            None => declared as f64 / src_rate as f64,
        }
    };
    let mut expected_frames = (declared_secs * src_rate as f64).round() as u64;

    // Gapless bounds (SPEC §17). Symphonia's gapless machinery marks packets
    // with `trim_start`/`trim_end` and the decoders honour those, but it cannot
    // mark the packets of the *first* page of an Ogg stream: they are queued
    // before the demuxer has established the stream's start bound. What is left
    // over has to be finished off here.
    //
    // Only Opus gets its head trimmed, because only Opus states the encoder
    // delay in band: `OpusHead` carries the pre-skip explicitly. Everywhere
    // else `CodecParameters::delay` is *derived* from a granule shortfall, and
    // when a whole stream fits in one Ogg page — anything under about a second
    // as written by ffmpeg — the demuxer cannot tell head padding from tail
    // padding and blames the head. Trimming on that guess would eat real audio.
    // The declared length is safe either way: it ends the file where the
    // container says it ends instead of wherever the last packet happens to
    // stop, which is what removes the padding for the other formats.
    let mut head_delay = 0u64;
    /* Everything from here to the end of the length bound below *is* the decode
    semantics the loudness cache keys measurements on: change what is trimmed
    or where the track ends and every cached LUFS / true peak for every AAC,
    MP4 and Opus file becomes a number this build would not produce. Bump
    `DECODE_SEMANTICS` in the same commit. */
    if params.codec == CODEC_TYPE_OPUS {
        if let Some(pre_skip) = crate::opus::pre_skip(params.extra_data.as_deref()) {
            // The Ogg mapper's timeline counts the pre-skip as audio, so the
            // real length is that much shorter than the granule says.
            head_delay = u64::from(pre_skip);
            expected_frames = expected_frames.saturating_sub(u64::from(pre_skip));
        }
    }
    // An MP4/MOV states its priming in an edit list instead, which Symphonia
    // does not read (see [`IsoEdit`]). Skipped when the head delay is already
    // known in band — Opus in MP4 would otherwise be trimmed twice, once for
    // the pre-skip the edit list is describing.
    if head_delay == 0 && matches!(sniffed, Some(Container::Mp4) | Some(Container::Mov)) {
        if let Some(edit) = iso_edit(path, src_rate).filter(|e| e.skip > 0) {
            head_delay = edit.skip;
            let after_head = expected_frames.saturating_sub(edit.skip);
            expected_frames = match edit.length {
                // The edit cannot claim more audio than the media holds.
                Some(length) => length.min(after_head),
                None => after_head,
            };
        }
    }
    let has_gapless_metadata =
        head_delay > 0 || params.delay.unwrap_or(0) > 0 || params.padding.unwrap_or(0) > 0;
    let limit = if has_gapless_metadata && expected_frames > 0 {
        Some(expected_frames)
    } else {
        None
    };
    let duration_secs = if declared == 0 {
        0.0
    } else {
        expected_frames as f64 / src_rate as f64
    };

    // Tags: prefer the container metadata, fall back to the probe's side data.
    let mut title = None;
    let mut artist = None;
    let mut album = None;
    let mut collect = |rev: &symphonia::core::meta::MetadataRevision| {
        for tag in rev.tags() {
            match tag.std_key {
                Some(StandardTagKey::TrackTitle) if title.is_none() => {
                    title = Some(tag.value.to_string())
                }
                Some(StandardTagKey::Artist) | Some(StandardTagKey::AlbumArtist)
                    if artist.is_none() =>
                {
                    artist = Some(tag.value.to_string())
                }
                Some(StandardTagKey::Album) if album.is_none() => {
                    album = Some(tag.value.to_string())
                }
                _ => {}
            }
        }
    };
    if let Some(rev) = format.metadata().current() {
        collect(rev);
    }
    if let Some(mut meta) = probed.metadata.get() {
        if let Some(rev) = meta.skip_to_latest() {
            collect(rev);
        }
    }

    let is_lossless = matches!(
        codec_name.as_str(),
        "flac" | "alac" | "pcm_s16le" | "pcm_s24le" | "pcm_s32le" | "pcm_f32le" | "pcm" | "adpcm"
    ) || codec_name.starts_with("pcm");

    let bitrate_kbps = if duration_secs > 0.1 && size_bytes > 0 {
        Some(((size_bytes as f64 * 8.0) / duration_secs / 1000.0).round() as u32)
    } else {
        None
    };

    let info = TrackInfo {
        path: path.to_string_lossy().to_string(),
        file_name: file_name_of(path),
        duration_secs,
        sample_rate: src_rate,
        channels: src_channels as u16,
        bits_per_sample: params.bits_per_sample,
        codec: codec_name,
        // Content, not extension. Falling back to the extension only when the
        // sniffer draws a blank keeps something in the badge for the odd
        // container Symphonia knows and we do not.
        container: sniffed.map(|c| c.label().to_string()).unwrap_or_else(|| {
            path.extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_uppercase()
        }),
        bitrate_kbps,
        is_lossless,
        size_bytes,
        title,
        artist,
        album,
        synth_bank: None,
        render_key: None,
    };

    Ok((
        info,
        Source {
            reader: SourceReader::Coded(Coded {
                format,
                decoder,
                track_id,
                sample_buf: None,
                last_spec: None,
                head_remaining: head_delay,
                limit,
                emitted: 0,
            }),
            src_rate,
            src_channels,
            expected_frames,
        },
    ))
}

/// Probe `path` and start decoding it in the background at `target_rate`.
///
/// Returns as soon as the header has been parsed - typically well under a
/// millisecond - which is what makes single-click playback feel instant.
pub fn open(path: &Path, target_rate: u32, budget_bytes: usize) -> Result<DecodeHandle> {
    open_with(path, target_rate, budget_bytes, &DecodeOptions::default())
}

/// [`open`] with explicit options — the app layer's entry point, because the
/// user's SoundFont choice lives in settings and only it knows about it.
///
/// Identical for every non-MIDI file; for a `.mid` this selects the bank the
/// render uses, which is reflected in [`TrackInfo::synth_bank`] and
/// [`TrackInfo::render_key`].
pub fn open_with(
    path: &Path,
    target_rate: u32,
    budget_bytes: usize,
    options: &DecodeOptions,
) -> Result<DecodeHandle> {
    if target_rate == 0 {
        // Every capacity and every resampler ratio below is derived from this,
        // so a zero rate would ask rubato for a ratio of 0 and allocate a
        // one-frame buffer. Reject it here rather than let it become a panic on
        // the decode thread.
        return Err(Error::Unsupported(
            "target sample rate must be greater than zero".into(),
        ));
    }
    let (info, source) = probe_inner(path, options, target_rate)?;

    let out_channels = source.src_channels.clamp(1, 2);
    let ratio = target_rate as f64 / source.src_rate as f64;
    let bit_transparent = target_rate == source.src_rate;

    // Work out how much to pre-allocate. A little slack absorbs resampler
    // rounding and containers that under-report their length.
    let estimated_out_frames = if source.expected_frames > 0 {
        ((source.expected_frames as f64 * ratio).ceil() as usize) + target_rate as usize / 4
    } else {
        estimate_frames_from_size(&info, target_rate)
    };

    let bytes_per_frame = out_channels * std::mem::size_of::<f32>();
    let max_frames = (budget_bytes / bytes_per_frame).max(target_rate as usize);
    let capacity_frames = estimated_out_frames.min(max_frames).max(1);
    let truncated = estimated_out_frames > max_frames;

    let (pcm, writer) = SharedPcm::new(
        out_channels,
        capacity_frames,
        estimated_out_frames.min(capacity_frames),
    );
    let waveform = Arc::new(Waveform::new(target_rate, capacity_frames));
    let status = Arc::new(DecodeStatus::new());
    if truncated {
        status.truncated.store(true, Ordering::Release);
    }

    let handle = DecodeHandle {
        info: info.clone(),
        pcm,
        waveform: Arc::clone(&waveform),
        status: Arc::clone(&status),
        stored_rate: target_rate,
        bit_transparent,
    };

    let path_owned: PathBuf = path.to_path_buf();
    let thread_status = Arc::clone(&status);
    let thread_waveform = Arc::clone(&waveform);
    // File name only: log lines are read out of bug reports, and a full path is
    // both noise and somebody's directory tree. The full path goes out at
    // `debug` on the failure path only.
    let name = info.file_name.clone();
    log::debug!(
        "decoding \"{name}\": {} Hz {} ch -> {target_rate} Hz {out_channels} ch, \
         {capacity_frames} frames reserved{}",
        source.src_rate,
        source.src_channels,
        if truncated { " (budget capped)" } else { "" }
    );
    std::thread::Builder::new()
        .name(format!(
            "onyx-decode-{}",
            path.file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_default()
        ))
        .spawn(move || {
            let result = decode_loop(
                source,
                writer,
                out_channels,
                target_rate,
                &thread_waveform,
                &thread_status,
                &name,
            );
            if let Err(e) = result {
                if !thread_status.is_cancelled() {
                    // The user asked for this file and will not get it: the host
                    // turns the stored message into a toast, and this is the
                    // matching line in the log file.
                    log::error!("could not decode \"{name}\": {e}");
                    log::debug!("failed decode was {}", path_owned.display());
                    *thread_status.error.lock() = Some(e.to_string());
                    thread_status.outcome.store(2, Ordering::Release);
                }
            } else {
                thread_status.outcome.store(1, Ordering::Release);
            }
            thread_status.finished.store(true, Ordering::Release);
        })
        .map_err(|e| Error::Other(format!("cannot spawn decode thread: {e}")))?;

    Ok(handle)
}

fn estimate_frames_from_size(info: &TrackInfo, target_rate: u32) -> usize {
    // Unknown duration: guess from size and bitrate, then clamp to something
    // sane so a broken header cannot ask for gigabytes.
    let secs = match info.bitrate_kbps {
        Some(kbps) if kbps > 0 => (info.size_bytes as f64 * 8.0) / (kbps as f64 * 1000.0),
        _ => {
            // Assume 1000 kbps if we know nothing at all.
            (info.size_bytes as f64 * 8.0) / 1_000_000.0
        }
    };
    let secs = secs.clamp(30.0, 3.0 * 3600.0);
    (secs * target_rate as f64) as usize
}

struct Analysis {
    loudness: LoudnessMeter,
    true_peak: TruePeak,
    sample_peak: f32,
    stereo_scratch: Vec<f32>,
}

impl Analysis {
    fn new(rate: u32) -> Self {
        Analysis {
            loudness: LoudnessMeter::new(rate as f64),
            true_peak: TruePeak::new(2),
            sample_peak: 0.0,
            stereo_scratch: Vec::with_capacity(RESAMPLE_CHUNK * 2),
        }
    }

    fn feed(&mut self, interleaved: &[f32], channels: usize) {
        self.stereo_scratch.clear();
        let frames = interleaved.len() / channels.max(1);
        for f in 0..frames {
            let (l, r) = if channels >= 2 {
                (interleaved[f * channels], interleaved[f * channels + 1])
            } else {
                let s = interleaved[f * channels];
                (s, s)
            };
            self.sample_peak = self.sample_peak.max(l.abs()).max(r.abs());
            self.stereo_scratch.push(l);
            self.stereo_scratch.push(r);
        }
        self.loudness.process(&self.stereo_scratch);
        self.true_peak.process(&self.stereo_scratch);
    }

    fn finish(mut self) -> LoudnessAnalysis {
        LoudnessAnalysis {
            integrated_lufs: self.loudness.integrated(),
            lra: self.loudness.lra(),
            true_peak_db: self.true_peak.peak_db(0).max(self.true_peak.peak_db(1)),
            sample_peak_db: crate::lin_to_db(self.sample_peak),
        }
    }
}

/// The one pipeline every source travels: pull → fold → resample → publish,
/// building waveform peaks and loudness on the way.
///
/// Coded audio and a MIDI render differ only in the [`SourceReader`] at the
/// top; from `pull` downwards this code cannot tell them apart, which is what
/// gives MIDI seeking, A/B, EQ and metering for free (SPEC §18).
fn decode_loop(
    source: Source,
    mut writer: PcmWriter,
    out_channels: usize,
    target_rate: u32,
    waveform: &Waveform,
    status: &DecodeStatus,
    name: &str,
) -> Result<()> {
    let Source {
        mut reader,
        src_rate,
        ..
    } = source;
    let needs_resample = target_rate != src_rate;

    // Synthesis happens *here*, on the decode thread, not in `open` — the UI
    // thread must never wait on a 30 MB SoundFont or a five-minute sequence.
    // Loading the bank happens here too: `probe` only peeks at the SoundFont's
    // header (see `midi::probe`), so nothing on the command thread has ever
    // parsed one. The cap keeps a MIDI render inside the same deck budget a
    // WAV gets.
    let src_frame_budget = if needs_resample {
        ((writer.remaining_frames() as f64 * src_rate as f64 / target_rate as f64).ceil() as usize)
            .max(1)
    } else {
        writer.remaining_frames()
    };
    reader.realise(src_frame_budget)?;
    if let SourceReader::Rendered(r) = &reader {
        // A render that filled its budget is truncated even if the declared
        // duration fitted, so the "budget capped" badge stays honest.
        if r.samples.len() / r.channels >= src_frame_budget {
            status.truncated.store(true, Ordering::Release);
        }
    }

    let mut wf = WaveformBuilder::new(waveform.bucket_frames());
    let mut analysis = Analysis::new(target_rate);

    let mut resampler = if needs_resample {
        let sinc_len = 256;
        let params = SincInterpolationParameters {
            sinc_len,
            f_cutoff: calculate_cutoff(sinc_len, WindowFunction::BlackmanHarris2),
            oversampling_factor: 256,
            interpolation: SincInterpolationType::Linear,
            window: WindowFunction::BlackmanHarris2,
        };
        Some(
            SincFixedIn::<f32>::new(
                target_rate as f64 / src_rate as f64,
                1.0,
                params,
                RESAMPLE_CHUNK,
                out_channels,
            )
            .map_err(|e| Error::Resample(e.to_string()))?,
        )
    } else {
        None
    };

    // Group delay: `SincFixedIn` is *already* delay compensated. Its internal
    // buffer is primed with `sinc_len` frames of history and `last_index`
    // starts negative, so output frame `k` is centred on input frame
    // `k / ratio`: an impulse at input frame n comes back out at output frame
    // round(n * ratio). `Resampler::output_delay()` reports the interpolator's
    // half length (139 frames for sinc_len 256 at 44.1 -> 48 kHz) and skipping
    // that many frames here would shift a resampled deck ~2.9 ms *early*
    // against a non-resampled one, which is precisely what A/B must not do.
    // Nothing to skip, therefore - but the plumbing stays so that swapping in
    // a resampler that is not compensated is a one-line change.
    let mut delay_to_skip = 0usize;

    // Input/output frame ledger for the resampled path. `SincFixedIn` always
    // consumes a full chunk, so the final `process_partial` call zero-pads the
    // leftovers and hands back a whole chunk worth of output. Without this
    // ledger the file would grow by up to one chunk (~23 ms at 44.1 -> 48 kHz)
    // of interpolator tail, which is exactly the kind of drift that breaks
    // sample-accurate A/B.
    let resample_ratio = target_rate as f64 / src_rate as f64;
    let mut in_frames_total: u64 = 0;
    let mut out_frames_total: u64 = 0;

    let mut planar_in: Vec<Vec<f32>> = vec![Vec::with_capacity(RESAMPLE_CHUNK * 2); out_channels];
    let mut planar_out: Vec<Vec<f32>> = resampler
        .as_ref()
        .map(|r| r.output_buffer_allocate(true))
        .unwrap_or_default();
    let mut interleaved_out: Vec<f32> = Vec::with_capacity(RESAMPLE_CHUNK * 2 * out_channels);

    let mut damage = Damage::default();
    let mut folded: Vec<f32> = Vec::with_capacity(8_192 * out_channels);

    loop {
        if status.is_cancelled() {
            return Ok(());
        }
        if writer.remaining_frames() == 0 {
            break;
        }

        folded.clear();
        let frames = match reader.pull(out_channels, &mut folded, &mut damage, name)? {
            Pull::Frames(n) => n,
            Pull::Skipped => continue,
            Pull::End => break,
        };
        if frames == 0 {
            continue;
        }

        if let Some(rs) = resampler.as_mut() {
            // Planar-ise, then drain in fixed chunks.
            for f in 0..frames {
                for c in 0..out_channels {
                    planar_in[c].push(folded[f * out_channels + c]);
                }
            }
            in_frames_total += frames as u64;
            while planar_in[0].len() >= rs.input_frames_next() {
                let need = rs.input_frames_next();
                let (_, produced) = rs
                    .process_into_buffer(
                        &planar_in
                            .iter()
                            .map(|c| &c[..need])
                            .collect::<Vec<&[f32]>>(),
                        &mut planar_out,
                        None,
                    )
                    .map_err(|e| Error::Resample(e.to_string()))?;
                for ch in planar_in.iter_mut().take(out_channels) {
                    ch.drain(..need);
                }
                out_frames_total += emit(
                    &planar_out,
                    produced,
                    out_channels,
                    &mut delay_to_skip,
                    usize::MAX,
                    &mut interleaved_out,
                    &mut writer,
                    &mut wf,
                    waveform,
                    &mut analysis,
                ) as u64;
                if writer.remaining_frames() == 0 {
                    break;
                }
            }
        } else {
            wf.push_interleaved(&folded, out_channels, waveform);
            analysis.feed(&folded, out_channels);
            writer.write_interleaved(&folded);
        }
    }

    // Flush the resampler tail so the last few milliseconds are not lost, but
    // no further than the input actually justifies (see the ledger above).
    if let Some(rs) = resampler.as_mut() {
        if !planar_in[0].is_empty() {
            let ideal_out = (in_frames_total as f64 * resample_ratio).round() as u64;
            let allowance = ideal_out.saturating_sub(out_frames_total) as usize;
            if allowance > 0 {
                match rs.process_partial_into_buffer(
                    Some(&planar_in.iter().map(|c| &c[..]).collect::<Vec<&[f32]>>()),
                    &mut planar_out,
                    None,
                ) {
                    Ok((_, produced)) => {
                        emit(
                            &planar_out,
                            produced,
                            out_channels,
                            &mut delay_to_skip,
                            allowance,
                            &mut interleaved_out,
                            &mut writer,
                            &mut wf,
                            waveform,
                            &mut analysis,
                        );
                    }
                    // Not fatal - we already have everything but the last few
                    // milliseconds - but it must not be silent either.
                    Err(e) => log::warn!(
                        "\"{name}\": the last few milliseconds were dropped, \
                         the resampler tail flush failed: {e}"
                    ),
                }
            }
        }
    }

    wf.flush(waveform);
    writer.finish();
    // One line per decode, with totals, at the severity that says "you have
    // audio, but it is not the whole file": a damaged or truncated file is
    // degraded-but-continuing, not a failure (the deck is playing).
    if damage.packets > 0 || damage.padded_frames > 0 {
        log::warn!(
            "\"{name}\" decoded with damage: {} unreadable packet(s) skipped, \
             {} frame(s) padded with silence - the file is truncated or corrupt",
            damage.packets,
            damage.padded_frames
        );
    }
    *status.analysis.lock() = Some(analysis.finish());
    Ok(())
}

/// Fold `frames` frames of `src_channels`-wide interleaved audio down to
/// `out_channels` and append them to `dst`.
///
/// Mono into stereo duplicates; anything wider than `out_channels` keeps the
/// front channels. A packet that is shorter than it claims is *padded with
/// silence* rather than indexed out of bounds - the decode thread must not
/// panic on a malformed file.
///
/// Returns how many frames had to be padded. This is deliberately *returned
/// rather than logged*: a file whose every packet is short would write a line
/// per packet, so [`decode_loop`] accumulates the count and logs one summary
/// when the decode finishes.
#[must_use]
fn fold_interleaved(
    samples: &[f32],
    frames: usize,
    src_channels: usize,
    out_channels: usize,
    dst: &mut Vec<f32>,
) -> usize {
    let src_channels = src_channels.max(1);
    let out_channels = out_channels.max(1);
    let available = samples.len() / src_channels;
    let full = frames.min(available);
    if src_channels == out_channels {
        dst.extend_from_slice(&samples[..full * src_channels]);
    } else {
        for f in 0..full {
            for c in 0..out_channels {
                // Mono source, stereo store: duplicate rather than pad.
                dst.push(samples[f * src_channels + c.min(src_channels - 1)]);
            }
        }
    }
    if full < frames {
        dst.resize(dst.len() + (frames - full) * out_channels, 0.0);
    }
    frames - full
}

/// Interleave `frames` of planar resampler output and publish it.
///
/// `delay_to_skip` swallows the interpolator's group delay (once, at the start
/// of the stream); `max_frames` caps how much is published, which the flush
/// path uses to drop the zero-padded tail. Returns the frames published.
#[allow(clippy::too_many_arguments)]
fn emit(
    planar: &[Vec<f32>],
    frames: usize,
    channels: usize,
    delay_to_skip: &mut usize,
    max_frames: usize,
    scratch: &mut Vec<f32>,
    writer: &mut PcmWriter,
    wf: &mut WaveformBuilder,
    waveform: &Waveform,
    analysis: &mut Analysis,
) -> usize {
    let mut start = 0usize;
    if *delay_to_skip > 0 {
        let skip = (*delay_to_skip).min(frames);
        *delay_to_skip -= skip;
        start = skip;
    }
    let end = frames.min(start.saturating_add(max_frames));
    if start >= end {
        return 0;
    }
    scratch.clear();
    // Planar -> interleaved. Indexed rather than iterator-chained because the
    // inner stride is over *channels*, not over one contiguous slice.
    #[allow(clippy::needless_range_loop)]
    for f in start..end {
        for c in 0..channels {
            scratch.push(planar[c][f]);
        }
    }
    wf.push_interleaved(scratch, channels, waveform);
    analysis.feed(scratch, channels);
    // The writer clamps at the memory budget, so report what it really took.
    writer.write_interleaved(scratch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Minimal 16-bit PCM WAV writer so the decode path can be tested without
    /// shipping binary fixtures.
    fn write_wav(path: &Path, sample_rate: u32, channels: u16, samples: &[f32]) {
        let bits = 16u16;
        let data_len = (samples.len() * 2) as u32;
        let mut f = File::create(path).unwrap();
        f.write_all(b"RIFF").unwrap();
        f.write_all(&(36 + data_len).to_le_bytes()).unwrap();
        f.write_all(b"WAVEfmt ").unwrap();
        f.write_all(&16u32.to_le_bytes()).unwrap();
        f.write_all(&1u16.to_le_bytes()).unwrap();
        f.write_all(&channels.to_le_bytes()).unwrap();
        f.write_all(&sample_rate.to_le_bytes()).unwrap();
        let byte_rate = sample_rate * channels as u32 * (bits as u32 / 8);
        f.write_all(&byte_rate.to_le_bytes()).unwrap();
        let block_align = channels * bits / 8;
        f.write_all(&block_align.to_le_bytes()).unwrap();
        f.write_all(&bits.to_le_bytes()).unwrap();
        f.write_all(b"data").unwrap();
        f.write_all(&data_len.to_le_bytes()).unwrap();
        for s in samples {
            let v = (s.clamp(-1.0, 1.0) * 32_767.0) as i16;
            f.write_all(&v.to_le_bytes()).unwrap();
        }
        f.flush().unwrap();
    }

    fn tmp(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("onyx-test-{}-{name}", std::process::id()));
        p
    }

    fn tone(freq: f32, sr: u32, secs: f32, channels: usize) -> Vec<f32> {
        let n = (sr as f32 * secs) as usize;
        (0..n)
            .flat_map(|i| {
                let s = 0.5 * (2.0 * std::f32::consts::PI * freq * i as f32 / sr as f32).sin();
                std::iter::repeat_n(s, channels)
            })
            .collect()
    }

    fn wait_for(handle: &DecodeHandle) {
        for _ in 0..600 {
            if handle.status.is_finished() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("decode did not finish in time");
    }

    #[test]
    fn probes_and_decodes_a_wav() {
        let path = tmp("probe.wav");
        write_wav(&path, 48_000, 2, &tone(1_000.0, 48_000, 1.0, 2));

        let info = probe(&path).unwrap();
        assert_eq!(info.sample_rate, 48_000);
        assert_eq!(info.channels, 2);
        assert!((info.duration_secs - 1.0).abs() < 0.01);
        assert!(info.is_lossless);

        let h = open(&path, 48_000, DEFAULT_DECK_BUDGET_BYTES).unwrap();
        assert!(h.bit_transparent);
        wait_for(&h);
        assert!(h.status.error().is_none());
        let frames = h.pcm.frames_ready();
        assert!(
            (frames as i64 - 48_000).abs() < 64,
            "decoded {frames} frames"
        );
        assert!(h.waveform.len() > 100);
        let a = h.status.analysis().unwrap();
        // A 1 kHz sine of amplitude 0.5 on both legs must read -6.01 LUFS:
        //   LUFS = -0.691 + 10*log10(sum_ch mean-square) + K(1 kHz)
        //        = -0.691 + 10*log10(2 * 0.5^2/2) + 0.698
        //        = -0.691 - 6.021 + 0.698 = -6.014
        // (K(1 kHz) = +0.698 dB is what the -0.691 offset exists to cancel; the
        // same design reproduces the BS.1770 calibration point of -3.01 LKFS
        // for a 0 dBFS sine on one channel.) The original assertion asked for
        // -20..-8 LUFS, which no correct BS.1770 meter can produce here.
        assert!(
            (a.integrated_lufs + 6.014).abs() < 0.15,
            "expected ~-6.01 LUFS, got {a:?}"
        );
        assert!((a.sample_peak_db - crate::lin_to_db(0.5)).abs() < 0.2);
        let _ = std::fs::remove_file(&path);
    }

    /// The decode-side tripwire named on [`DECODE_SEMANTICS`] (SPEC §8).
    ///
    /// Every number below is a decode *semantic*: what is trimmed from the
    /// head, how long the result is, whether the sample path is bit-transparent
    /// or resampled, and what the meter then reads. The loudness cache in
    /// `src-tauri/src/cache.rs` keys its records on `DECODE_SEMANTICS` precisely
    /// so that changing one of these is not silent — so if you deliberately
    /// change an expectation here, bump the constant in the same commit, or
    /// every measurement taken by the old behaviour keeps being served.
    #[test]
    fn changing_what_a_decode_produces_must_bump_decode_semantics() {
        // Head trimming, the part that moved most recently. `OpusHead`'s
        // declared pre-skip is the authority (RFC 7845 §4.2)...
        let mut head = b"OpusHead\x01\x02".to_vec();
        head.extend_from_slice(&312u16.to_le_bytes());
        assert_eq!(crate::opus::pre_skip(Some(&head)), Some(312));
        // ...and AAC/MP4 priming comes from the audio track's edit list, pinned
        // in detail by `the_edit_list_of_the_audio_track_is_what_is_read`.

        // Then the sample path. A 44.1 kHz source on a 48 kHz engine is
        // resampled, keeps its length, and measures what BS.1770 says a 1 kHz
        // sine of amplitude 0.5 on both legs measures (see
        // `probes_and_decodes_a_wav` for the arithmetic).
        let path = tmp("semantics.wav");
        write_wav(&path, 44_100, 2, &tone(1_000.0, 44_100, 1.0, 2));
        let h = open(&path, 48_000, DEFAULT_DECK_BUDGET_BYTES).unwrap();
        assert!(!h.bit_transparent, "44.1 kHz on a 48 kHz engine resamples");
        assert_eq!(h.stored_rate, 48_000);
        wait_for(&h);
        let frames = h.pcm.frames_ready();
        assert!(
            (frames as i64 - 48_000).abs() < 400,
            "a second of audio must stay a second: {frames} frames"
        );
        let a = h.status.analysis().unwrap();
        assert!(
            (a.integrated_lufs + 6.014).abs() < 0.15,
            "loudness maths moved: {a:?}"
        );
        assert!(
            (a.sample_peak_db - crate::lin_to_db(0.5)).abs() < 0.3,
            "peak maths moved: {a:?}"
        );

        // The same file at its own rate is bit-transparent and exactly as long
        // as the container says. That difference — same file, two rates, two
        // sets of numbers — is why the cache key carries the rate as well.
        let h = open(&path, 44_100, DEFAULT_DECK_BUDGET_BYTES).unwrap();
        assert!(
            h.bit_transparent,
            "a decode at the source rate must not touch the samples"
        );
        assert_eq!(h.stored_rate, 44_100);
        wait_for(&h);
        assert_eq!(h.pcm.frames_ready(), 44_100);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn resamples_to_the_engine_rate_and_stays_aligned() {
        let path = tmp("resample.wav");
        write_wav(&path, 44_100, 2, &tone(440.0, 44_100, 2.0, 2));
        let h = open(&path, 48_000, DEFAULT_DECK_BUDGET_BYTES).unwrap();
        assert!(!h.bit_transparent);
        wait_for(&h);
        let frames = h.pcm.frames_ready();
        // 2 s at 48 kHz, allow a couple of ms for the interpolator tail.
        assert!(
            (frames as i64 - 96_000).abs() < 400,
            "expected ~96000 frames, got {frames}"
        );

        // The resampler group delay must be compensated, otherwise a resampled
        // deck drifts against a bit-transparent one and A/B stops being a
        // comparison of the *same* moment in the music.
        //
        // NB: the original assertion here demanded |s| < 0.15 for the first 16
        // output samples. That can never hold, aligned or not: a 440 Hz tone of
        // amplitude 0.5 sampled at 48 kHz reaches 0.5*sin(2*pi*440*15/48000) =
        // 0.380 by sample 15. What alignment actually means is that output
        // sample i equals the source tone evaluated at i/48000, so that is what
        // we test - and we test it against every plausible shift, so a
        // one-sample-off implementation still fails.
        let ideal =
            |i: usize| 0.5 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin();
        let head: Vec<f32> = (0..512).map(|i| h.pcm.frame_stereo(i)[0]).collect();
        let err_at = |shift: usize| -> f32 {
            (0..256)
                .map(|i| (head[i + shift] - ideal(i)).abs())
                .fold(0.0f32, f32::max)
        };
        let (best_shift, best_err) = (0..256)
            .map(|s| (s, err_at(s)))
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
            .unwrap();
        assert_eq!(
            best_shift, 0,
            "resampled deck is offset by {best_shift} frames (err {best_err})"
        );
        assert!(
            err_at(0) < 0.02,
            "resampled head does not track the source tone: {:?}",
            &head[..16]
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn mono_files_are_stored_as_one_channel_and_folded_up() {
        let path = tmp("mono.wav");
        write_wav(&path, 48_000, 1, &tone(1_000.0, 48_000, 0.5, 1));
        let h = open(&path, 48_000, DEFAULT_DECK_BUDGET_BYTES).unwrap();
        wait_for(&h);
        assert_eq!(h.pcm.channels(), 1);
        let f = h.pcm.frame_stereo(1_000);
        assert_eq!(f[0], f[1]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn playback_can_start_before_decoding_finishes() {
        let path = tmp("progressive.wav");
        write_wav(&path, 48_000, 2, &tone(1_000.0, 48_000, 20.0, 2));
        let h = open(&path, 48_000, DEFAULT_DECK_BUDGET_BYTES).unwrap();
        // Wait for *some* audio, not all of it.
        let mut ready = 0;
        for _ in 0..500 {
            ready = h.pcm.frames_ready();
            if ready > 4_800 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(ready > 4_800, "only {ready} frames after waiting");
        wait_for(&h);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn budget_truncates_instead_of_exhausting_memory() {
        let path = tmp("budget.wav");
        write_wav(&path, 48_000, 2, &tone(1_000.0, 48_000, 5.0, 2));
        // 1 MiB budget => 131072 frames at 2ch f32.
        let h = open(&path, 48_000, 1024 * 1024).unwrap();
        wait_for(&h);
        assert!(h.status.is_truncated());
        assert!(h.pcm.frames_ready() <= 131_072);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn cancelling_stops_the_decode_thread() {
        let path = tmp("cancel.wav");
        write_wav(&path, 48_000, 2, &tone(1_000.0, 48_000, 30.0, 2));
        let h = open(&path, 48_000, DEFAULT_DECK_BUDGET_BYTES).unwrap();
        h.status.cancel();
        wait_for(&h);
        assert!(h.status.is_cancelled());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rejects_nonsense_files() {
        let path = tmp("garbage.wav");
        std::fs::write(&path, b"not audio at all").unwrap();
        assert!(open(&path, 48_000, DEFAULT_DECK_BUDGET_BYTES).is_err());
        assert!(probe(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }

    /// SPEC §9.3/§9.6. None of these may panic, hang, or report success on a
    /// failure: a missing file, an empty file, a nonsense rate, a file with a
    /// valid header and no audio, a single-frame file, and a file whose `data`
    /// chunk claims more bytes than it carries.
    #[test]
    fn degenerate_files_are_errors_or_empty_but_never_panics() {
        // Missing file.
        let missing = tmp("does-not-exist.wav");
        let _ = std::fs::remove_file(&missing);
        assert!(probe(&missing).is_err());
        assert!(open(&missing, 48_000, DEFAULT_DECK_BUDGET_BYTES).is_err());

        // Completely empty file.
        let empty = tmp("empty.wav");
        std::fs::write(&empty, b"").unwrap();
        assert!(open(&empty, 48_000, DEFAULT_DECK_BUDGET_BYTES).is_err());
        let _ = std::fs::remove_file(&empty);

        // A rate of zero is nonsense and must not reach the resampler.
        let ok = tmp("zero-rate.wav");
        write_wav(&ok, 48_000, 2, &tone(1_000.0, 48_000, 0.05, 2));
        assert!(open(&ok, 0, DEFAULT_DECK_BUDGET_BYTES).is_err());
        let _ = std::fs::remove_file(&ok);

        // Zero-length audio: a real header, no samples.
        let zero = tmp("zero-frames.wav");
        write_wav(&zero, 48_000, 2, &[]);
        let info = probe(&zero).expect("a header with no audio is still a header");
        assert_eq!(info.sample_rate, 48_000);
        assert_eq!(info.duration_secs, 0.0);
        let h = open(&zero, 48_000, DEFAULT_DECK_BUDGET_BYTES).unwrap();
        wait_for(&h);
        assert!(h.status.error().is_none(), "{:?}", h.status.error());
        assert_eq!(h.pcm.frames_ready(), 0);
        assert!(h.pcm.is_complete());
        // Reading it anyway is silence, not undefined behaviour.
        assert_eq!(h.pcm.frame_stereo(0), [0.0, 0.0]);
        assert_eq!(h.pcm.frame_stereo(usize::MAX), [0.0, 0.0]);
        assert_eq!(h.duration_secs(), 0.0);
        assert_eq!(h.waveform.data(0).count, 0);
        let _ = std::fs::remove_file(&zero);

        // A single frame, both stored straight and resampled.
        let one = tmp("one-frame.wav");
        write_wav(&one, 48_000, 2, &[0.5, -0.5]);
        for rate in [48_000u32, 44_100] {
            let h = open(&one, rate, DEFAULT_DECK_BUDGET_BYTES).unwrap();
            wait_for(&h);
            assert!(h.status.error().is_none(), "{rate}: {:?}", h.status.error());
            assert!(
                h.pcm.frames_ready() <= 2,
                "{rate}: one input frame became {} output frames",
                h.pcm.frames_ready()
            );
        }
        let _ = std::fs::remove_file(&one);

        // Truncated: the `data` chunk claims 48 000 frames, the file stops after
        // 1 000. Symphonia hits EOF mid-stream; we must keep what we decoded and
        // finish cleanly rather than failing the whole track.
        let cut = tmp("truncated.wav");
        write_wav(&cut, 48_000, 2, &tone(1_000.0, 48_000, 1.0, 2));
        let bytes = std::fs::read(&cut).unwrap();
        std::fs::write(&cut, &bytes[..44 + 1_000 * 2 * 2]).unwrap();
        let h = open(&cut, 48_000, DEFAULT_DECK_BUDGET_BYTES).unwrap();
        wait_for(&h);
        let ready = h.pcm.frames_ready();
        assert!(
            (900..=1_100).contains(&ready),
            "truncated file yielded {ready} frames"
        );
        assert!(h.pcm.is_complete());
        let _ = std::fs::remove_file(&cut);
    }

    /// The channel fold must never index past a short packet, must widen mono
    /// without attenuating it, and must *report* how much silence it had to
    /// invent - that count is what the decode summary line is built from.
    #[test]
    fn fold_interleaved_is_bounds_safe() {
        let mut out = Vec::new();

        // Mono -> stereo duplicates.
        assert_eq!(fold_interleaved(&[0.5, -0.25], 2, 1, 2, &mut out), 0);
        assert_eq!(out, vec![0.5, 0.5, -0.25, -0.25]);

        // 6 channels -> front pair.
        out.clear();
        assert_eq!(
            fold_interleaved(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], 1, 6, 2, &mut out),
            0
        );
        assert_eq!(out, vec![1.0, 2.0]);

        // Same width is a straight copy.
        out.clear();
        assert_eq!(
            fold_interleaved(&[1.0, 2.0, 3.0, 4.0], 2, 2, 2, &mut out),
            0
        );
        assert_eq!(out, vec![1.0, 2.0, 3.0, 4.0]);

        // A packet claiming more frames than it carries is padded, not indexed
        // out of bounds, and the three invented frames are reported.
        out.clear();
        assert_eq!(fold_interleaved(&[1.0, 2.0], 4, 2, 2, &mut out), 3);
        assert_eq!(out, vec![1.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);

        // ... including when the fold has to widen at the same time.
        out.clear();
        assert_eq!(fold_interleaved(&[1.0], 3, 1, 2, &mut out), 2);
        assert_eq!(out, vec![1.0, 1.0, 0.0, 0.0, 0.0, 0.0]);

        // Degenerate channel counts are clamped rather than dividing by zero.
        out.clear();
        assert_eq!(fold_interleaved(&[1.0, 2.0], 2, 0, 0, &mut out), 0);
        assert_eq!(out, vec![1.0, 2.0]);
    }

    #[test]
    fn extension_filter_matches_what_we_advertise() {
        assert!(is_supported_path(Path::new("/x/y.FLAC")));
        assert!(is_supported_path(Path::new("/x/y.wav")));
        assert!(!is_supported_path(Path::new("/x/y.txt")));
        assert!(!is_supported_path(Path::new("/x/y")));
    }

    /// A track that states a channel count nowhere at all is refused, not
    /// guessed at. See `stored_channels`: the guess used to be stereo, which
    /// turned a malformed file into a confident wrong answer in the UI.
    #[test]
    fn a_track_with_no_channel_count_is_refused() {
        assert_eq!(stored_channels(Some(1), None).unwrap(), 1);
        assert_eq!(stored_channels(Some(6), Some(2)).unwrap(), 6);
        // An MP4 states it only in the codec configuration, which is then the
        // one source there is.
        assert_eq!(stored_channels(None, Some(2)).unwrap(), 2);
        assert_eq!(stored_channels(Some(0), Some(1)).unwrap(), 1);
        for (declared, coded) in [(None, None), (Some(0), None), (None, Some(0))] {
            let err = stored_channels(declared, coded).unwrap_err();
            assert!(
                matches!(err, Error::Unsupported(ref m) if m.contains("channels")),
                "{err:?}"
            );
        }
    }

    /// What the *bitstream* states, for the two codecs that state it, read from
    /// the configuration Symphonia hands us verbatim. See [`BitstreamConfig`].
    #[test]
    fn the_bitstream_config_is_read_from_the_codec_configuration() {
        // AAC-LC, frequency index 3 (48 kHz), 2 channels: what ffmpeg wrote
        // into the `esds` of `tests/fixtures/tone-aac.m4a`.
        assert_eq!(
            aac_config(&[0x11, 0x90]),
            BitstreamConfig {
                rate: Some(48_000),
                channels: Some(2)
            }
        );
        // Index 4 is 44.1 kHz; index 8 is 16 kHz, here mono.
        assert_eq!(aac_config(&[0x12, 0x10]).rate, Some(44_100));
        assert_eq!(
            aac_config(&[0x14, 0x08]),
            BitstreamConfig {
                rate: Some(16_000),
                channels: Some(1)
            }
        );
        // Index 15 escapes to an explicit 24-bit rate: object type 2, index 15,
        // 48 000, two channels.
        assert_eq!(
            aac_config(&[0x17, 0x80, 0x5D, 0xC0, 0x10]),
            BitstreamConfig {
                rate: Some(48_000),
                channels: Some(2)
            }
        );
        // Object type 31 escapes to six more bits before the index (here 4,
        // 44.1 kHz).
        assert_eq!(aac_config(&[0xF8, 0xA8, 0x40]).rate, Some(44_100));
        // Channel configuration 7 is 7.1, i.e. eight channels, not seven.
        assert_eq!(aac_config(&[0x11, 0xB8]).channels, Some(8));
        // Nothing to read, a reserved index that is not in the table, a
        // truncated explicit rate, and channel configuration 0 ("see the
        // program config element") all decline rather than invent a number.
        assert_eq!(aac_config(&[]), BitstreamConfig::default());
        assert_eq!(aac_config(&[0x16, 0x90]).rate, None);
        assert_eq!(aac_config(&[0x17, 0x80, 0x00]).rate, None);
        assert_eq!(aac_config(&[0x11, 0x80]).channels, None);

        // ALAC: the magic cookie's channel count at byte 9 and its rate in the
        // last four bytes, big-endian.
        let mut cookie = vec![0u8; 24];
        cookie[9] = 2;
        cookie[20..24].copy_from_slice(&44_100u32.to_be_bytes());
        assert_eq!(
            alac_config(&cookie),
            BitstreamConfig {
                rate: Some(44_100),
                channels: Some(2)
            }
        );
        assert_eq!(alac_config(&cookie[..23]), BitstreamConfig::default());
        assert_eq!(alac_config(&[0u8; 24]), BitstreamConfig::default());

        // Only those two codecs: everything else has no independent evidence to
        // offer, and the container is then believed.
        let mut params = CodecParameters::new();
        params
            .for_codec(CODEC_TYPE_AAC)
            .with_sample_rate(24_000)
            .with_extra_data(vec![0x11, 0x90].into_boxed_slice());
        assert_eq!(BitstreamConfig::of(&params).rate, Some(48_000));
        let mut vorbis = CodecParameters::new();
        vorbis
            .for_codec(symphonia::core::codecs::CODEC_TYPE_VORBIS)
            .with_sample_rate(48_000)
            .with_extra_data(vec![0x11, 0x90].into_boxed_slice());
        assert_eq!(BitstreamConfig::of(&vorbis), BitstreamConfig::default());
        let mut bare = CodecParameters::new();
        bare.for_codec(CODEC_TYPE_AAC).with_sample_rate(48_000);
        assert_eq!(BitstreamConfig::of(&bare), BitstreamConfig::default());
    }

    /// One ISO-BMFF box: `[size][type][body]`.
    fn iso(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        out
    }

    /// A version-0 `mdhd`/`mvhd` body: two timestamps, timescale, duration.
    fn header_box(kind: &[u8; 4], timescale: u32, duration: u32) -> Vec<u8> {
        let mut body = vec![0u8; 12];
        body.extend_from_slice(&timescale.to_be_bytes());
        body.extend_from_slice(&duration.to_be_bytes());
        body.extend_from_slice(&[0u8; 4]);
        iso(kind, &body)
    }

    fn hdlr(handler: &[u8; 4]) -> Vec<u8> {
        let mut body = vec![0u8; 8];
        body.extend_from_slice(handler);
        body.extend_from_slice(&[0u8; 12]);
        iso(b"hdlr", &body)
    }

    /// A version-0 `elst` with `entries` copies of one edit.
    fn elst(entries: u32, segment: u32, media_time: i32) -> Vec<u8> {
        let mut body = vec![0u8; 4];
        body.extend_from_slice(&entries.to_be_bytes());
        for _ in 0..entries {
            body.extend_from_slice(&segment.to_be_bytes());
            body.extend_from_slice(&media_time.to_be_bytes());
            body.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        }
        iso(b"elst", &body)
    }

    fn trak(handler: &[u8; 4], timescale: u32, duration: u32, edit: &[u8]) -> Vec<u8> {
        let mut mdia = hdlr(handler);
        mdia.extend_from_slice(&header_box(b"mdhd", timescale, duration));
        let mut body = iso(b"mdia", &mdia);
        if !edit.is_empty() {
            body.extend_from_slice(&iso(b"edts", edit));
        }
        iso(b"trak", &body)
    }

    /// Write an MP4 whose `moov` holds `traks`, after a `mdat` big enough that a
    /// reader which does not follow box sizes would not find the `moov` at all.
    fn mp4_with(traks: &[Vec<u8>], movie_timescale: u32) -> PathBuf {
        use std::sync::atomic::AtomicUsize;
        static NEXT: AtomicUsize = AtomicUsize::new(0);

        let mut moov = header_box(b"mvhd", movie_timescale, 500);
        for t in traks {
            moov.extend_from_slice(t);
        }
        let mut bytes = iso(b"ftyp", b"isom\0\0\x02\0isom");
        bytes.extend_from_slice(&iso(b"mdat", &vec![0x5Au8; 4_096]));
        bytes.extend_from_slice(&iso(b"moov", &moov));
        let path = std::env::temp_dir().join(format!(
            "onyx-elst-{}-{}.mp4",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&path, &bytes).unwrap();
        path
    }

    /// The AAC priming an MP4 states in its edit list, which the demuxer does
    /// not report. See [`IsoEdit`] and `tests/gapless.rs`.
    #[test]
    fn the_edit_list_of_the_audio_track_is_what_is_read() {
        // The real shape of `tone-aac.m4a`, plus a video track first so the
        // handler check is doing something: 1024 frames of priming at 48 kHz
        // and a half-second segment in a 1000 Hz movie timescale.
        let video = trak(b"vide", 10_240, 5_120, &elst(1, 500, 2_048));
        let audio = trak(b"soun", 48_000, 25_024, &elst(1, 500, 1_024));
        let path = mp4_with(&[video, audio.clone()], 1_000);
        assert_eq!(
            iso_edit(&path, 48_000),
            Some(IsoEdit {
                skip: 1_024,
                length: Some(24_000)
            })
        );
        let _ = std::fs::remove_file(&path);

        // A media timescale that is not the sample rate is rescaled, and so is
        // the segment duration out of the movie timescale.
        let path = mp4_with(
            &[trak(b"soun", 24_000, 12_512, &elst(1, 1_000, 512))],
            2_000,
        );
        assert_eq!(
            iso_edit(&path, 48_000),
            Some(IsoEdit {
                skip: 1_024,
                length: Some(24_000)
            })
        );
        let _ = std::fs::remove_file(&path);

        // Nothing to honour: no edit list at all, an empty edit (negative
        // media time, i.e. silence before the media), and a multi-entry list
        // whose meaning this player does not implement.
        for edit in [Vec::new(), elst(1, 500, -1), elst(2, 250, 1_024)] {
            let path = mp4_with(&[trak(b"soun", 48_000, 25_024, &edit)], 1_000);
            assert_eq!(iso_edit(&path, 48_000), None, "{edit:?}");
            let _ = std::fs::remove_file(&path);
        }

        // A file with no audio track has nothing to say either.
        let path = mp4_with(&[trak(b"vide", 10_240, 5_120, &elst(1, 500, 2_048))], 1_000);
        assert_eq!(iso_edit(&path, 48_000), None);
        let _ = std::fs::remove_file(&path);

        // Truncated boxes are declined, not panicked on: this parser reads a
        // file the user picked, and §9.6 asks for hostile input to be boring.
        let full = mp4_with(&[audio], 1_000);
        let bytes = std::fs::read(&full).unwrap();
        let _ = std::fs::remove_file(&full);
        for cut in [1usize, 9, 32, bytes.len() / 2, bytes.len() - 4] {
            let path = std::env::temp_dir()
                .join(format!("onyx-elst-cut-{}-{cut}.mp4", std::process::id()));
            std::fs::write(&path, &bytes[..cut]).unwrap();
            // The only requirement is that it returns.
            let _ = iso_edit(&path, 48_000);
            let _ = std::fs::remove_file(&path);
        }
    }
}
