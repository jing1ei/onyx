//! MIDI playback through General MIDI (SPEC §18).
//!
//! A `.mid` is not decoded, it is **rendered**: `rustysynth` plays the file
//! through a SoundFont bank and produces stereo f32 at the engine rate. That
//! render then travels the ordinary decoded-audio path in [`crate::decode`],
//! which is the whole point — waveform peaks, seeking, loudness analysis, A/B,
//! EQ and metering all work on it without a single line of MIDI-awareness
//! downstream. There is no second playback path, and if you ever find yourself
//! writing `if is_midi` below [`crate::decode::open`], something has gone
//! wrong.
//!
//! Three things this module is careful about:
//!
//! * **The release tail.** A sequence ends when its last event is dispatched,
//!   not when the sound stops. Rendering exactly `MidiFile::get_length()`
//!   seconds chops the final chord off mid-decay. Rendering continues past the
//!   last event until the output has actually decayed into silence (or
//!   [`TAIL_MAX_SECS`] has passed, so a runaway reverb cannot render forever),
//!   and a tail that has to be cut is faded rather than truncated.
//! * **The bank is not loaded to look at it.** Parsing a SoundFont costs
//!   hundreds of milliseconds and tens of megabytes, so [`probe`] reads the
//!   bank's name out of the RIFF header and leaves the load to [`render`], on
//!   the decode thread. No UI thread ever waits for a `.sf2`.
//! * **Bank identity.** The loudness cache (SPEC §8) is keyed on file identity.
//!   A synthesised render's loudness is a property of the file *and the bank*,
//!   so [`BankInfo::identity`] exists for the app layer to fold into that key.
//!   Without it, switching SoundFont serves a stale measurement.
//! * **Hostile input.** `rustysynth` panics on a MIDI file with no events
//!   (`get_length` indexes an empty vector) and can reject a SoundFont in a
//!   dozen ways. Everything here returns `Result`; nothing panics.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use rustysynth::{MidiFile, MidiFileSequencer, SoundFont, Synthesizer, SynthesizerSettings};

use crate::error::{Error, Result};

/// The General MIDI bank compiled into the binary.
///
/// GeneralUser GS v2.0.3 by S. Christian Collins, 30.8 MiB, redistributable
/// under the GeneralUser GS License v2.0 (see `assets/gm/`, `THIRD-PARTY.md`
/// and the README). It is the standard *compact* GM bank — full 128-patch
/// melodic coverage plus 13 drum kits in ~31 MB, as against the 100 MB-plus
/// orchestral sets — which is why it is here rather than something larger.
const BUNDLED_SF2: &[u8] = include_bytes!("../assets/gm/GeneralUser-GS.sf2");

/// Display name of the bundled bank, shown next to `MIDI · GM` in the UI.
pub const BUNDLED_BANK_NAME: &str = "GeneralUser GS v2.0.3";

/// Cache-key component identifying the bundled bank. Bump the suffix whenever
/// the bundled `.sf2` is replaced, so cached loudness measurements taken with
/// the old bank are not reused with the new one.
const BUNDLED_BANK_ID: &str = "bundled:generaluser-gs-2.0.3";

/// `rustysynth` refuses to build a synthesiser outside this range, so a render
/// at an exotic engine rate is done at the nearest supported rate and the
/// ordinary resampler in [`crate::decode`] carries it the rest of the way.
const MIN_SYNTH_RATE: u32 = 16_000;
const MAX_SYNTH_RATE: u32 = 192_000;

/// Longest release tail rendered after the last MIDI event, in seconds.
///
/// The number is chosen against the bank, not by taste, and the figures below
/// are what its own `pdta` says (`releaseVolEnv`, generator 38, in timecents).
/// The bundled GeneralUser GS has 2487 instrument zones, of which 691 set a
/// release explicitly and the rest keep the SoundFont default of 1 ms. Across
/// those 691 the median is 1.8 s, the 75th percentile 4.1 s, the 90th 15.0 s
/// and the longest 101.6 s. Ten seconds therefore covers 89% of them — 97% of
/// all zones — and what it leaves out are the deliberate outliers: pad and
/// cymbal patches whose envelopes are longer than any note they are ever asked
/// to play. It is [`TAIL_SILENCE`], not this cap, that ends nearly every
/// render: the tail stops the moment a whole block falls below -96 dBFS. What
/// the cap really bounds is memory, at 10 s of stereo f32 per deck (about
/// 3.8 MB at 48 kHz), and time, for a bank that never quite reaches silence.
///
/// When the cap *does* bite, [`render`] fades the last few milliseconds rather
/// than stopping dead; see `TAIL_FADE_SECS`.
///
/// This is the bound [`MidiInfo::max_render_secs`] states, and therefore what
/// the deck is sized from. It is deliberately *not* what a probe reports as the
/// duration — see [`TAIL_TYPICAL_SECS`].
pub const TAIL_MAX_SECS: f64 = 10.0;

/// Release tail a probe *expects*, in seconds, for the duration it reports
/// before anything has been rendered.
///
/// [`TAIL_MAX_SECS`] is a worst case, and reporting a worst case as a duration
/// is its own kind of dishonesty: it would show a two-bar sketch as eleven
/// seconds long in the playlist. This is the middle of the distribution above —
/// the median explicit release in the bundled bank, rounded up — so the figure
/// shown before playback is close to what the render turns out to be, in either
/// direction. As soon as the file is opened, the render's *measured* length
/// replaces it (`DecodeHandle::duration_secs`), so the estimate is never what
/// the transport or the loop bounds are built on.
pub const TAIL_TYPICAL_SECS: f64 = 2.0;

/// Length of the fade applied when a render is cut short (by the tail cap or
/// by the caller's frame budget). Long enough to remove the step, short enough
/// not to be heard as a fade.
const TAIL_FADE_SECS: f64 = 0.010;

/// Peak level below which the tail is considered finished (-96 dBFS, i.e.
/// below the last bit of a 16-bit master).
const TAIL_SILENCE: f32 = 1.6e-5;

/// Frames rendered per block. Also the granularity of the tail decay test.
const RENDER_BLOCK: usize = 1_024;

/// Where the SoundFont in use came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BankSource {
    /// The bank compiled into Onyx.
    Bundled,
    /// A `.sf2` the user pointed at in Settings.
    User(PathBuf),
}

/// The SoundFont a render used, and how to identify it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BankInfo {
    /// Human-readable bank name, for `MIDI · GM · <name>`.
    pub name: String,
    pub source: BankSource,
    /// Stable identity string for cache keys — see [`BankInfo::identity`].
    identity: String,
    /// Set when a user bank was requested but could not be used, so the app
    /// can say *why* it fell back instead of silently sounding different.
    pub fallback_reason: Option<String>,
}

impl BankInfo {
    /// Opaque identity of this bank, for the loudness cache key (SPEC §8).
    ///
    /// The app layer keys the cache on `path|size|mtime`. For a rendered
    /// source that is not enough: the same `.mid` measured through two
    /// different banks has two different loudnesses. Append this — e.g.
    /// `format!("{path}|{size}|{mtime}|{}", info.render_key.unwrap_or_default())`
    /// — and a bank change invalidates the entry instead of serving a stale
    /// measurement. For a user bank it includes the file's size and mtime, so
    /// editing an `.sf2` in place also invalidates.
    pub fn identity(&self) -> &str {
        &self.identity
    }
}

/// What [`probe`] can tell the app about a MIDI file without rendering it.
#[derive(Clone, Debug)]
pub struct MidiInfo {
    /// Length of the sequence itself, in seconds — the last event's time.
    pub sequence_secs: f64,
    /// Upper bound on the rendered length: [`sequence_secs`](Self::sequence_secs)
    /// plus [`TAIL_MAX_SECS`]. The exact length is only known once rendered, and
    /// is always less than or equal to this. Buffers are sized from it.
    pub max_render_secs: f64,
    /// What the render is *expected* to come to:
    /// [`sequence_secs`](Self::sequence_secs) plus [`TAIL_TYPICAL_SECS`]. This
    /// is the figure to show a user before the file has been rendered; see
    /// [`TAIL_TYPICAL_SECS`] for why it is not the bound above.
    pub expected_secs: f64,
    pub bank: BankInfo,
}

/// A finished render, ready to be fed to the ordinary decode pipeline.
pub struct MidiRender {
    /// Interleaved stereo f32.
    pub samples: Vec<f32>,
    /// Rate the render was produced at. Equal to the requested rate unless it
    /// was outside what the synthesiser supports.
    pub rate: u32,
    pub bank: BankInfo,
    /// True when the render hit the frame cap and the tail (or the music) was
    /// cut short.
    pub truncated: bool,
}

impl MidiRender {
    pub fn frames(&self) -> usize {
        self.samples.len() / 2
    }

    pub fn duration_secs(&self) -> f64 {
        self.frames() as f64 / self.rate as f64
    }
}

/// How to render MIDI. Constructed by the app layer from `settings.json`.
#[derive(Clone, Debug, Default)]
pub struct MidiOptions {
    /// A user-supplied `.sf2`. Missing, unreadable or invalid files fall back
    /// to the bundled bank and the reason is reported in
    /// [`BankInfo::fallback_reason`].
    pub soundfont: Option<PathBuf>,
}

// ---------------------------------------------------------------------------
// SoundFont loading
// ---------------------------------------------------------------------------

/// Most-recently-used SoundFont, kept because parsing a 30 MB bank takes long
/// enough to be noticeable on every track change in a folder of MIDI files.
/// One entry: a session realistically uses one bank.
static BANK_CACHE: Mutex<Option<(String, Arc<SoundFont>)>> = Mutex::new(None);

/// Identity of a user `.sf2`: path, size and mtime, matching how the loudness
/// cache identifies audio files.
fn user_bank_identity(path: &Path) -> String {
    let (size, mtime) = std::fs::metadata(path)
        .map(|m| {
            let mtime = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis())
                .unwrap_or(0);
            (m.len(), mtime)
        })
        .unwrap_or((0, 0));
    format!("user:{}|{size}|{mtime}", path.to_string_lossy())
}

/// Bank name from the SoundFont's own `INAM`, falling back to the file stem.
fn bank_name(sf: &SoundFont, path: &Path) -> String {
    let declared = sf.get_info().get_bank_name().trim().to_string();
    if declared.is_empty() {
        path.file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "user SoundFont".to_string())
    } else {
        declared
    }
}

/// Bytes of a `.sf2` read for the header peek. The `INFO` list is the first
/// thing in the file and is a few hundred bytes in practice; a bank that has
/// not named itself within 64 kB is named after its file instead.
const SF2_PEEK_BYTES: usize = 64 * 1024;

/// Read a SoundFont's name out of its RIFF `INFO` list without parsing the
/// bank (SPEC §18).
///
/// [`load_bank`] hands the whole file to `rustysynth`, which builds every
/// sample and region: about 200 ms and 31 MB of resident memory for the
/// bundled bank, and unbounded for whatever the user points at. That is fine
/// on the decode thread and unacceptable on the thread answering a Tauri
/// command, which is where [`probe`] runs. Everything probe needs is in the
/// first few hundred bytes: the name for the badge, and the identity for the
/// loudness cache key, which is only path/size/mtime anyway.
///
/// A file that is not a SoundFont at all is caught here, because the RIFF
/// header says so. A file that is a SoundFont with corrupt sample data is not
/// — that is only discovered when the bank is really loaded, and [`render`]
/// reports it then. Probe states a claim; the render is the truth.
fn peek_bank_name(path: &Path) -> std::result::Result<String, String> {
    use std::io::Read;

    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; SF2_PEEK_BYTES];
    let mut filled = 0usize;
    while filled < buf.len() {
        match file.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    buf.truncate(filled);

    let four = |at: usize| -> Option<[u8; 4]> { buf.get(at..at + 4)?.try_into().ok() };
    let len = |at: usize| -> Option<usize> { Some(u32::from_le_bytes(four(at)?) as usize) };

    if four(0) != Some(*b"RIFF") || four(8) != Some(*b"sfbk") {
        return Err("not a SoundFont (no RIFF/sfbk header)".to_string());
    }

    // Walk the top-level chunks looking for `LIST INFO`, then its `INAM`.
    let mut at = 12usize;
    while let (Some(id), Some(size)) = (four(at), len(at + 4)) {
        let body = at + 8;
        if &id == b"LIST" && four(body) == Some(*b"INFO") {
            let end = body.saturating_add(size).min(buf.len());
            let mut sub = body + 4;
            while sub + 8 <= end {
                let (Some(sub_id), Some(sub_size)) = (four(sub), len(sub + 4)) else {
                    break;
                };
                let sub_body = sub + 8;
                if &sub_id == b"INAM" {
                    let stop = sub_body.saturating_add(sub_size).min(end);
                    let raw = buf.get(sub_body..stop).unwrap_or(&[]);
                    let name = String::from_utf8_lossy(raw)
                        .trim_end_matches('\0')
                        .trim()
                        .to_string();
                    if !name.is_empty() {
                        return Ok(name);
                    }
                    break;
                }
                // RIFF chunks are word aligned.
                sub = sub_body.saturating_add(sub_size + (sub_size & 1));
            }
            break;
        }
        at = body.saturating_add(size + (size & 1));
    }

    Ok(path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "user SoundFont".to_string()))
}

/// Identify the bank [`probe`] would render through, without loading it.
///
/// Same answer as `load_bank(..).1` for every case a probe can tell apart —
/// see [`peek_bank_name`] for the one it cannot.
fn peek_bank(opts: &MidiOptions) -> BankInfo {
    let Some(path) = opts.soundfont.as_ref() else {
        return bundled_info(None);
    };
    let identity = user_bank_identity(path);
    // Already loaded: use the name the real parser read.
    if let Some((id, sf)) = BANK_CACHE.lock().as_ref() {
        if *id == identity {
            return BankInfo {
                name: bank_name(sf, path),
                source: BankSource::User(path.clone()),
                identity,
                fallback_reason: None,
            };
        }
    }
    match peek_bank_name(path) {
        Ok(name) => BankInfo {
            name,
            source: BankSource::User(path.clone()),
            identity,
            fallback_reason: None,
        },
        Err(reason) => bundled_info(Some(fallback_message(path, &reason))),
    }
}

/// The one sentence the app shows when a user bank cannot be used.
fn fallback_message(path: &Path, reason: &str) -> String {
    let name = path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string());
    format!(
        "\"{name}\" is not a usable SoundFont ({reason}); \
         playing through {BUNDLED_BANK_NAME} instead"
    )
}

/// Load the bundled bank (cached).
fn load_bundled() -> Result<(Arc<SoundFont>, BankInfo)> {
    let mut cache = BANK_CACHE.lock();
    if let Some((id, sf)) = cache.as_ref() {
        if id == BUNDLED_BANK_ID {
            return Ok((Arc::clone(sf), bundled_info(None)));
        }
    }
    let mut cursor = Cursor::new(BUNDLED_SF2);
    let sf = Arc::new(SoundFont::new(&mut cursor).map_err(|e| {
        // This is a compiled-in asset: if it fails, the build is broken, not
        // the user's file.
        Error::Other(format!("the bundled General MIDI bank is unreadable: {e}"))
    })?);
    *cache = Some((BUNDLED_BANK_ID.to_string(), Arc::clone(&sf)));
    Ok((sf, bundled_info(None)))
}

fn bundled_info(fallback_reason: Option<String>) -> BankInfo {
    BankInfo {
        name: BUNDLED_BANK_NAME.to_string(),
        source: BankSource::Bundled,
        identity: BUNDLED_BANK_ID.to_string(),
        fallback_reason,
    }
}

/// Resolve [`MidiOptions`] to a usable bank, falling back to the bundled one.
///
/// Never fails because of the *user's* choice — a bad `.sf2` produces the
/// bundled bank plus a [`BankInfo::fallback_reason`] for the app to surface.
pub fn load_bank(opts: &MidiOptions) -> Result<(Arc<SoundFont>, BankInfo)> {
    let Some(path) = opts.soundfont.as_ref() else {
        return load_bundled();
    };

    let identity = user_bank_identity(path);
    {
        let cache = BANK_CACHE.lock();
        if let Some((id, sf)) = cache.as_ref() {
            if *id == identity {
                let sf = Arc::clone(sf);
                let name = bank_name(&sf, path);
                return Ok((
                    sf,
                    BankInfo {
                        name,
                        source: BankSource::User(path.clone()),
                        identity,
                        fallback_reason: None,
                    },
                ));
            }
        }
    }

    let attempt = std::fs::File::open(path)
        .map_err(|e| e.to_string())
        .and_then(|mut f| SoundFont::new(&mut f).map_err(|e| e.to_string()));

    match attempt {
        Ok(sf) => {
            let sf = Arc::new(sf);
            let name = bank_name(&sf, path);
            *BANK_CACHE.lock() = Some((identity.clone(), Arc::clone(&sf)));
            Ok((
                sf,
                BankInfo {
                    name,
                    source: BankSource::User(path.clone()),
                    identity,
                    fallback_reason: None,
                },
            ))
        }
        Err(reason) => {
            log::warn!(
                "SoundFont \"{}\" could not be used ({reason}); using the bundled bank",
                path.display()
            );
            let (sf, _) = load_bundled()?;
            Ok((sf, bundled_info(Some(fallback_message(path, &reason)))))
        }
    }
}

// ---------------------------------------------------------------------------
// Parsing and rendering
// ---------------------------------------------------------------------------

/// Parse a Standard MIDI File.
fn parse_midi(path: &Path) -> Result<MidiFile> {
    let mut file = std::fs::File::open(path)?;
    MidiFile::new(&mut file)
        .map_err(|e| Error::Unsupported(format!("not a MIDI file we can play: {e}")))
}

/// Sequence length in seconds.
///
/// `MidiFile::get_length` is `self.times.last().unwrap()`, which panics on a
/// file with no events at all — a legal, if pointless, `.mid`. Contain it here
/// rather than let it reach a decode thread.
fn sequence_secs(midi: &MidiFile) -> f64 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| midi.get_length()))
        .unwrap_or(0.0)
        .max(0.0)
}

/// The rate a render for engine rate `rate` will actually be produced at.
///
/// Normally the engine rate itself, so a MIDI render is not resampled and is
/// handed to the pipeline at the rate it will be played at. Only an engine
/// rate outside what `rustysynth` supports moves it, and then the ordinary
/// resampler converts the render like any other file.
pub fn synth_rate_for(rate: u32) -> u32 {
    rate.clamp(MIN_SYNTH_RATE, MAX_SYNTH_RATE)
}

/// Read what a MIDI file claims without rendering it.
///
/// Cheap: it parses the (tiny) MIDI and reads the bank's *name* out of its
/// header. It neither loads the SoundFont nor synthesises a single sample, so
/// adding a folder of MIDI files to the playlist stays instant however large
/// the user's `.sf2` is.
pub fn probe(path: &Path, opts: &MidiOptions) -> Result<MidiInfo> {
    let midi = parse_midi(path)?;
    // Peek, do not load: `probe` is reachable from a Tauri command and a user
    // SoundFont is arbitrarily large. See `peek_bank_name`.
    let bank = peek_bank(opts);
    let sequence = sequence_secs(&midi);
    Ok(MidiInfo {
        sequence_secs: sequence,
        max_render_secs: sequence + TAIL_MAX_SECS,
        expected_secs: sequence + TAIL_TYPICAL_SECS,
        bank,
    })
}

/// Render `path` to interleaved stereo f32 at (or near) `rate`.
///
/// `max_frames` caps the output so a hostile or merely enormous file cannot
/// exhaust memory; the caller derives it from the deck budget. The render
/// runs to the end of the sequence and then follows the release tail down to
/// silence, so the last chord is not cut dead.
pub fn render(path: &Path, rate: u32, opts: &MidiOptions, max_frames: usize) -> Result<MidiRender> {
    let midi = Arc::new(parse_midi(path)?);
    let (bank, bank_info) = load_bank(opts)?;

    let synth_rate = synth_rate_for(rate);
    if synth_rate != rate {
        log::debug!(
            "rendering MIDI at {synth_rate} Hz because the engine rate {rate} Hz is outside \
             what the synthesiser supports; the usual resampler will convert it"
        );
    }

    let settings = SynthesizerSettings::new(synth_rate as i32);
    let synth = Synthesizer::new(&bank, &settings)
        .map_err(|e| Error::Other(format!("could not start the MIDI synthesiser: {e}")))?;
    let mut seq = MidiFileSequencer::new(synth);
    seq.play(&midi, false);

    let sequence_frames = (sequence_secs(&midi) * synth_rate as f64).ceil() as usize;
    let tail_cap = (TAIL_MAX_SECS * synth_rate as f64) as usize;
    let hard_cap = sequence_frames.saturating_add(tail_cap).min(max_frames);

    let mut samples: Vec<f32> = Vec::with_capacity((hard_cap * 2).min(max_frames * 2));
    let mut left = vec![0.0f32; RENDER_BLOCK];
    let mut right = vec![0.0f32; RENDER_BLOCK];
    let mut frames_done = 0usize;
    let mut truncated = false;
    let mut decayed = false;
    let mut last_peak = 0.0f32;

    while frames_done < hard_cap {
        let block = RENDER_BLOCK.min(hard_cap - frames_done);
        seq.render(&mut left[..block], &mut right[..block]);

        let mut peak = 0.0f32;
        for i in 0..block {
            let (l, r) = (left[i], right[i]);
            // A NaN out of a synthesiser would poison every meter downstream.
            let l = if l.is_finite() { l } else { 0.0 };
            let r = if r.is_finite() { r } else { 0.0 };
            peak = peak.max(l.abs()).max(r.abs());
            samples.push(l);
            samples.push(r);
        }
        frames_done += block;

        // Past the last event and quiet: the tail has decayed, stop.
        if frames_done >= sequence_frames && peak < TAIL_SILENCE {
            decayed = true;
            break;
        }
        last_peak = peak;
    }
    if frames_done >= max_frames {
        truncated = true;
    }

    // Cut off mid-tail: ease the last few milliseconds down to zero. A render
    // that stops on a non-zero sample is a step, and a step is a click on
    // every playback, at the loop point and again against the other deck.
    // Nothing to do when the tail decayed on its own — it is already silent.
    if !decayed && last_peak >= TAIL_SILENCE {
        let fade = ((TAIL_FADE_SECS * synth_rate as f64) as usize).min(frames_done);
        let start = samples.len() - fade * 2;
        for i in 0..fade {
            // Equal-power would be wrong here: this is a fade to silence, not
            // a crossfade, and linear-in-amplitude is what does not colour it.
            let g = 1.0 - (i as f32 + 1.0) / fade as f32;
            samples[start + i * 2] *= g;
            samples[start + i * 2 + 1] *= g;
        }
    }

    Ok(MidiRender {
        samples,
        rate: synth_rate,
        bank: bank_info,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // -- a tiny Standard MIDI File writer, so the tests need no fixtures -----

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
        // End of track.
        var_len(0, &mut body);
        body.extend_from_slice(&[0xFF, 0x2F, 0x00]);

        let mut out = b"MTrk".to_vec();
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(&body);
        out
    }

    fn smf(format: u16, division: u16, tracks: &[Vec<u8>]) -> Vec<u8> {
        let mut out = b"MThd".to_vec();
        out.extend_from_slice(&6u32.to_be_bytes());
        out.extend_from_slice(&format.to_be_bytes());
        out.extend_from_slice(&(tracks.len() as u16).to_be_bytes());
        out.extend_from_slice(&division.to_be_bytes());
        for t in tracks {
            out.extend_from_slice(t);
        }
        out
    }

    /// Tempo meta-event: `bpm` beats per minute.
    fn tempo(bpm: f64) -> Vec<u8> {
        let us_per_beat = (60_000_000.0 / bpm).round() as u32;
        let b = us_per_beat.to_be_bytes();
        vec![0xFF, 0x51, 0x03, b[1], b[2], b[3]]
    }

    fn tmp(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("onyx-midi-{}-{name}", std::process::id()));
        p
    }

    fn write(path: &Path, bytes: &[u8]) {
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(bytes).unwrap();
        f.flush().unwrap();
    }

    /// Two tracks, 480 ticks per beat, 120 BPM: one beat is 0.5 s.
    /// Track 1 holds a piano note for four beats, track 2 a string note for
    /// two. Total sequence length: 4 beats = 2.0 s.
    fn two_track_midi() -> Vec<u8> {
        let meta = track(&[(0, tempo(120.0))]);
        let piano = track(&[
            (0, vec![0xC0, 0]),           // program 0: piano
            (0, vec![0x90, 60, 100]),     // note on
            (480 * 4, vec![0x80, 60, 0]), // note off after four beats
        ]);
        let strings = track(&[
            (0, vec![0xC1, 48]),
            (0, vec![0x91, 67, 90]),
            (480 * 2, vec![0x81, 67, 0]),
        ]);
        smf(1, 480, &[meta, piano, strings])
    }

    #[test]
    fn the_bundled_bank_is_a_complete_general_midi_set() {
        let (sf, info) = load_bundled().unwrap();
        assert_eq!(info.source, BankSource::Bundled);
        assert_eq!(info.identity(), BUNDLED_BANK_ID);
        assert!(info.fallback_reason.is_none());

        // GM level 1 requires all 128 melodic programs in bank 0 and a
        // percussion kit in bank 128. A bank missing patches would play some
        // files as silence, which is worse than refusing them.
        let melodic: std::collections::BTreeSet<i32> = sf
            .get_presets()
            .iter()
            .filter(|p| p.get_bank_number() == 0)
            .map(|p| p.get_patch_number())
            .collect();
        for program in 0..128 {
            assert!(
                melodic.contains(&program),
                "GM program {program} is missing"
            );
        }
        assert!(
            sf.get_presets()
                .iter()
                .any(|p| p.get_bank_number() == 128 && p.get_patch_number() == 0),
            "the standard drum kit is missing"
        );
    }

    #[test]
    fn renders_a_multi_track_file_and_keeps_the_release_tail() {
        let path = tmp("two-track.mid");
        write(&path, &two_track_midi());

        let info = probe(&path, &MidiOptions::default()).unwrap();
        assert!(
            (info.sequence_secs - 2.0).abs() < 0.05,
            "sequence is {} s, expected 2.0",
            info.sequence_secs
        );

        let out = render(&path, 48_000, &MidiOptions::default(), 48_000 * 60).unwrap();
        assert_eq!(out.rate, 48_000);
        assert!(!out.truncated);

        // Longer than the sequence (the tail is kept) but not unboundedly so.
        let secs = out.duration_secs();
        assert!(
            secs > 2.0,
            "render is {secs} s: the last note was cut off at the sequence end"
        );
        assert!(
            secs <= info.max_render_secs + 0.05,
            "render is {secs} s, longer than the {} s upper bound probe reported",
            info.max_render_secs
        );

        // It must contain audio, and the second half (after the strings stop)
        // must still contain the sustained piano note.
        let frames = out.frames();
        let peak = |from: usize, to: usize| {
            out.samples[from * 2..to * 2]
                .iter()
                .fold(0.0f32, |m, s| m.max(s.abs()))
        };
        assert!(peak(0, frames / 4) > 0.01, "the render is silent");
        assert!(
            peak(frames / 2, frames * 3 / 4) > 0.0005,
            "the sustained note stopped early"
        );
        // ... and it must have actually decayed by the end, otherwise "tail"
        // just means "we rendered three more seconds of music".
        assert!(
            peak(frames - 256, frames) < peak(0, frames / 4),
            "the render never decayed"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// The tempo map has to be honoured, not just the first tempo: the same
    /// note lengths at half the tempo must take twice as long.
    #[test]
    fn tempo_changes_change_the_length() {
        let fast = tmp("fast.mid");
        let slow = tmp("slow.mid");
        let notes = |bpm: f64| {
            let meta = track(&[(0, tempo(bpm))]);
            let notes = track(&[(0, vec![0x90, 60, 100]), (480 * 4, vec![0x80, 60, 0])]);
            smf(1, 480, &[meta, notes])
        };
        write(&fast, &notes(120.0));
        write(&slow, &notes(60.0));

        let f = probe(&fast, &MidiOptions::default()).unwrap().sequence_secs;
        let s = probe(&slow, &MidiOptions::default()).unwrap().sequence_secs;
        assert!((f - 2.0).abs() < 0.05, "{f}");
        assert!((s - 4.0).abs() < 0.05, "{s}");

        // And a tempo change *inside* the file: two beats at 120 then two at
        // 60 is 1.0 + 2.0 = 3.0 s, not 2.0 and not 4.0.
        let mixed = tmp("mixed.mid");
        let meta = track(&[(0, tempo(120.0)), (480 * 2, tempo(60.0))]);
        let notes = track(&[(0, vec![0x90, 60, 100]), (480 * 4, vec![0x80, 60, 0])]);
        write(&mixed, &smf(1, 480, &[meta, notes]));
        let m = probe(&mixed, &MidiOptions::default())
            .unwrap()
            .sequence_secs;
        assert!((m - 3.0).abs() < 0.05, "tempo map ignored: {m} s");

        for p in [fast, slow, mixed] {
            let _ = std::fs::remove_file(&p);
        }
    }

    #[test]
    fn a_bad_soundfont_falls_back_to_the_bundled_bank_with_a_reason() {
        let junk = tmp("not-a-bank.sf2");
        write(&junk, b"this is not a SoundFont");
        let opts = MidiOptions {
            soundfont: Some(junk.clone()),
        };
        let (_, info) = load_bank(&opts).unwrap();
        assert_eq!(info.source, BankSource::Bundled);
        assert!(
            info.fallback_reason
                .as_deref()
                .unwrap_or_default()
                .contains("not-a-bank.sf2"),
            "{:?}",
            info.fallback_reason
        );

        // A path that does not exist at all behaves the same way.
        let missing = tmp("nope.sf2");
        let _ = std::fs::remove_file(&missing);
        let (_, info) = load_bank(&MidiOptions {
            soundfont: Some(missing),
        })
        .unwrap();
        assert_eq!(info.source, BankSource::Bundled);
        assert!(info.fallback_reason.is_some());

        // Rendering still works through the fallback.
        let mid = tmp("fallback.mid");
        write(&mid, &two_track_midi());
        let out = render(&mid, 44_100, &opts, 44_100 * 30).unwrap();
        assert!(out.frames() > 0);
        assert_eq!(out.bank.source, BankSource::Bundled);

        let _ = std::fs::remove_file(&junk);
        let _ = std::fs::remove_file(&mid);
    }

    /// The bank identity is what keeps the loudness cache honest across a bank
    /// change (SPEC §18). Two different banks must not share a key.
    #[test]
    fn bank_identity_distinguishes_banks() {
        let bundled = load_bundled().unwrap().1;
        let user = tmp("identity.sf2");
        write(&user, BUNDLED_SF2);
        let (_, info) = load_bank(&MidiOptions {
            soundfont: Some(user.clone()),
        })
        .unwrap();
        assert_eq!(info.source, BankSource::User(user.clone()));
        assert_ne!(
            info.identity(),
            bundled.identity(),
            "the same file loaded as a user bank must not reuse the bundled key"
        );
        assert!(info.identity().contains(&format!("{}", BUNDLED_SF2.len())));
        let _ = std::fs::remove_file(&user);
    }

    #[test]
    fn degenerate_midi_files_are_errors_or_silence_but_never_panics() {
        // Not MIDI at all.
        let junk = tmp("junk.mid");
        write(&junk, b"MThd but not really");
        assert!(probe(&junk, &MidiOptions::default()).is_err());
        assert!(render(&junk, 48_000, &MidiOptions::default(), 48_000).is_err());

        // Empty file.
        let empty = tmp("empty.mid");
        write(&empty, b"");
        assert!(probe(&empty, &MidiOptions::default()).is_err());

        // Format 2 is not something rustysynth plays; it must say so rather
        // than render nonsense.
        let fmt2 = tmp("format2.mid");
        write(&fmt2, &smf(2, 480, &[track(&[(0, tempo(120.0))])]));
        assert!(probe(&fmt2, &MidiOptions::default()).is_err());

        // A valid header with a single empty track: zero-length, not a panic.
        // (`MidiFile::get_length` indexes an empty vector here.)
        let bare = tmp("bare.mid");
        write(&bare, &smf(0, 480, &[track(&[])]));
        if let Ok(info) = probe(&bare, &MidiOptions::default()) {
            assert!(info.sequence_secs >= 0.0);
            let out = render(&bare, 48_000, &MidiOptions::default(), 48_000 * 10).unwrap();
            assert!(out.duration_secs() <= TAIL_MAX_SECS + 0.05);
        }

        for p in [junk, empty, fmt2, bare] {
            let _ = std::fs::remove_file(&p);
        }
    }

    /// The frame cap is the only thing standing between a pathological file
    /// and the whole deck budget, so it has to actually bite.
    #[test]
    fn the_frame_cap_truncates_instead_of_exhausting_memory() {
        let path = tmp("capped.mid");
        write(&path, &two_track_midi());
        let out = render(&path, 48_000, &MidiOptions::default(), 4_800).unwrap();
        assert_eq!(out.frames(), 4_800);
        assert!(out.truncated);
        let _ = std::fs::remove_file(&path);
    }

    /// A render cut short mid-note must not end on a step. The frame cap is the
    /// easiest way to force the same path the [`TAIL_MAX_SECS`] cap takes.
    #[test]
    fn a_truncated_render_is_faded_not_chopped() {
        let path = tmp("chopped.mid");
        write(&path, &two_track_midi());
        let out = render(&path, 48_000, &MidiOptions::default(), 24_000).unwrap();
        assert!(out.truncated);
        let frames = out.frames();
        assert_eq!(frames, 24_000);

        // Loud before the fade, silent at the very end.
        let peak = |from: usize, to: usize| {
            out.samples[from * 2..to * 2]
                .iter()
                .fold(0.0f32, |m, s| m.max(s.abs()))
        };
        let fade = (TAIL_FADE_SECS * 48_000.0) as usize;
        assert!(
            peak(frames - 2 * fade, frames - fade) > 0.001,
            "nothing was playing, so there was nothing to fade"
        );
        assert!(
            out.samples[frames * 2 - 2].abs() < 1e-6 && out.samples[frames * 2 - 1].abs() < 1e-6,
            "the render ends on a step: {:?}",
            &out.samples[frames * 2 - 2..]
        );
        // Monotone in the large: the last tenth of the fade is quieter than the
        // first tenth of it, whatever the waveform is doing inside a cycle.
        let head = peak(frames - fade, frames - fade * 9 / 10);
        let tail = peak(frames - fade / 10, frames);
        assert!(tail < head, "fade did not descend: {head} then {tail}");
        let _ = std::fs::remove_file(&path);
    }

    /// SPEC §18: probing a MIDI file must not load the SoundFont. The proof is
    /// a file the *peek* accepts and the real parser does not — probe reports
    /// the name from its header, and only the render discovers it is rubbish
    /// and falls back.
    #[test]
    fn probe_reads_the_bank_header_without_loading_the_bank() {
        // A well-formed RIFF/sfbk INFO list, then nothing a synthesiser can use.
        let mut sf2: Vec<u8> = Vec::new();
        let name = b"Pretend Orchestra\0\0";
        let mut info: Vec<u8> = Vec::new();
        info.extend_from_slice(b"INFO");
        info.extend_from_slice(b"ifil");
        info.extend_from_slice(&4u32.to_le_bytes());
        info.extend_from_slice(&[2, 0, 1, 0]);
        info.extend_from_slice(b"INAM");
        info.extend_from_slice(&(name.len() as u32).to_le_bytes());
        info.extend_from_slice(name);
        sf2.extend_from_slice(b"RIFF");
        sf2.extend_from_slice(&((4 + 8 + info.len()) as u32).to_le_bytes());
        sf2.extend_from_slice(b"sfbk");
        sf2.extend_from_slice(b"LIST");
        sf2.extend_from_slice(&(info.len() as u32).to_le_bytes());
        sf2.extend_from_slice(&info);

        let bank_path = tmp("headers-only.sf2");
        write(&bank_path, &sf2);
        let opts = MidiOptions {
            soundfont: Some(bank_path.clone()),
        };
        let mid = tmp("peek.mid");
        write(&mid, &two_track_midi());

        let info = probe(&mid, &opts).unwrap();
        assert_eq!(info.bank.name, "Pretend Orchestra");
        assert_eq!(info.bank.source, BankSource::User(bank_path.clone()));
        assert!(
            info.bank.identity().contains(&format!("{}", sf2.len())),
            "identity {} should key on the file itself",
            info.bank.identity()
        );

        // The render is where the truth comes out.
        let out = render(&mid, 48_000, &opts, 48_000).unwrap();
        assert_eq!(out.bank.source, BankSource::Bundled);
        assert!(out.bank.fallback_reason.is_some());

        // A file that is not a SoundFont at all is caught by the peek, so probe
        // reports the fallback without reading past the header either.
        let junk = tmp("peek-junk.sf2");
        write(&junk, b"not a SoundFont");
        let info = probe(
            &mid,
            &MidiOptions {
                soundfont: Some(junk.clone()),
            },
        )
        .unwrap();
        assert_eq!(info.bank.source, BankSource::Bundled);
        assert!(info
            .bank
            .fallback_reason
            .as_deref()
            .unwrap_or_default()
            .contains("peek-junk.sf2"));

        for p in [bank_path, mid, junk] {
            let _ = std::fs::remove_file(p);
        }
    }
}
