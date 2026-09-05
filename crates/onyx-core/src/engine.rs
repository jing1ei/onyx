//! The playback engine: CPAL output stream, two position-locked decks,
//! transport, A/B switching, EQ and the meter tap.
//!
//! # Thread map
//!
//! ```text
//!  UI thread ──invoke──► AudioEngine ──lock-free queue──► output callback
//!                              │                              │
//!                              ├──HostCmd──► host thread      └─ring─► analysis thread
//!                              │            (owns cpal::Stream)          (MeterBank)
//!                              └──reads──── RtShared atomics / meter snapshot
//! ```
//!
//! The output callback never allocates, never blocks and never waits on the UI.
//! The only lock it touches is a `try_lock` on the core (contended solely while
//! the device is being rebuilt), and it degrades to silence rather than stalling.
//!
//! # Diagnostics
//!
//! Nothing on the real-time path may call `log::` — the macros format, allocate
//! and take a lock inside the logger. Faults that can only be noticed there
//! (cpal handing us a stream error, a command queue that overflowed) are
//! *counted* in [`RtShared`] and drained by the host through
//! [`AudioEngine::take_faults`], which is where they turn into log lines.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SampleRate, StreamConfig, SupportedBufferSize};
use crossbeam_channel::{bounded, Sender};
use crossbeam_queue::ArrayQueue;
use parking_lot::{Mutex, RwLock};

use crate::dsp::eq::{AuditionFilter, EqSetting, StereoEq};
use crate::dsp::meters::MeterBank;
use crate::error::{Error, Result};
use crate::pcm::SharedPcm;
use crate::types::{
    latency_ms, BufferRange, Deck, DeviceInfo, EngineSource, EqConfig, HostInfo, MeterSnapshot,
    MonitorMode, MONITOR_IDENTITY,
};

/// Largest block we process in one go (keeps scratch buffers bounded).
const MAX_BLOCK: usize = 4_096;
/// Play/pause and seek de-click ramp.
const ENV_RAMP_SECS: f32 = 0.005;
/// Volume glide.
const VOLUME_RAMP_SECS: f32 = 0.02;
/// Per-deck level-match trim glide. Long enough to be inaudible on a gain
/// change, short enough that an A/B switch is still "instant".
const TRIM_RAMP_SECS: f32 = 0.04;
/// Default A/B crossfade.
const DEFAULT_CROSSFADE_SECS: f32 = 0.008;
/// Monitor-matrix crossfade. Long enough to hide the discontinuity of a fold
/// change, short enough that the switch still feels instant.
const MONITOR_RAMP_SECS: f32 = 0.005;
/// Fallback rate when the device advertises nothing we recognise.
pub const FALLBACK_RATE: u32 = 48_000;
/// Largest A/B alignment offset, in seconds either way (SPEC §11).
pub const MAX_AB_OFFSET_SECS: f64 = 30.0;

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

/// What cpal's error callback told us, coded so it can live in an atomic.
const FAULT_NONE: u8 = 0;
const FAULT_DEVICE_UNAVAILABLE: u8 = 1;
const FAULT_BACKEND: u8 = 2;

/// Why the output stream complained.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamFault {
    /// The device went away (unplugged interface, session stolen).
    DeviceUnavailable,
    /// Anything the backend reported that is not a lost device.
    Backend,
}

impl StreamFault {
    fn from_code(code: u8) -> Option<StreamFault> {
        match code {
            FAULT_DEVICE_UNAVAILABLE => Some(StreamFault::DeviceUnavailable),
            FAULT_BACKEND => Some(StreamFault::Backend),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            StreamFault::DeviceUnavailable => "the output device is no longer available",
            StreamFault::Backend => "the audio backend reported an error",
        }
    }
}

/// Faults noticed where logging is forbidden, coalesced into counts.
///
/// Drained by the host (the 60 Hz frame thread) with
/// [`AudioEngine::take_faults`]: one log line per drain however many times the
/// fault fired, so a device that errors on every callback cannot write
/// thousands of identical lines a second.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RtFaults {
    /// Commands that never reached the callback because the queue was full.
    pub dropped_commands: u32,
    /// Calls into cpal's error callback.
    pub stream_faults: u32,
    /// The most recent stream fault, if there was one.
    pub last_stream_fault: Option<StreamFault>,
}

impl RtFaults {
    pub fn is_empty(&self) -> bool {
        self.dropped_commands == 0 && self.stream_faults == 0
    }
}

/// Atomics shared between the callback and everyone else.
pub struct RtShared {
    playing: AtomicBool,
    pos_frames: AtomicU64,
    engine_rate: AtomicU32,
    underruns: AtomicU32,
    ended: AtomicBool,
    buffering: AtomicBool,
    active_deck: AtomicU8,
    ab_enabled: AtomicBool,
    /// See [`RtFaults`]. Written from the real-time path, drained by the host.
    dropped_commands: AtomicU32,
    stream_faults: AtomicU32,
    last_stream_fault: AtomicU8,
}

impl RtShared {
    fn new(rate: u32) -> Self {
        RtShared {
            playing: AtomicBool::new(false),
            pos_frames: AtomicU64::new(0),
            engine_rate: AtomicU32::new(rate),
            underruns: AtomicU32::new(0),
            ended: AtomicBool::new(false),
            buffering: AtomicBool::new(false),
            active_deck: AtomicU8::new(0),
            ab_enabled: AtomicBool::new(false),
            dropped_commands: AtomicU32::new(0),
            stream_faults: AtomicU32::new(0),
            last_stream_fault: AtomicU8::new(FAULT_NONE),
        }
    }

    /// Take everything that has gone wrong since the last call.
    pub fn take_faults(&self) -> RtFaults {
        let stream_faults = self.stream_faults.swap(0, Ordering::AcqRel);
        let code = if stream_faults > 0 {
            self.last_stream_fault.swap(FAULT_NONE, Ordering::AcqRel)
        } else {
            FAULT_NONE
        };
        RtFaults {
            dropped_commands: self.dropped_commands.swap(0, Ordering::AcqRel),
            stream_faults,
            last_stream_fault: StreamFault::from_code(code),
        }
    }

    pub fn engine_rate(&self) -> u32 {
        self.engine_rate.load(Ordering::Relaxed).max(1)
    }

    pub fn position_frames(&self) -> u64 {
        self.pos_frames.load(Ordering::Relaxed)
    }

    pub fn position_secs(&self) -> f64 {
        self.position_frames() as f64 / self.engine_rate() as f64
    }

    pub fn is_playing(&self) -> bool {
        self.playing.load(Ordering::Relaxed)
    }

    pub fn is_buffering(&self) -> bool {
        self.buffering.load(Ordering::Relaxed)
    }

    pub fn underruns(&self) -> u32 {
        self.underruns.load(Ordering::Relaxed)
    }

    pub fn active_deck(&self) -> Deck {
        Deck::from_index(self.active_deck.load(Ordering::Relaxed) as usize)
    }

    /// Consume the "reached the end of the track" flag.
    pub fn take_ended(&self) -> bool {
        self.ended.swap(false, Ordering::AcqRel)
    }
}

/// Control flags for the analysis thread.
struct MeterCtl {
    reset: AtomicBool,
    reset_transient: AtomicBool,
    rate: AtomicU32,
    running: AtomicBool,
    /// A closed analyser panel costs no FFT at all (SPEC §12).
    spectrum: AtomicBool,
}

/// Commands delivered to the callback through a lock-free queue.
///
/// `SetEq` carries a whole configuration inline (~340 bytes) and makes this
/// enum large. That is deliberate: boxing it would mean the *audio thread*
/// frees the box, and a `free()` in the callback is exactly what we refuse to
/// do. The queue is 512 entries, so the cost is a few hundred kB of RAM.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
enum RtCmd {
    LoadDeck {
        deck: usize,
        pcm: Arc<SharedPcm>,
        trim: f32,
    },
    ClearDeck(usize),
    SetTrim {
        deck: usize,
        trim: f32,
    },
    Play,
    Pause,
    Stop,
    Seek(u64),
    SetVolume(f32),
    SetMute(bool),
    SetLoop(bool),
    SetLoopRegion(Option<(u64, u64)>),
    SelectDeck(usize),
    SetCrossfadeFrames(u32),
    /// Whole EQ configuration in one fixed-size, `Copy` message.
    SetEq(EqSetting),
    SetAudition {
        freq_hz: Option<f32>,
        q: f32,
    },
    SetMonitor(MonitorMode),
    SetAbOffset(i64),
    SetInvert {
        deck: usize,
        invert: bool,
    },
}

enum HostCmd {
    Rebuild {
        host: Option<String>,
        device: Option<String>,
        rate: u32,
        buffer: Option<u32>,
        reply: Sender<Result<StreamOutcome>>,
    },
    Shutdown,
}

/// What a stream rebuild actually produced, as opposed to what was asked for.
///
/// The two differ whenever a device cannot do the requested rate, is missing
/// (so the system default was used instead), or ignores the buffer size —
/// which is exactly the information the UI has to show.
#[derive(Clone, Debug, PartialEq, Eq)]
struct StreamOutcome {
    host_id: String,
    device_name: Option<String>,
    /// True when the device was reached through the OS default rather than by
    /// name, either because that is what was asked for or because the named
    /// device was not there.
    following_system_default: bool,
    rate: u32,
    /// The size we asked cpal for, when the backend accepts one at all.
    buffer_frames: Option<u32>,
}

// ---------------------------------------------------------------------------
// Real-time core
// ---------------------------------------------------------------------------

struct RtDeck {
    pcm: Arc<SharedPcm>,
    /// Level-match trim as a positive linear gain.
    trim: f32,
    /// -1.0 when the deck's polarity is inverted, +1.0 otherwise.
    polarity: f32,
    /// Smoothed `trim * polarity`.
    gain: f32,
    gain_target: f32,
}

struct RtCore {
    rate: f32,
    decks: [Option<RtDeck>; 2],
    /// 0.0 = deck A audible, 1.0 = deck B audible.
    fade: f32,
    fade_target: f32,
    fade_step: f32,
    /// Crossfade time as *seconds*, so a device rate change can re-derive
    /// `fade_step` instead of silently resetting the user's setting.
    crossfade_secs: f32,
    env: f32,
    env_target: f32,
    env_step: f32,
    volume: f32,
    volume_target: f32,
    volume_step: f32,
    /// Per-sample step for the per-deck trim glide (rate-scaled, like the
    /// others - a fixed step would glide twice as slowly at 96 kHz).
    trim_step: f32,
    muted: bool,
    loop_enabled: bool,
    loop_region: Option<(u64, u64)>,
    pos: u64,
    /// Signed deck-B read offset in frames (SPEC §11). Deck B reads at
    /// `pos + ab_offset`; reads outside its decoded region produce silence.
    ab_offset: i64,
    eq: StereoEq,
    /// Band-solo filter, after the EQ and after the meter tap.
    audition: AuditionFilter,
    /// Monitoring fold. `monitor` is what the user asked for; `monitor_cur` is
    /// the matrix actually on the bus, gliding towards `monitor_target` over
    /// [`MONITOR_RAMP_SECS`] so a mode change cannot click.
    monitor: MonitorMode,
    monitor_cur: [f32; 4],
    monitor_target: [f32; 4],
    monitor_step: [f32; 4],
    mix: Vec<f32>,
    shared: Arc<RtShared>,
    cmds: Arc<ArrayQueue<RtCmd>>,
    garbage: Arc<ArrayQueue<Arc<SharedPcm>>>,
    meter_tx: rtrb::Producer<f32>,
}

impl RtCore {
    fn new(
        rate: u32,
        shared: Arc<RtShared>,
        cmds: Arc<ArrayQueue<RtCmd>>,
        garbage: Arc<ArrayQueue<Arc<SharedPcm>>>,
        meter_tx: rtrb::Producer<f32>,
    ) -> Self {
        let rate_f = rate as f32;
        RtCore {
            rate: rate_f,
            decks: [None, None],
            fade: 0.0,
            fade_target: 0.0,
            fade_step: 1.0 / (DEFAULT_CROSSFADE_SECS * rate_f),
            crossfade_secs: DEFAULT_CROSSFADE_SECS,
            env: 0.0,
            env_target: 0.0,
            env_step: 1.0 / (ENV_RAMP_SECS * rate_f),
            volume: 1.0,
            volume_target: 1.0,
            volume_step: 1.0 / (VOLUME_RAMP_SECS * rate_f),
            trim_step: 1.0 / (TRIM_RAMP_SECS * rate_f),
            muted: false,
            loop_enabled: false,
            loop_region: None,
            pos: 0,
            ab_offset: 0,
            eq: StereoEq::new(rate as f64),
            audition: AuditionFilter::new(rate as f64),
            monitor: MonitorMode::Stereo,
            monitor_cur: MONITOR_IDENTITY,
            monitor_target: MONITOR_IDENTITY,
            monitor_step: [0.0; 4],
            mix: vec![0.0; MAX_BLOCK * 2],
            shared,
            cmds,
            garbage,
            meter_tx,
        }
    }

    fn set_rate(&mut self, rate: u32) {
        let rate = rate.max(1);
        let old = self.rate.max(1.0) as f64;
        let new = rate as f64;
        self.rate = rate as f32;

        // Everything held in *frames* has to be re-expressed at the new rate.
        // Without this, following the source rate (or switching to a device
        // that cannot do the current one) silently teleports the playhead:
        // 44.1 -> 48 kHz moves it 8.8% and drags the loop bounds with it.
        if (new - old).abs() > f64::EPSILON {
            let ratio = new / old;
            self.pos = (self.pos as f64 * ratio).round() as u64;
            self.loop_region = self.loop_region.map(|(a, b)| {
                (
                    (a as f64 * ratio).round() as u64,
                    (b as f64 * ratio).round() as u64,
                )
            });
            self.ab_offset = (self.ab_offset as f64 * ratio).round() as i64;
            self.shared.pos_frames.store(self.pos, Ordering::Relaxed);
        }

        // Re-derive every smoother from its *time* constant. Following the
        // source sample rate re-opens the device, and that must not quietly
        // change the crossfade length or the de-click ramp.
        self.fade_step = crossfade_step(self.crossfade_secs, self.rate);
        self.env_step = 1.0 / (ENV_RAMP_SECS * self.rate);
        self.volume_step = 1.0 / (VOLUME_RAMP_SECS * self.rate);
        self.trim_step = 1.0 / (TRIM_RAMP_SECS * self.rate);
        self.retarget_monitor();
        self.eq.set_sample_rate(rate as f64);
        self.audition.set_sample_rate(rate as f64);
        self.shared.engine_rate.store(rate, Ordering::Relaxed);
    }

    #[inline]
    fn recycle(&self, pcm: Arc<SharedPcm>) {
        // Never drop an `Arc<SharedPcm>` in the callback: if we hold the last
        // reference, `Drop` frees a multi-megabyte allocation, which can take
        // a lock inside the allocator and blow the deadline.
        //
        // `push` gives the Arc *back* when the queue is full, and `let _ =`
        // would then drop it right here - exactly what we are trying to avoid.
        // The queue is 256 deep and the host thread drains it at least every
        // 100 ms, so overflow means something pathological is happening; in
        // that case we deliberately leak rather than break real-time safety.
        // The leak is bounded by however many buffers were in flight and the
        // host thread reclaims everything else as usual.
        if let Err(returned) = self.garbage.push(pcm) {
            std::mem::forget(returned);
        }
    }

    fn drain_commands(&mut self) {
        while let Some(cmd) = self.cmds.pop() {
            match cmd {
                RtCmd::LoadDeck { deck, pcm, trim } => {
                    let trim = sane_gain(trim);
                    let polarity = self.decks[deck].as_ref().map_or(1.0, |d| d.polarity);
                    if let Some(old) = self.decks[deck].take() {
                        self.recycle(old.pcm);
                    }
                    self.decks[deck] = Some(RtDeck {
                        pcm,
                        trim,
                        polarity,
                        gain: trim * polarity,
                        gain_target: trim * polarity,
                    });
                }
                RtCmd::ClearDeck(deck) => {
                    if let Some(old) = self.decks[deck].take() {
                        self.recycle(old.pcm);
                    }
                }
                RtCmd::SetTrim { deck, trim } => {
                    if let Some(d) = self.decks[deck].as_mut() {
                        d.trim = sane_gain(trim);
                        d.gain_target = d.trim * d.polarity;
                    }
                }
                RtCmd::SetInvert { deck, invert } => {
                    if let Some(d) = self.decks[deck].as_mut() {
                        d.polarity = if invert { -1.0 } else { 1.0 };
                        // Glide through zero rather than flipping the sign of
                        // a live signal, which would be a full-scale step.
                        d.gain_target = d.trim * d.polarity;
                    }
                }
                RtCmd::Play => {
                    self.env_target = 1.0;
                    self.shared.playing.store(true, Ordering::Relaxed);
                }
                RtCmd::Pause => {
                    self.env_target = 0.0;
                    self.shared.playing.store(false, Ordering::Relaxed);
                }
                RtCmd::Stop => {
                    self.env_target = 0.0;
                    self.env = 0.0;
                    // Stop means "back to the start". Only an *enabled* loop
                    // region redefines where the start is.
                    self.pos = if self.loop_enabled {
                        self.loop_region.map(|(s, _)| s).unwrap_or(0)
                    } else {
                        0
                    };
                    self.shared.playing.store(false, Ordering::Relaxed);
                    self.shared.pos_frames.store(self.pos, Ordering::Relaxed);
                }
                RtCmd::Seek(frames) => {
                    self.pos = frames;
                    // De-click: dip and come back up.
                    self.env = 0.0;
                    self.shared.pos_frames.store(self.pos, Ordering::Relaxed);
                }
                RtCmd::SetVolume(v) => self.volume_target = sane_gain(v).min(4.0),
                RtCmd::SetMute(m) => self.muted = m,
                RtCmd::SetLoop(l) => self.loop_enabled = l,
                RtCmd::SetLoopRegion(r) => {
                    // A degenerate region would freeze the playhead: wrapping
                    // happens on `pos >= end`, so `end <= start` never advances.
                    self.loop_region = r.filter(|(s, e)| e > s);
                }
                RtCmd::SelectDeck(d) => {
                    self.fade_target = if d == 0 { 0.0 } else { 1.0 };
                    self.shared.active_deck.store(d as u8, Ordering::Relaxed);
                }
                RtCmd::SetCrossfadeFrames(f) => {
                    // Remember the duration, not just the step, so `set_rate`
                    // can rebuild it after a device change.
                    self.crossfade_secs = f as f32 / self.rate;
                    self.fade_step = if f == 0 { 1.0 } else { 1.0 / f as f32 };
                }
                RtCmd::SetEq(setting) => self.eq.apply(&setting),
                RtCmd::SetAudition { freq_hz, q } => self.audition.set(freq_hz, q),
                RtCmd::SetMonitor(mode) => {
                    if mode != self.monitor {
                        self.monitor = mode;
                        self.monitor_target = mode.matrix();
                        // Glide from wherever the bus actually is, which may be
                        // half way through a previous change.
                        self.retarget_monitor();
                    }
                }
                RtCmd::SetAbOffset(frames) => self.ab_offset = frames,
            }
        }
    }

    /// Fill `out` (interleaved, `channels` wide) with the next block.
    ///
    /// Signal order is deliberate:
    /// `decks -> trim -> crossfade -> volume/env -> EQ -> [meter tap] ->
    /// audition -> monitor fold -> device`.
    /// The meters are tapped *before* the audition filter and the monitor
    /// matrix so that LUFS, true peak and correlation keep describing the
    /// programme rather than the fold the engineer happens to be listening
    /// through (SPEC §6).
    fn process(&mut self, out: &mut [f32], channels: usize) {
        self.drain_commands();
        let channels = channels.max(1);
        let total_frames = out.len() / channels;
        let mut done = 0usize;
        while done < total_frames {
            let n = MAX_BLOCK.min(total_frames - done);
            self.render(n);
            self.tap_meters(n);
            self.apply_monitor_chain(n);
            let mix = &self.mix[..n * 2];
            let dst = &mut out[done * channels..(done + n) * channels];
            match channels {
                1 => {
                    for f in 0..n {
                        dst[f] = 0.5 * (mix[f * 2] + mix[f * 2 + 1]);
                    }
                }
                2 => dst.copy_from_slice(mix),
                _ => {
                    for f in 0..n {
                        let base = f * channels;
                        dst[base] = mix[f * 2];
                        dst[base + 1] = mix[f * 2 + 1];
                        for c in 2..channels {
                            dst[base + c] = 0.0;
                        }
                    }
                }
            }
            done += n;
        }
        self.shared.pos_frames.store(self.pos, Ordering::Relaxed);
    }

    /// Hand the programme signal to the meter bridge. If the analysis thread
    /// is behind we simply drop the block: metering must never stall playback.
    fn tap_meters(&mut self, n: usize) {
        let mix = &self.mix[..n * 2];
        for s in mix.iter() {
            if self.meter_tx.push(*s).is_err() {
                break;
            }
        }
    }

    /// Audition (band solo) followed by the monitor matrix.
    ///
    /// `Stereo` with no crossfade in flight returns without touching a single
    /// sample - that is what keeps the default path bit-transparent.
    fn apply_monitor_chain(&mut self, n: usize) {
        {
            let (mix, audition) = (&mut self.mix[..n * 2], &mut self.audition);
            audition.process(mix);
        }

        // Early-out, not a multiply by the identity (SPEC §6). The check is
        // on the *live* matrix as well as the target, so the last block of a
        // fade back to `Stereo` is processed and every block after it is not.
        if self.monitor_cur == MONITOR_IDENTITY && self.monitor_target == MONITOR_IDENTITY {
            return;
        }

        let mut cur = self.monitor_cur;
        let step = self.monitor_step;
        let target = self.monitor_target;
        let mix = &mut self.mix[..n * 2];
        for f in 0..n {
            let l = mix[f * 2];
            let r = mix[f * 2 + 1];
            mix[f * 2] = cur[0] * l + cur[1] * r;
            mix[f * 2 + 1] = cur[2] * l + cur[3] * r;
            for c in 0..4 {
                // Monotone approach plus a clamp: `cur` lands *exactly* on the
                // target, so a settled `Stereo` really is the identity and the
                // early-out above can fire.
                if step[c] > 0.0 {
                    cur[c] = (cur[c] + step[c]).min(target[c]);
                } else if step[c] < 0.0 {
                    cur[c] = (cur[c] + step[c]).max(target[c]);
                } else {
                    cur[c] = target[c];
                }
            }
        }
        self.monitor_cur = cur;
    }

    /// Re-derive the per-coefficient glide so the current matrix reaches the
    /// target in [`MONITOR_RAMP_SECS`], whatever the rate and wherever the
    /// glide happens to be right now.
    fn retarget_monitor(&mut self) {
        let frames = (MONITOR_RAMP_SECS * self.rate).max(1.0);
        for c in 0..4 {
            self.monitor_step[c] = (self.monitor_target[c] - self.monitor_cur[c]) / frames;
        }
    }

    /// Render `n` frames into `self.mix`.
    fn render(&mut self, n: usize) {
        let mix = &mut self.mix[..n * 2];
        mix.iter_mut().for_each(|s| *s = 0.0);

        // Snapshot how far each deck has been decoded (one atomic load each).
        let ready = [
            self.decks[0]
                .as_ref()
                .map(|d| d.pcm.frames_ready())
                .unwrap_or(0),
            self.decks[1]
                .as_ref()
                .map(|d| d.pcm.frames_ready())
                .unwrap_or(0),
        ];
        let complete = [
            self.decks[0]
                .as_ref()
                .map(|d| d.pcm.is_complete())
                .unwrap_or(true),
            self.decks[1]
                .as_ref()
                .map(|d| d.pcm.is_complete())
                .unwrap_or(true),
        ];
        let any_loaded = self.decks[0].is_some() || self.decks[1].is_some();
        // Everything is expressed on deck A's timeline; deck B's material maps
        // onto it shifted by `-ab_offset`, so its end lands there too. Only
        // *loaded* decks may extend the timeline: an empty deck B with a
        // negative offset used to report an end of `-ab_offset`, which kept the
        // transport running through seconds of silence after deck A had ended.
        let ab_offset = self.ab_offset;
        let mut end = 0i64;
        if self.decks[0].is_some() {
            end = end.max(ready[0] as i64);
        }
        if self.decks[1].is_some() {
            end = end.max(ready[1] as i64 - ab_offset);
        }
        let longest = end.max(0) as u64;

        // A seek past the end of a fully decoded programme must report the end,
        // not a playhead in the void. Only when both decks are complete: while
        // decoding, holding the seek target and waiting for it is the better
        // behaviour (see `starving` below).
        if any_loaded && complete[0] && complete[1] && self.pos > longest {
            self.pos = longest;
        }

        // The audible deck decides whether we are starved.
        let active = self.shared.active_deck.load(Ordering::Relaxed) as usize;
        let starving = |pos: u64| {
            if !any_loaded || complete[active] {
                return false;
            }
            let read = pos as i64 + if active == 0 { 0 } else { ab_offset };
            // A read before the start of deck B is silence, not starvation.
            read >= 0 && read >= ready[active] as i64 - 1
        };

        let playing = self.env_target > 0.0;
        if !any_loaded || (starving(self.pos) && playing) {
            // Hold position, output silence (the mix is already zeroed), keep
            // the envelope where it is so playback resumes without a click.
            self.advance_smoothers(n);
            self.shared
                .buffering
                .store(any_loaded && starving(self.pos), Ordering::Relaxed);
            return;
        }

        // Copied out once: the per-sample loop holds `&mut self.decks`, so it
        // cannot also read `self.trim_step`.
        let trim_step = self.trim_step;

        for f in 0..n {
            // Equal-gain crossfade: A/B compares two versions of the *same*
            // programme, so the sum must stay at unity, not at equal power.
            let ga = 1.0 - self.fade;
            let gb = self.fade;

            let mut l = 0.0f32;
            let mut r = 0.0f32;
            let pos = self.pos as i64;

            for (d, g) in [(0usize, ga), (1usize, gb)] {
                if g <= 0.0 {
                    continue;
                }
                if let Some(deck) = self.decks[d].as_mut() {
                    // Per-deck trim glide (level matching / polarity).
                    if deck.gain < deck.gain_target {
                        deck.gain = (deck.gain + trim_step).min(deck.gain_target);
                    } else if deck.gain > deck.gain_target {
                        deck.gain = (deck.gain - trim_step).max(deck.gain_target);
                    }
                    // Deck B is read through the alignment offset. Outside its
                    // decoded region it is silent: holding the first or last
                    // frame would be a lie about the material.
                    let idx = pos + if d == 0 { 0 } else { ab_offset };
                    if idx >= 0 && (idx as usize) < ready[d] {
                        let s = deck.pcm.frame_stereo(idx as usize);
                        let gain = g * deck.gain;
                        // Unity is the common case and must stay exact.
                        if gain == 1.0 {
                            l += s[0];
                            r += s[1];
                        } else {
                            l += s[0] * gain;
                            r += s[1] * gain;
                        }
                    }
                }
            }

            // The envelope is applied *before* it is advanced, so the first
            // sample after a play/seek is exactly zero: that is what makes the
            // de-click ramp actually de-click.
            let amp = self.env * if self.muted { 0.0 } else { self.volume };
            mix[f * 2] = l * amp;
            mix[f * 2 + 1] = r * amp;

            // ---- smoothers, one sample at a time ----
            if self.env < self.env_target {
                self.env = (self.env + self.env_step).min(self.env_target);
            } else if self.env > self.env_target {
                self.env = (self.env - self.env_step).max(self.env_target);
            }
            if self.fade < self.fade_target {
                self.fade = (self.fade + self.fade_step).min(self.fade_target);
            } else if self.fade > self.fade_target {
                self.fade = (self.fade - self.fade_step).max(self.fade_target);
            }
            if self.volume < self.volume_target {
                self.volume = (self.volume + self.volume_step).min(self.volume_target);
            } else if self.volume > self.volume_target {
                self.volume = (self.volume - self.volume_step).max(self.volume_target);
            }

            // ---- transport ----
            // Keep moving while the release tail is still audible, otherwise a
            // pause would fade out on top of one repeated frame.
            if self.env_target > 0.0 || self.env > 0.0 {
                self.pos += 1;
                let wrapped = match self.loop_region {
                    Some((start, end)) if self.loop_enabled && self.pos >= end => {
                        self.pos = start;
                        true
                    }
                    _ => false,
                };
                if !wrapped && self.pos >= longest {
                    if complete[0] && complete[1] {
                        if self.loop_enabled {
                            self.pos = self.loop_region.map(|(s, _)| s).unwrap_or(0);
                        } else {
                            self.pos = longest;
                            // Only announce the end once: `env_target` is our
                            // "was still playing" latch.
                            if self.env_target > 0.0 {
                                self.env_target = 0.0;
                                self.shared.playing.store(false, Ordering::Relaxed);
                                self.shared.ended.store(true, Ordering::Release);
                            }
                        }
                    } else {
                        // Still decoding: hold at the edge.
                        self.pos = longest;
                    }
                }
            }
        }

        // EQ runs on the summed bus so the comparison is apples to apples.
        self.eq.process(mix);

        // Re-evaluate at the *end* of the block: we may have run into the edge
        // of the decoded region half way through it, and the UI has to show the
        // buffering state on this frame rather than on the next one.
        self.shared
            .buffering
            .store(starving(self.pos), Ordering::Relaxed);
    }

    /// Advance the smoothers without producing audio (used while starved).
    fn advance_smoothers(&mut self, n: usize) {
        let steps = n as f32;
        if self.volume < self.volume_target {
            self.volume = (self.volume + self.volume_step * steps).min(self.volume_target);
        } else if self.volume > self.volume_target {
            self.volume = (self.volume - self.volume_step * steps).max(self.volume_target);
        }
        if self.fade < self.fade_target {
            self.fade = (self.fade + self.fade_step * steps).min(self.fade_target);
        } else if self.fade > self.fade_target {
            self.fade = (self.fade - self.fade_step * steps).max(self.fade_target);
        }
    }
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

/// Engine construction / device preferences.
///
/// The four fields the Settings "Audio device" panel writes (SPEC §16) are
/// `host_id`, `device_name`, the rate pair and `buffer_frames`. All four are
/// preferences, not promises: whatever the hardware actually grants comes back
/// in [`EngineSource`].
#[derive(Clone, Debug)]
pub struct EngineConfig {
    /// cpal host / audio API to use, matching [`HostInfo::id`] — `coreaudio`,
    /// `wasapi`, `asio`, `alsa`, `jack`. `None` for whatever cpal considers
    /// the platform default.
    pub host_id: Option<String>,
    /// Exact output device name, or `None` for the system default.
    pub device_name: Option<String>,
    /// Try to run the device at the source sample rate (bit-transparent path).
    /// **Default, and it should stay that way**: turning it off means every
    /// file that is not already at `fallback_rate` gets resampled.
    pub follow_source_rate: bool,
    /// Rate used when the source rate is unavailable, and the *fixed* rate the
    /// engine runs at when `follow_source_rate` is off.
    pub fallback_rate: u32,
    /// Preferred buffer size in frames, or `None` to let the backend choose.
    /// Clamped to what the device advertises; see [`BufferRange`].
    pub buffer_frames: Option<u32>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig {
            host_id: None,
            device_name: None,
            follow_source_rate: true,
            fallback_rate: FALLBACK_RATE,
            buffer_frames: None,
        }
    }
}

/// A change to the engine's audio source (SPEC §16).
///
/// Every field is "leave it alone" when `None`, so the app layer can change
/// one thing without restating the rest. Applied atomically: one stream
/// rebuild, one answer.
///
/// ```no_run
/// # use onyx_core::engine::{AudioEngine, EngineConfig, SourceRequest};
/// # let engine = AudioEngine::new(EngineConfig::default()).unwrap();
/// let source = engine.set_source(&SourceRequest {
///     device_name: Some("Studio Monitors".into()),
///     buffer_frames: Some(256),
///     ..Default::default()
/// })?;
/// println!("{} ms", source.latency_ms.unwrap_or(0.0));
/// # Ok::<(), onyx_core::Error>(())
/// ```
#[derive(Clone, Debug, Default)]
pub struct SourceRequest {
    /// Host to use. See [`list_hosts`].
    pub host_id: Option<String>,
    /// Device to use, by name. Ignored when `use_system_default_device` is set.
    pub device_name: Option<String>,
    /// Follow the OS default output device instead of pinning one by name.
    /// This is how the UI expresses "System default", which `device_name:
    /// None` cannot, that being "unchanged".
    pub use_system_default_device: bool,
    /// Rate to open the device at. With `follow_source_rate` on this is only
    /// the starting rate — loading a file moves it again.
    pub sample_rate: Option<u32>,
    /// Turn source-rate following on or off. Off pins the engine to
    /// `sample_rate` (or the configured fallback) and resamples everything
    /// else, which is why on is the default.
    pub follow_source_rate: Option<bool>,
    /// Buffer size in frames, clamped to the device's advertised range.
    pub buffer_frames: Option<u32>,
}

pub struct AudioEngine {
    shared: Arc<RtShared>,
    cmds: Arc<ArrayQueue<RtCmd>>,
    meters: Arc<RwLock<MeterSnapshot>>,
    meter_ctl: Arc<MeterCtl>,
    to_host: Sender<HostCmd>,
    config: Mutex<EngineConfig>,
    /// Device name actually in use.
    current_device: Mutex<Option<String>>,
    /// The last stream that was successfully opened, as opened (SPEC §16).
    current_source: Mutex<Option<EngineSource>>,
    /// Mirrors of values the UI needs to read back.
    volume: Mutex<f32>,
    muted: Mutex<bool>,
    loop_enabled: Mutex<bool>,
    loop_region: Mutex<Option<(f64, f64)>>,
    ab_enabled: Mutex<bool>,
    crossfade_ms: Mutex<f32>,
    eq: Mutex<EqConfig>,
    /// Mirror of the EQ audition (band solo) target, so the UI can read it back.
    eq_audition: Mutex<Option<(f32, f32)>>,
    monitor_mode: Mutex<MonitorMode>,
    /// A/B alignment offset kept in *seconds* so that a device rate change
    /// cannot silently rescale it out from under the UI.
    ab_offset_secs: Mutex<f64>,
    deck_inverted: Mutex<[bool; 2]>,
}

impl AudioEngine {
    /// Build the engine and open the output device.
    pub fn new(config: EngineConfig) -> Result<Arc<AudioEngine>> {
        let rate = config.fallback_rate.max(8_000);
        let shared = Arc::new(RtShared::new(rate));
        let cmds = Arc::new(ArrayQueue::new(512));
        // Deep enough that the callback never has to leak (see `recycle`).
        let garbage = Arc::new(ArrayQueue::new(256));

        // ~1 s of stereo audio for the meter tap.
        let (meter_tx, meter_rx) = rtrb::RingBuffer::<f32>::new(192_000 * 2);
        let core = Arc::new(Mutex::new(RtCore::new(
            rate,
            Arc::clone(&shared),
            Arc::clone(&cmds),
            Arc::clone(&garbage),
            meter_tx,
        )));

        let meters = Arc::new(RwLock::new(MeterSnapshot::default()));
        let meter_ctl = Arc::new(MeterCtl {
            reset: AtomicBool::new(false),
            reset_transient: AtomicBool::new(false),
            rate: AtomicU32::new(rate),
            running: AtomicBool::new(true),
            spectrum: AtomicBool::new(true),
        });
        spawn_analysis_thread(meter_rx, Arc::clone(&meters), Arc::clone(&meter_ctl), rate)?;

        let (to_host, from_engine) = bounded::<HostCmd>(8);
        // Note: `core` and `garbage` are deliberately *not* stored on the
        // AudioEngine. The host thread and the cpal callback own the only
        // references they need, and holding extra clones here would only keep
        // decoded buffers alive after shutdown.
        let engine = Arc::new(AudioEngine {
            shared: Arc::clone(&shared),
            cmds: Arc::clone(&cmds),
            meters,
            meter_ctl: Arc::clone(&meter_ctl),
            to_host,
            config: Mutex::new(config.clone()),
            current_device: Mutex::new(None),
            current_source: Mutex::new(None),
            volume: Mutex::new(1.0),
            muted: Mutex::new(false),
            loop_enabled: Mutex::new(false),
            loop_region: Mutex::new(None),
            ab_enabled: Mutex::new(false),
            crossfade_ms: Mutex::new(8.0),
            eq: Mutex::new(EqConfig::default()),
            eq_audition: Mutex::new(None),
            monitor_mode: Mutex::new(MonitorMode::Stereo),
            ab_offset_secs: Mutex::new(0.0),
            deck_inverted: Mutex::new([false; 2]),
        });

        spawn_host_thread(
            from_engine,
            Arc::clone(&core),
            Arc::clone(&shared),
            Arc::clone(&garbage),
            Arc::clone(&meter_ctl),
        )?;

        // Open the device now so failures surface immediately.
        engine.rebuild_stream(&config, rate)?;
        Ok(engine)
    }

    // -- device management --------------------------------------------------

    fn rebuild_stream(&self, config: &EngineConfig, rate: u32) -> Result<EngineSource> {
        let (tx, rx) = bounded(1);
        self.to_host
            .send(HostCmd::Rebuild {
                host: config.host_id.clone(),
                device: config.device_name.clone(),
                rate,
                buffer: config.buffer_frames,
                reply: tx,
            })
            .map_err(|_| Error::NotRunning)?;
        let outcome = rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| Error::Device("output device did not respond".into()))??;
        *self.current_device.lock() = outcome.device_name.clone();
        self.meter_ctl.rate.store(outcome.rate, Ordering::Release);
        let source = EngineSource {
            host_id: outcome.host_id,
            device_name: outcome.device_name,
            following_system_default: outcome.following_system_default,
            sample_rate: outcome.rate,
            buffer_frames: outcome.buffer_frames,
            latency_ms: outcome.buffer_frames.map(|f| latency_ms(f, outcome.rate)),
            follow_source_rate: config.follow_source_rate,
        };
        *self.current_source.lock() = Some(source.clone());
        Ok(source)
    }

    /// Audio APIs available on this machine (SPEC §16).
    pub fn list_hosts(&self) -> Vec<HostInfo> {
        list_hosts()
    }

    /// Available output devices on the host currently in use.
    pub fn list_devices(&self) -> Vec<DeviceInfo> {
        list_output_devices_for_host(self.config.lock().host_id.as_deref())
    }

    /// Available output devices on a specific host, for the Settings panel's
    /// "audio API" dropdown — the user picks an API before they pick a device.
    pub fn list_devices_for_host(&self, host_id: Option<&str>) -> Vec<DeviceInfo> {
        list_output_devices_for_host(host_id)
    }

    /// Change host, device, rate and/or buffer size in one go, **while
    /// playing** (SPEC §16).
    ///
    /// The stream is torn down and rebuilt; the playhead, the transport state,
    /// the loop region, the A/B offset and both decks survive it, because they
    /// live in the real-time core rather than in the stream, and a rate change
    /// rescales the frame-based ones.
    ///
    /// On failure the previous stream is restored by the audio host thread and
    /// the preferences are rolled back, so a device that has been unplugged
    /// costs you an error toast and nothing else. Never panics when the target
    /// is absent: it falls back to the system default, and to the previous
    /// stream if even that fails.
    ///
    /// Returns what is actually playing, including the resulting latency in
    /// milliseconds, which is not necessarily what was asked for.
    pub fn set_source(&self, req: &SourceRequest) -> Result<EngineSource> {
        let previous = self.config.lock().clone();
        let mut next = previous.clone();
        if let Some(host) = &req.host_id {
            next.host_id = Some(host.clone());
        }
        if req.use_system_default_device {
            next.device_name = None;
        } else if let Some(device) = &req.device_name {
            next.device_name = Some(device.clone());
        }
        if let Some(follow) = req.follow_source_rate {
            next.follow_source_rate = follow;
        }
        if let Some(buffer) = req.buffer_frames {
            next.buffer_frames = Some(buffer.max(1));
        }
        // A fixed-rate engine remembers the rate it is pinned to, so the next
        // rebuild (or restart) uses it rather than the last file's rate.
        let rate = match req.sample_rate {
            Some(r) if r >= 8_000 => r,
            _ => {
                if next.follow_source_rate {
                    self.shared.engine_rate()
                } else {
                    next.fallback_rate
                }
            }
        };
        if !next.follow_source_rate {
            next.fallback_rate = rate;
        }

        *self.config.lock() = next.clone();
        match self.rebuild_stream(&next, rate) {
            Ok(source) => Ok(source),
            Err(e) => {
                // Otherwise the next rate change would keep re-trying a device
                // we already know does not work.
                *self.config.lock() = previous;
                Err(e)
            }
        }
    }

    /// What the engine is playing through right now, for the Settings panel.
    ///
    /// `None` only before the first stream has been opened, which cannot
    /// happen through [`AudioEngine::new`] — it opens one or fails.
    pub fn current_source(&self) -> Option<EngineSource> {
        let mut source = self.current_source.lock().clone();
        if let Some(s) = source.as_mut() {
            // The rate moves without a rebuild whenever a file at a different
            // rate is loaded, so read it from the horse's mouth.
            s.sample_rate = self.shared.engine_rate();
            s.latency_ms = s.buffer_frames.map(|f| latency_ms(f, s.sample_rate));
            s.follow_source_rate = self.config.lock().follow_source_rate;
        }
        source
    }

    /// The engine's device preferences, for persisting to settings.
    pub fn config(&self) -> EngineConfig {
        self.config.lock().clone()
    }

    /// Switch device, keeping the current rate if the new device supports it.
    ///
    /// Thin wrapper over [`AudioEngine::set_source`]; `None` means the system
    /// default device.
    pub fn set_device(&self, name: Option<String>) -> Result<u32> {
        let source = self.set_source(&SourceRequest {
            device_name: name.clone(),
            use_system_default_device: name.is_none(),
            ..Default::default()
        })?;
        Ok(source.sample_rate)
    }

    /// Switch audio API. Devices are host-specific, so the device preference
    /// is dropped and the new host's default is used until the user picks one.
    pub fn set_host(&self, host_id: Option<String>) -> Result<EngineSource> {
        self.set_source(&SourceRequest {
            host_id,
            use_system_default_device: true,
            ..Default::default()
        })
    }

    /// Ask for a buffer size in frames. Clamped to what the device supports;
    /// the granted size and its latency come back in the result.
    pub fn set_buffer_frames(&self, frames: u32) -> Result<EngineSource> {
        self.set_source(&SourceRequest {
            buffer_frames: Some(frames),
            ..Default::default()
        })
    }

    pub fn current_device(&self) -> Option<String> {
        self.current_device.lock().clone()
    }

    pub fn engine_rate(&self) -> u32 {
        self.shared.engine_rate()
    }

    /// Output latency of the current buffer at the current rate, in ms.
    /// `None` when the backend chose the buffer size itself and will not say
    /// what it is.
    pub fn latency_ms(&self) -> Option<f32> {
        self.current_source().and_then(|s| s.latency_ms)
    }

    /// Ask the device to run at `rate`. Returns the rate actually obtained.
    ///
    /// This is what keeps the signal path bit-transparent: when a 44.1 kHz file
    /// is loaded we re-open CoreAudio/WASAPI at 44.1 kHz instead of resampling.
    /// With `follow_source_rate` off the engine stays where it is and the file
    /// is resampled instead.
    pub fn request_rate(&self, rate: u32) -> Result<u32> {
        if rate == self.shared.engine_rate() {
            return Ok(rate);
        }
        if !self.config.lock().follow_source_rate {
            return Ok(self.shared.engine_rate());
        }
        let config = self.config.lock().clone();
        Ok(self.rebuild_stream(&config, rate)?.sample_rate)
    }

    pub fn set_follow_source_rate(&self, follow: bool) {
        self.config.lock().follow_source_rate = follow;
    }

    pub fn follow_source_rate(&self) -> bool {
        self.config.lock().follow_source_rate
    }

    pub fn shared(&self) -> &Arc<RtShared> {
        &self.shared
    }

    // -- deck / transport ---------------------------------------------------

    fn push(&self, cmd: RtCmd) {
        // The queue is 512 deep; if the callback is not draining it we are
        // already in trouble, so dropping is the least-bad option. Counted
        // rather than logged: this is called at pointer rate from the UI, and
        // a full queue would otherwise produce a line per dropped command.
        // The host drains the count through `take_faults`.
        if self.cmds.push(cmd).is_err() {
            self.shared.dropped_commands.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Faults counted on the real-time path since the last call (see
    /// [`RtFaults`]). Drained by the host so they can be logged off the audio
    /// thread, coalesced.
    pub fn take_faults(&self) -> RtFaults {
        self.shared.take_faults()
    }

    /// Put decoded audio on a deck. `trim_db` is the level-match offset.
    pub fn load_deck(&self, deck: Deck, pcm: Arc<SharedPcm>, trim_db: f32) {
        self.push(RtCmd::LoadDeck {
            deck: deck.index(),
            pcm,
            trim: trim_to_gain(trim_db),
        });
    }

    pub fn clear_deck(&self, deck: Deck) {
        self.push(RtCmd::ClearDeck(deck.index()));
    }

    pub fn set_trim_db(&self, deck: Deck, trim_db: f32) {
        self.push(RtCmd::SetTrim {
            deck: deck.index(),
            trim: trim_to_gain(trim_db),
        });
    }

    pub fn play(&self) {
        self.push(RtCmd::Play);
    }

    pub fn pause(&self) {
        self.push(RtCmd::Pause);
    }

    pub fn toggle(&self) -> bool {
        if self.shared.is_playing() {
            self.pause();
            false
        } else {
            self.play();
            true
        }
    }

    pub fn stop(&self) {
        self.push(RtCmd::Stop);
        self.reset_meters_transient();
    }

    pub fn seek_secs(&self, secs: f64) {
        let frames = (secs.max(0.0) * self.shared.engine_rate() as f64) as u64;
        self.push(RtCmd::Seek(frames));
        self.reset_meters_transient();
    }

    pub fn seek_frames(&self, frames: u64) {
        self.push(RtCmd::Seek(frames));
        self.reset_meters_transient();
    }

    pub fn set_volume(&self, v: f32) {
        let v = if v.is_finite() {
            v.clamp(0.0, 2.0)
        } else {
            1.0
        };
        *self.volume.lock() = v;
        self.push(RtCmd::SetVolume(v));
    }

    pub fn volume(&self) -> f32 {
        *self.volume.lock()
    }

    pub fn set_muted(&self, m: bool) {
        *self.muted.lock() = m;
        self.push(RtCmd::SetMute(m));
    }

    pub fn muted(&self) -> bool {
        *self.muted.lock()
    }

    pub fn set_loop_enabled(&self, l: bool) {
        *self.loop_enabled.lock() = l;
        self.push(RtCmd::SetLoop(l));
    }

    pub fn loop_enabled(&self) -> bool {
        *self.loop_enabled.lock()
    }

    /// Set (or clear) an A-B loop region in seconds.
    ///
    /// Non-finite or negative bounds are rejected rather than converted: a
    /// region of `(0, 0)` frames would make the transport wrap on every sample
    /// and freeze the playhead, which looks like a hang rather than a bad
    /// argument.
    pub fn set_loop_region(&self, region: Option<(f64, f64)>) {
        let rate = self.shared.engine_rate() as f64;
        let normalised = region.and_then(|(a, b)| {
            if !a.is_finite() || !b.is_finite() {
                // The IPC layer rejects this before it ever gets here, so this
                // is a defensive branch rather than something a user can hit:
                // detail, not a warning.
                log::debug!("ignoring a loop region with non-finite bounds ({a}, {b})");
                return None;
            }
            let (a, b) = if a <= b { (a, b) } else { (b, a) };
            let (a, b) = (a.max(0.0), b.max(0.0));
            if b - a < 0.02 {
                None
            } else {
                Some((a, b))
            }
        });
        let frames = normalised.and_then(|(a, b)| {
            let (a, b) = ((a * rate) as u64, (b * rate) as u64);
            if b > a {
                Some((a, b))
            } else {
                None
            }
        });
        *self.loop_region.lock() = if frames.is_some() { normalised } else { None };
        self.push(RtCmd::SetLoopRegion(frames));
    }

    pub fn loop_region(&self) -> Option<(f64, f64)> {
        *self.loop_region.lock()
    }

    // -- A/B ---------------------------------------------------------------

    pub fn set_ab_enabled(&self, enabled: bool) {
        *self.ab_enabled.lock() = enabled;
        self.shared.ab_enabled.store(enabled, Ordering::Relaxed);
        if !enabled {
            self.select_deck(Deck::A);
        }
    }

    pub fn ab_enabled(&self) -> bool {
        *self.ab_enabled.lock()
    }

    /// Switch the audible deck. Position is untouched, which is the whole point
    /// of the feature: you hear the same bar of music from the other file.
    pub fn select_deck(&self, deck: Deck) {
        self.push(RtCmd::SelectDeck(deck.index()));
        self.reset_meters_transient();
    }

    pub fn active_deck(&self) -> Deck {
        self.shared.active_deck()
    }

    pub fn set_crossfade_ms(&self, ms: f32) {
        let ms = if ms.is_finite() {
            ms.clamp(0.0, 200.0)
        } else {
            DEFAULT_CROSSFADE_SECS * 1000.0
        };
        *self.crossfade_ms.lock() = ms;
        let frames = (ms / 1000.0 * self.shared.engine_rate() as f32) as u32;
        self.push(RtCmd::SetCrossfadeFrames(frames));
    }

    pub fn crossfade_ms(&self) -> f32 {
        *self.crossfade_ms.lock()
    }

    // -- EQ ----------------------------------------------------------------

    /// Install a whole EQ configuration. This is the single authoritative
    /// setter (SPEC §12): the UI owns the band list, the engine owns the
    /// legal ranges. Returns the configuration as it was actually applied.
    pub fn set_eq(&self, cfg: EqConfig) -> EqConfig {
        let rate = self.shared.engine_rate() as f64;
        let cfg = cfg.sanitised(rate);
        self.push(RtCmd::SetEq(EqSetting::from_config(&cfg)));
        *self.eq.lock() = cfg.clone();
        cfg
    }

    pub fn eq_config(&self) -> EqConfig {
        self.eq.lock().clone()
    }

    /// Band-solo sweep filter on the master, after the EQ. `None` disables it.
    pub fn set_eq_audition(&self, freq_hz: Option<f32>, q: f32) {
        let q = if q.is_finite() {
            q.clamp(0.3, 40.0)
        } else {
            4.0
        };
        let freq_hz = freq_hz.filter(|f| f.is_finite() && *f > 0.0);
        *self.eq_audition.lock() = freq_hz.map(|f| (f, q));
        self.push(RtCmd::SetAudition { freq_hz, q });
    }

    pub fn eq_audition(&self) -> Option<(f32, f32)> {
        *self.eq_audition.lock()
    }

    /// Magnitude response of the current EQ at `freqs`, in dB.
    pub fn eq_curve(&self, freqs: &[f32]) -> Vec<f32> {
        // Computed from the host-side copy so the audio thread is untouched.
        let cfg = self.eq.lock().clone();
        let rate = self.shared.engine_rate() as f64;
        let mut out = Vec::with_capacity(freqs.len());
        crate::dsp::eq::curve_db(&cfg, rate, freqs, &mut out);
        out
    }

    // -- monitor matrix / alignment / polarity -------------------------------

    /// Monitoring fold, applied after the meter tap (SPEC §6).
    pub fn set_monitor_mode(&self, mode: MonitorMode) {
        *self.monitor_mode.lock() = mode;
        self.push(RtCmd::SetMonitor(mode));
    }

    pub fn monitor_mode(&self) -> MonitorMode {
        *self.monitor_mode.lock()
    }

    /// Signed deck-B read offset in frames at the current engine rate.
    pub fn set_ab_offset_frames(&self, frames: i64) {
        let rate = self.shared.engine_rate() as f64;
        let limit = (MAX_AB_OFFSET_SECS * rate) as i64;
        let frames = frames.clamp(-limit, limit);
        *self.ab_offset_secs.lock() = frames as f64 / rate;
        self.push(RtCmd::SetAbOffset(frames));
        self.reset_meters_transient();
    }

    pub fn ab_offset_frames(&self) -> i64 {
        let rate = self.shared.engine_rate() as f64;
        (*self.ab_offset_secs.lock() * rate).round() as i64
    }

    /// Flip a deck's polarity (the `ø` button). Applied through the trim
    /// glide, so it never steps the signal.
    pub fn set_deck_invert(&self, deck: Deck, invert: bool) {
        self.deck_inverted.lock()[deck.index()] = invert;
        self.push(RtCmd::SetInvert {
            deck: deck.index(),
            invert,
        });
    }

    pub fn deck_inverted(&self, deck: Deck) -> bool {
        self.deck_inverted.lock()[deck.index()]
    }

    // -- meters -------------------------------------------------------------

    pub fn meters(&self) -> MeterSnapshot {
        self.meters.read().clone()
    }

    pub fn reset_meters(&self) {
        self.meter_ctl.reset.store(true, Ordering::Release);
    }

    /// Turn the FFT on the analysis thread on or off. A closed analyser panel
    /// should cost nothing (SPEC §12).
    pub fn set_spectrum_enabled(&self, enabled: bool) {
        self.meter_ctl.spectrum.store(enabled, Ordering::Release);
    }

    pub fn spectrum_enabled(&self) -> bool {
        self.meter_ctl.spectrum.load(Ordering::Acquire)
    }

    fn reset_meters_transient(&self) {
        self.meter_ctl
            .reset_transient
            .store(true, Ordering::Release);
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        self.meter_ctl.running.store(false, Ordering::Release);
        let _ = self.to_host.send(HostCmd::Shutdown);
    }
}

// ---------------------------------------------------------------------------
// Host thread: owns the cpal stream
// ---------------------------------------------------------------------------

/// The stream the host thread currently has open, kept so a failed switch can
/// be undone. Not the same thing as the *preference*: this is what is playing.
#[derive(Clone, Debug)]
struct LiveStream {
    host: Option<String>,
    device: Option<String>,
    rate: u32,
    buffer: Option<u32>,
}

fn spawn_host_thread(
    rx: crossbeam_channel::Receiver<HostCmd>,
    core: Arc<Mutex<RtCore>>,
    shared: Arc<RtShared>,
    garbage: Arc<ArrayQueue<Arc<SharedPcm>>>,
    meter_ctl: Arc<MeterCtl>,
) -> Result<()> {
    std::thread::Builder::new()
        .name("onyx-audio-host".into())
        .spawn(move || {
            let mut stream: Option<cpal::Stream> = None;
            // What is currently playing, so a failed switch can be undone.
            let mut live: Option<LiveStream> = None;
            loop {
                match rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(HostCmd::Rebuild {
                        host,
                        device,
                        rate,
                        buffer,
                        reply,
                    }) => {
                        // Tear the old stream down first so exclusive-mode
                        // devices are released before we ask for them again.
                        stream = None;
                        log::debug!(
                            "opening output \"{}\" on {} at {rate} Hz{}",
                            device_label(device.as_deref()),
                            host.as_deref().unwrap_or("the default audio API"),
                            match buffer {
                                Some(b) => format!(", {b} frames"),
                                None => String::new(),
                            }
                        );
                        match start_stream(
                            host.as_deref(),
                            device.as_deref(),
                            rate,
                            buffer,
                            &core,
                            &shared,
                            &meter_ctl,
                        ) {
                            Ok((s, outcome)) => {
                                log::info!(
                                    "output \"{}\" running at {} Hz on {}{}",
                                    device_label(outcome.device_name.as_deref()),
                                    outcome.rate,
                                    outcome.host_id,
                                    match outcome.buffer_frames {
                                        Some(b) => format!(
                                            " ({b} frames, {:.1} ms)",
                                            latency_ms(b, outcome.rate)
                                        ),
                                        None => " (backend-chosen buffer)".to_string(),
                                    }
                                );
                                stream = Some(s);
                                live = Some(LiveStream {
                                    host,
                                    device,
                                    rate: outcome.rate,
                                    buffer: outcome.buffer_frames,
                                });
                                let _ = reply.send(Ok(outcome));
                            }
                            Err(e) => {
                                // Losing a device (unplugged interface, a rate
                                // the new device cannot do, an audio API that
                                // is not installed) must not leave the app
                                // permanently silent: put the previous stream
                                // back and report the failure.
                                let wanted = device_label(device.as_deref()).to_string();
                                if let Some(prev) = live.clone() {
                                    match start_stream(
                                        prev.host.as_deref(),
                                        prev.device.as_deref(),
                                        prev.rate,
                                        prev.buffer,
                                        &core,
                                        &shared,
                                        &meter_ctl,
                                    ) {
                                        Ok((s, prev_outcome)) => {
                                            log::warn!(
                                                "could not open output \"{wanted}\" at {rate} Hz \
                                                 ({e}); still playing through \"{}\" at \
                                                 {} Hz",
                                                device_label(prev.device.as_deref()),
                                                prev_outcome.rate
                                            );
                                            stream = Some(s);
                                            live = Some(LiveStream {
                                                rate: prev_outcome.rate,
                                                buffer: prev_outcome.buffer_frames,
                                                ..prev
                                            });
                                        }
                                        Err(e2) => {
                                            log::error!(
                                                "could not open output \"{wanted}\" at {rate} Hz \
                                                 ({e}) and \"{}\" could not be restored either \
                                                 ({e2}); there is no audio output until a device \
                                                 is selected again",
                                                device_label(prev.device.as_deref())
                                            );
                                            live = None;
                                        }
                                    }
                                } else {
                                    log::error!(
                                        "could not open output \"{wanted}\" at {rate} Hz ({e})"
                                    );
                                }
                                let _ = reply.send(Err(e));
                            }
                        }
                    }
                    Ok(HostCmd::Shutdown) => break,
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                }
                // Free buffers handed back by the callback.
                while garbage.pop().is_some() {}
            }
            drop(stream);
        })
        .map_err(|e| Error::Device(format!("could not start the audio host thread: {e}")))?;
    Ok(())
}

/// Build a stream, tell the core and the meters about the rate it really got,
/// and start it.
#[allow(clippy::too_many_arguments)]
fn start_stream(
    host_id: Option<&str>,
    device: Option<&str>,
    rate: u32,
    buffer: Option<u32>,
    core: &Arc<Mutex<RtCore>>,
    shared: &Arc<RtShared>,
    meter_ctl: &Arc<MeterCtl>,
) -> Result<(cpal::Stream, StreamOutcome)> {
    let (stream, outcome) = build_stream(
        host_id,
        device,
        rate,
        buffer,
        Arc::clone(core),
        Arc::clone(shared),
    )?;
    {
        let mut c = core.lock();
        // Rescales the playhead, the loop region and the A/B offset, so a
        // device or rate change mid-playback resumes where it left off rather
        // than teleporting (SPEC §16).
        c.set_rate(outcome.rate);
    }
    meter_ctl.rate.store(outcome.rate, Ordering::Release);
    stream.play()?;
    Ok((stream, outcome))
}

/// Resolve a host id (`coreaudio`, `wasapi`, `alsa`, ...) to a cpal host.
///
/// An unknown or uninitialisable host falls back to the default one rather
/// than failing: a settings file written on a machine with ASIO must not stop
/// Onyx making a sound on a machine without it.
fn resolve_host(host_id: Option<&str>) -> Result<cpal::Host> {
    let Some(want) = host_id else {
        return Ok(cpal::default_host());
    };
    let want_lc = want.to_ascii_lowercase();
    let found = cpal::available_hosts()
        .into_iter()
        .find(|h| host_key(*h) == want_lc);
    match found.map(cpal::host_from_id) {
        Some(Ok(host)) => Ok(host),
        Some(Err(e)) => {
            log::warn!(
                "audio API \"{want}\" is present but would not start ({e}); \
                 using the default API instead"
            );
            Ok(cpal::default_host())
        }
        None => {
            log::warn!("no audio API called \"{want}\" on this system; using the default API");
            Ok(cpal::default_host())
        }
    }
}

/// Stable, lower-case identifier for a cpal host.
fn host_key(id: cpal::HostId) -> String {
    id.name().to_ascii_lowercase()
}

fn build_stream(
    host_id: Option<&str>,
    device_name: Option<&str>,
    requested_rate: u32,
    requested_buffer: Option<u32>,
    core: Arc<Mutex<RtCore>>,
    shared: Arc<RtShared>,
) -> Result<(cpal::Stream, StreamOutcome)> {
    let host = resolve_host(host_id)?;
    // A named device that is not there falls back to the system default: the
    // interface being unplugged should cost you your preference, not your
    // audio.
    let mut following_system_default = device_name.is_none();
    let device = match device_name {
        Some(name) => match host
            .output_devices()
            .ok()
            .and_then(|mut ds| ds.find(|d| d.name().map(|n| n == name).unwrap_or(false)))
        {
            Some(d) => d,
            None => {
                following_system_default = true;
                log::warn!("output device \"{name}\" is not available; using the system default");
                host.default_output_device().ok_or(Error::NoOutputDevice)?
            }
        },
        None => host.default_output_device().ok_or(Error::NoOutputDevice)?,
    };

    let (config, actual_rate) = pick_config(&device, requested_rate, requested_buffer)?;
    let buffer_frames = match config.buffer_size {
        cpal::BufferSize::Fixed(n) => Some(n),
        cpal::BufferSize::Default => None,
    };
    let channels = config.channels as usize;
    let err_shared = Arc::clone(&shared);

    let stream = device.build_output_stream(
        &config,
        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
            // `try_lock` keeps the callback wait-free: the lock is only held by
            // the host thread while a stream is being replaced.
            match core.try_lock() {
                Some(mut c) => c.process(data, channels),
                None => {
                    data.iter_mut().for_each(|s| *s = 0.0);
                    shared.underruns.fetch_add(1, Ordering::Relaxed);
                }
            }
        },
        move |err| {
            // Real-time path: on CoreAudio and WASAPI cpal invokes this from
            // the device thread, so it must not format, allocate or take the
            // logger's lock. Classify into an atomic and let the host log it.
            note_stream_error(&err_shared, &err);
        },
        None,
    )?;

    let outcome = StreamOutcome {
        // The host that was actually opened, which is not the requested one
        // when the request named an API this machine does not have.
        host_id: host_key(host.id()),
        device_name: device.name().ok(),
        following_system_default,
        rate: actual_rate,
        buffer_frames,
    };
    Ok((stream, outcome))
}

/// Record a cpal stream error without allocating, formatting or locking.
///
/// Split out of the closure in [`build_stream`] so it can be tested on a
/// machine with no audio hardware — and so the allocation-counting test can
/// prove that the error callback is as real-time safe as the data callback.
#[inline]
fn note_stream_error(shared: &RtShared, err: &cpal::StreamError) {
    let code = match err {
        cpal::StreamError::DeviceNotAvailable => FAULT_DEVICE_UNAVAILABLE,
        // Matching the variant is free; `err.description` is *not* touched.
        cpal::StreamError::BackendSpecific { .. } => FAULT_BACKEND,
    };
    shared.last_stream_fault.store(code, Ordering::Relaxed);
    shared.stream_faults.fetch_add(1, Ordering::Relaxed);
    // A stream error means the device dropped audio, which is what the underrun
    // counter in the transport read-out is for.
    shared.underruns.fetch_add(1, Ordering::Relaxed);
}

/// Choose a stream config, preferring the requested rate and stereo f32.
fn pick_config(
    device: &cpal::Device,
    requested_rate: u32,
    requested_buffer: Option<u32>,
) -> Result<(StreamConfig, u32)> {
    let supported: Vec<_> = device.supported_output_configs()?.collect();
    pick_config_from(&supported, requested_rate, requested_buffer)
}

/// The pure half of [`pick_config`]: pick a config out of what a device
/// advertises. Split out so the selection rules can be tested on a machine with
/// no audio hardware at all (see the tests at the bottom of this file).
fn pick_config_from(
    supported: &[cpal::SupportedStreamConfigRange],
    requested_rate: u32,
    requested_buffer: Option<u32>,
) -> Result<(StreamConfig, u32)> {
    let f32_configs: Vec<&cpal::SupportedStreamConfigRange> = supported
        .iter()
        .filter(|c| c.sample_format() == SampleFormat::F32)
        .collect();
    if f32_configs.is_empty() {
        return Err(Error::UnsupportedStreamConfig);
    }

    // Prefer exactly two channels; otherwise the smallest count >= 2, else max.
    let pick_by_channels = |f: &dyn Fn(u16) -> bool| -> Vec<&cpal::SupportedStreamConfigRange> {
        f32_configs
            .iter()
            .copied()
            .filter(|c| f(c.channels()))
            .collect()
    };
    let mut candidates = pick_by_channels(&|ch| ch == 2);
    if candidates.is_empty() {
        candidates = pick_by_channels(&|ch| ch > 2);
    }
    if candidates.is_empty() {
        candidates = f32_configs.clone();
    }

    // Exact rate first: this is the bit-transparent path.
    for c in &candidates {
        if c.min_sample_rate().0 <= requested_rate && requested_rate <= c.max_sample_rate().0 {
            return Ok((
                make_config(c, requested_rate, requested_buffer),
                requested_rate,
            ));
        }
    }

    // Otherwise the closest rate any candidate can actually deliver. cpal
    // ranges are continuous, so clamping into the range *is* the nearest rate
    // that range offers.
    let mut best: Option<(u32, u32, &cpal::SupportedStreamConfigRange)> = None;
    for c in &candidates {
        let rate = requested_rate.clamp(c.min_sample_rate().0, c.max_sample_rate().0);
        let dist = rate.abs_diff(requested_rate);
        // Ties go to the first (device-preferred) entry, and to the higher rate
        // when two ranges are equally far away - upsampling beats downsampling.
        let better = match best {
            None => true,
            Some((best_dist, best_rate, _)) => {
                dist < best_dist || (dist == best_dist && rate > best_rate)
            }
        };
        if better {
            best = Some((dist, rate, *c));
        }
    }
    let (_, rate, chosen) = best.ok_or(Error::UnsupportedStreamConfig)?;
    Ok((make_config(chosen, rate, requested_buffer), rate))
}

/// Human-readable device name for a log line.
fn device_label(device: Option<&str>) -> &str {
    device.unwrap_or("system default")
}

/// Per-sample crossfade increment for a crossfade of `secs` at `rate`.
/// A zero-length crossfade is an instant switch (step of 1.0).
#[inline]
fn crossfade_step(secs: f32, rate: f32) -> f32 {
    let frames = secs * rate;
    if frames < 1.0 {
        1.0
    } else {
        1.0 / frames
    }
}

/// Last line of defence for any gain that came from outside the crate.
///
/// A single NaN trim (an unmeasurable file's LUFS difference, a hostile IPC
/// payload) would otherwise poison the mix bus permanently: NaN times zero is
/// still NaN, so even muting would not clear it.
#[inline]
fn sane_gain(g: f32) -> f32 {
    if g.is_finite() {
        g.max(0.0)
    } else {
        1.0
    }
}

/// Convert a level-match trim in dB to a linear gain. A non-finite trim (an
/// un-measurable file) means "leave this deck alone", i.e. unity.
#[inline]
fn trim_to_gain(trim_db: f32) -> f32 {
    if trim_db.is_finite() {
        crate::db_to_lin(trim_db.clamp(-24.0, 24.0))
    } else {
        1.0
    }
}

/// Turn a supported range into a concrete config at `rate`.
///
/// `requested` is the user's buffer size from Settings (SPEC §16); with
/// nothing chosen we ask for 512 frames, ~10.7 ms at 48 kHz, which keeps A/B
/// switching feeling instant without risking dropouts. Either way the value is
/// clamped into what the device advertises, and a device that reports no range
/// at all (cpal's `SupportedBufferSize::Unknown`) keeps its own default —
/// asking for a fixed size there is how you get a backend error instead of
/// audio.
fn make_config(
    range: &cpal::SupportedStreamConfigRange,
    rate: u32,
    requested: Option<u32>,
) -> StreamConfig {
    let mut cfg = (*range).with_sample_rate(SampleRate(rate)).config();
    if let SupportedBufferSize::Range { min, max } = range.buffer_size() {
        let want = requested.unwrap_or(DEFAULT_BUFFER_FRAMES).clamp(*min, *max);
        cfg.buffer_size = cpal::BufferSize::Fixed(want);
    }
    cfg
}

/// Buffer size asked for when the user has not chosen one.
pub const DEFAULT_BUFFER_FRAMES: u32 = 512;

/// Sample rates worth advertising for a device. A cpal range is continuous, so
/// this is "the standard rates this device covers", not everything it can do.
const STANDARD_RATES: [u32; 8] = [
    44_100, 48_000, 88_200, 96_000, 176_400, 192_000, 352_800, 384_000,
];

/// Every audio API this build can talk to (SPEC §16).
///
/// `available` is the honest answer to "can this be used *here*": a host can be
/// compiled in and still fail to initialise (no JACK server, no ASIO driver),
/// and the UI should grey those out rather than let the user pick one and get
/// an error. Returns an empty list on nothing — there is no panic path here,
/// which matters because this runs on machines with no sound card at all.
pub fn list_hosts() -> Vec<HostInfo> {
    let default_id = cpal::default_host().id();
    cpal::available_hosts()
        .into_iter()
        .map(|id| {
            let (available, device_count) = match cpal::host_from_id(id) {
                Ok(host) => (true, host.output_devices().map(|d| d.count()).unwrap_or(0)),
                Err(_) => (false, 0),
            };
            HostInfo {
                id: host_key(id),
                name: id.name().to_string(),
                is_default: id == default_id,
                available,
                device_count,
            }
        })
        .collect()
}

/// Enumerate output devices with the rates they advertise for f32 output.
///
/// The default host. See [`list_output_devices_for_host`] for a specific API.
pub fn list_output_devices() -> Vec<DeviceInfo> {
    list_output_devices_for_host(None)
}

/// Output devices on `host_id` (`None` = the default host), with the rates and
/// buffer sizes each one advertises (SPEC §16).
///
/// Everything here is best-effort: a device that refuses to describe itself is
/// still listed, with empty rates and no buffer range, because a device you
/// cannot query is not the same as a device that is not there. An absent host
/// yields an empty list rather than an error — which is also what this returns
/// on a headless build machine.
pub fn list_output_devices_for_host(host_id: Option<&str>) -> Vec<DeviceInfo> {
    let Ok(host) = resolve_host(host_id) else {
        return Vec::new();
    };
    let id = host_key(host.id());
    let default_name = host.default_output_device().and_then(|d| d.name().ok());
    let mut out = Vec::new();
    if let Ok(devices) = host.output_devices() {
        for d in devices {
            let name = match d.name() {
                Ok(n) => n,
                Err(_) => continue,
            };
            let mut rates = Vec::new();
            let mut buffer: Option<(u32, u32)> = None;
            let mut max_channels = 0u16;
            if let Ok(configs) = d.supported_output_configs() {
                for c in configs {
                    if c.sample_format() != SampleFormat::F32 {
                        continue;
                    }
                    max_channels = max_channels.max(c.channels());
                    for r in STANDARD_RATES {
                        if r >= c.min_sample_rate().0
                            && r <= c.max_sample_rate().0
                            && !rates.contains(&r)
                        {
                            rates.push(r);
                        }
                    }
                    if let SupportedBufferSize::Range { min, max } = c.buffer_size() {
                        buffer = Some(match buffer {
                            Some((lo, hi)) => (lo.min(*min), hi.max(*max)),
                            None => (*min, *max),
                        });
                    }
                }
            }
            rates.sort_unstable();
            out.push(DeviceInfo {
                is_default: Some(&name) == default_name.as_ref(),
                name,
                sample_rates: rates,
                host_id: id.clone(),
                default_sample_rate: d.default_output_config().ok().map(|c| c.sample_rate().0),
                buffer_frames: buffer.map(|(min, max)| BufferRange::new(min, max)),
                max_channels,
            });
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Analysis thread
// ---------------------------------------------------------------------------

fn spawn_analysis_thread(
    mut rx: rtrb::Consumer<f32>,
    meters: Arc<RwLock<MeterSnapshot>>,
    ctl: Arc<MeterCtl>,
    rate: u32,
) -> Result<()> {
    std::thread::Builder::new()
        .name("onyx-meters".into())
        .spawn(move || {
            let mut bank = MeterBank::new(rate as f32);
            let mut scratch = vec![0.0f32; MAX_BLOCK * 2];
            let mut snapshot = MeterSnapshot::default();
            let mut since_publish = std::time::Instant::now();
            while ctl.running.load(Ordering::Acquire) {
                let wanted = ctl.rate.load(Ordering::Acquire);
                if (wanted as f32 - bank.sample_rate()).abs() > 0.5 {
                    bank.set_sample_rate(wanted as f32);
                }
                bank.set_spectrum_enabled(ctl.spectrum.load(Ordering::Acquire));
                if ctl.reset.swap(false, Ordering::AcqRel) {
                    bank.reset();
                }
                if ctl.reset_transient.swap(false, Ordering::AcqRel) {
                    bank.reset_transient();
                }

                let mut consumed = 0usize;
                while let Ok(chunk) = rx.read_chunk(rx.slots().min(scratch.len()) & !1) {
                    let n = chunk.len();
                    if n == 0 {
                        break;
                    }
                    let (a, b) = chunk.as_slices();
                    scratch[..a.len()].copy_from_slice(a);
                    scratch[a.len()..a.len() + b.len()].copy_from_slice(b);
                    chunk.commit_all();
                    bank.process(&scratch[..n]);
                    consumed += n;
                    if consumed >= scratch.len() * 4 {
                        break;
                    }
                }

                if since_publish.elapsed() >= Duration::from_millis(20) {
                    bank.fill_snapshot(&mut snapshot);
                    *meters.write() = snapshot.clone();
                    since_publish = std::time::Instant::now();
                }
                std::thread::sleep(Duration::from_millis(4));
            }
        })
        .map_err(|e| Error::Device(format!("could not start the meter thread: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pcm::SharedPcm;
    use crate::types::{EqBand, EqConfig, FilterKind};

    fn eq_setting(enabled: bool, bands: &[EqBand]) -> EqSetting {
        EqSetting::from_config(&EqConfig {
            enabled,
            bands: bands.to_vec(),
        })
    }

    /// Drive `RtCore` directly - no audio device required, so this runs in CI.
    fn make_core(rate: u32) -> (RtCore, Arc<RtShared>, Arc<ArrayQueue<RtCmd>>) {
        let shared = Arc::new(RtShared::new(rate));
        let cmds = Arc::new(ArrayQueue::new(512));
        let garbage = Arc::new(ArrayQueue::new(64));
        let (tx, _rx) = rtrb::RingBuffer::<f32>::new(1 << 16);
        let core = RtCore::new(rate, Arc::clone(&shared), Arc::clone(&cmds), garbage, tx);
        (core, shared, cmds)
    }

    /// Everything a test needs to drive the RT core by hand.
    type TestRig = (
        RtCore,
        Arc<RtShared>,
        Arc<ArrayQueue<RtCmd>>,
        Arc<ArrayQueue<Arc<SharedPcm>>>,
    );

    /// Same as [`make_core`] but keeps the garbage queue so a test can check
    /// what the callback handed back instead of dropping.
    fn make_core_with_garbage(rate: u32, garbage_cap: usize) -> TestRig {
        let shared = Arc::new(RtShared::new(rate));
        let cmds = Arc::new(ArrayQueue::new(512));
        let garbage = Arc::new(ArrayQueue::new(garbage_cap));
        let (tx, _rx) = rtrb::RingBuffer::<f32>::new(1 << 16);
        let core = RtCore::new(
            rate,
            Arc::clone(&shared),
            Arc::clone(&cmds),
            Arc::clone(&garbage),
            tx,
        );
        (core, shared, cmds, garbage)
    }

    fn ramp(frames: usize) -> Arc<SharedPcm> {
        let mut s = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let v = i as f32 / frames as f32;
            s.push(v);
            s.push(-v);
        }
        SharedPcm::from_interleaved(2, &s)
    }

    fn dc(frames: usize, value: f32) -> Arc<SharedPcm> {
        SharedPcm::from_interleaved(2, &vec![value; frames * 2])
    }

    #[test]
    fn silent_when_nothing_is_loaded() {
        let (mut core, _s, _c) = make_core(48_000);
        let mut out = vec![1.0f32; 512];
        core.process(&mut out, 2);
        assert!(out.iter().all(|s| *s == 0.0));
    }

    #[test]
    fn plays_and_advances_the_playhead() {
        let (mut core, shared, cmds) = make_core(48_000);
        cmds.push(RtCmd::LoadDeck {
            deck: 0,
            pcm: dc(48_000, 0.5),
            trim: 1.0,
        })
        .unwrap();
        cmds.push(RtCmd::SetCrossfadeFrames(0)).unwrap();
        cmds.push(RtCmd::Play).unwrap();
        let mut out = vec![0.0f32; 2_048];
        core.process(&mut out, 2);
        assert_eq!(shared.position_frames(), 1_024);
        assert!(shared.is_playing());
        // After the 5 ms de-click ramp we should be at full level.
        let tail = out[out.len() - 2];
        assert!((tail - 0.5).abs() < 0.01, "tail {tail}");
    }

    #[test]
    fn pause_holds_the_playhead() {
        let (mut core, shared, cmds) = make_core(48_000);
        cmds.push(RtCmd::LoadDeck {
            deck: 0,
            pcm: dc(48_000, 0.5),
            trim: 1.0,
        })
        .unwrap();
        cmds.push(RtCmd::Play).unwrap();
        let mut out = vec![0.0f32; 1_024];
        core.process(&mut out, 2);
        let pos = shared.position_frames();
        cmds.push(RtCmd::Pause).unwrap();
        // Two blocks: one to ramp down, one fully paused.
        core.process(&mut out, 2);
        core.process(&mut out, 2);
        let after = shared.position_frames();
        assert!(after >= pos && after <= pos + 600, "{pos} -> {after}");
        assert!(!shared.is_playing());
        assert!(out.iter().all(|s| s.abs() < 1e-6), "still audible");
    }

    #[test]
    fn ab_switch_keeps_the_position_and_swaps_the_source() {
        let (mut core, shared, cmds) = make_core(48_000);
        cmds.push(RtCmd::LoadDeck {
            deck: 0,
            pcm: dc(48_000, 0.5),
            trim: 1.0,
        })
        .unwrap();
        cmds.push(RtCmd::LoadDeck {
            deck: 1,
            pcm: dc(48_000, -0.25),
            trim: 1.0,
        })
        .unwrap();
        cmds.push(RtCmd::SetCrossfadeFrames(64)).unwrap();
        cmds.push(RtCmd::Play).unwrap();
        let mut out = vec![0.0f32; 4_096];
        core.process(&mut out, 2);
        assert!((out[out.len() - 2] - 0.5).abs() < 0.01);

        let pos_before = shared.position_frames();
        cmds.push(RtCmd::SelectDeck(1)).unwrap();
        core.process(&mut out, 2);
        assert!(
            (out[out.len() - 2] + 0.25).abs() < 0.01,
            "deck B not audible"
        );
        // The playhead must not jump when switching.
        assert_eq!(shared.position_frames(), pos_before + 2_048);
    }

    #[test]
    fn crossfade_is_equal_gain_so_identical_files_do_not_dip() {
        let (mut core, _shared, cmds) = make_core(48_000);
        cmds.push(RtCmd::LoadDeck {
            deck: 0,
            pcm: dc(48_000, 0.5),
            trim: 1.0,
        })
        .unwrap();
        cmds.push(RtCmd::LoadDeck {
            deck: 1,
            pcm: dc(48_000, 0.5),
            trim: 1.0,
        })
        .unwrap();
        cmds.push(RtCmd::SetCrossfadeFrames(512)).unwrap();
        cmds.push(RtCmd::Play).unwrap();
        let mut out = vec![0.0f32; 2_048];
        core.process(&mut out, 2);
        cmds.push(RtCmd::SelectDeck(1)).unwrap();
        core.process(&mut out, 2);
        // Every sample through the crossfade must stay at 0.5.
        for (i, s) in out.iter().enumerate() {
            assert!(
                (s - 0.5).abs() < 0.01,
                "sample {i} dipped to {s} during the crossfade"
            );
        }
    }

    #[test]
    fn level_match_trim_is_applied() {
        let (mut core, _shared, cmds) = make_core(48_000);
        cmds.push(RtCmd::LoadDeck {
            deck: 0,
            pcm: dc(48_000, 0.5),
            trim: crate::db_to_lin(-6.0),
        })
        .unwrap();
        cmds.push(RtCmd::Play).unwrap();
        let mut out = vec![0.0f32; 8_192];
        core.process(&mut out, 2);
        let v = out[out.len() - 2];
        assert!((v - 0.25).abs() < 0.01, "expected ~0.25, got {v}");
    }

    #[test]
    fn seek_moves_the_playhead_and_declicks() {
        let (mut core, shared, cmds) = make_core(48_000);
        cmds.push(RtCmd::LoadDeck {
            deck: 0,
            pcm: ramp(48_000),
            trim: 1.0,
        })
        .unwrap();
        cmds.push(RtCmd::Play).unwrap();
        let mut out = vec![0.0f32; 1_024];
        core.process(&mut out, 2);
        cmds.push(RtCmd::Seek(24_000)).unwrap();
        core.process(&mut out, 2);
        assert!(shared.position_frames() >= 24_000);
        // The first sample after a seek must be silent (ramp restarts).
        assert!(out[0].abs() < 1e-3, "seek clicked: {}", out[0]);
    }

    #[test]
    fn stops_at_the_end_and_raises_the_ended_flag() {
        let (mut core, shared, cmds) = make_core(48_000);
        cmds.push(RtCmd::LoadDeck {
            deck: 0,
            pcm: dc(1_000, 0.5),
            trim: 1.0,
        })
        .unwrap();
        cmds.push(RtCmd::Play).unwrap();
        let mut out = vec![0.0f32; 4_096];
        core.process(&mut out, 2);
        assert!(shared.take_ended());
        assert!(!shared.is_playing());
        assert_eq!(shared.position_frames(), 1_000);
    }

    #[test]
    fn loop_wraps_within_the_region() {
        let (mut core, shared, cmds) = make_core(48_000);
        cmds.push(RtCmd::LoadDeck {
            deck: 0,
            pcm: dc(48_000, 0.5),
            trim: 1.0,
        })
        .unwrap();
        cmds.push(RtCmd::SetLoopRegion(Some((1_000, 2_000))))
            .unwrap();
        cmds.push(RtCmd::SetLoop(true)).unwrap();
        cmds.push(RtCmd::Seek(1_000)).unwrap();
        cmds.push(RtCmd::Play).unwrap();
        let mut out = vec![0.0f32; 8_192];
        core.process(&mut out, 2);
        let pos = shared.position_frames();
        assert!(
            (1_000..2_000).contains(&pos),
            "playhead escaped the loop: {pos}"
        );
    }

    #[test]
    fn waits_instead_of_glitching_while_still_decoding() {
        let (mut core, shared, cmds) = make_core(48_000);
        // 4800 frames available, buffer not complete.
        let (pcm, mut writer) = SharedPcm::new(2, 48_000, 48_000);
        writer.write_interleaved(&vec![0.5f32; 4_800 * 2]);
        cmds.push(RtCmd::LoadDeck {
            deck: 0,
            pcm,
            trim: 1.0,
        })
        .unwrap();
        cmds.push(RtCmd::Play).unwrap();
        let mut out = vec![0.0f32; 4_096];
        core.process(&mut out, 2); // 2048 frames, fine
        core.process(&mut out, 2); // 4096 -> runs into the edge
        core.process(&mut out, 2); // starved: must be silent, not garbage
        assert!(shared.is_buffering());
        assert!(out.iter().all(|s| s.is_finite()));
        // More audio arrives: playback resumes from where it stopped.
        let pos = shared.position_frames();
        writer.write_interleaved(&vec![0.5f32; 4_800 * 2]);
        core.process(&mut out, 2);
        assert!(shared.position_frames() > pos);
    }

    #[test]
    fn multichannel_devices_get_the_front_pair_only() {
        let (mut core, _shared, cmds) = make_core(48_000);
        cmds.push(RtCmd::LoadDeck {
            deck: 0,
            pcm: dc(48_000, 0.5),
            trim: 1.0,
        })
        .unwrap();
        cmds.push(RtCmd::Play).unwrap();
        let mut out = vec![9.0f32; 512 * 6];
        core.process(&mut out, 6);
        for f in 0..512 {
            for c in 2..6 {
                assert_eq!(out[f * 6 + c], 0.0, "channel {c} should be silent");
            }
        }
    }

    #[test]
    fn mono_devices_get_a_downmix() {
        let (mut core, _shared, cmds) = make_core(48_000);
        let pcm = SharedPcm::from_interleaved(2, &[0.5, -0.5].repeat(48_000));
        cmds.push(RtCmd::LoadDeck {
            deck: 0,
            pcm,
            trim: 1.0,
        })
        .unwrap();
        cmds.push(RtCmd::Play).unwrap();
        let mut out = vec![0.0f32; 4_096];
        core.process(&mut out, 1);
        // L + R cancel exactly.
        assert!(out.iter().all(|s| s.abs() < 1e-6));
    }

    #[test]
    fn command_queue_survives_a_flood() {
        let (mut core, _shared, cmds) = make_core(48_000);
        for i in 0..2_000 {
            let _ = cmds.push(RtCmd::SetVolume((i % 100) as f32 / 100.0));
        }
        let mut out = vec![0.0f32; 512];
        core.process(&mut out, 2);
        assert!(out.iter().all(|s| s.is_finite()));
    }

    // -- smoothers vs. sample rate -----------------------------------------

    /// Following the source sample rate re-opens the device and calls
    /// `set_rate`. Every smoother is specified in *seconds*, so none of them
    /// may change length when that happens - in particular the user's
    /// crossfade setting used to be silently reset to the 8 ms default.
    #[test]
    fn a_rate_change_preserves_the_crossfade_length() {
        let (mut core, _shared, cmds) = make_core(48_000);
        // 100 ms crossfade at 48 kHz = 4800 frames.
        cmds.push(RtCmd::SetCrossfadeFrames(4_800)).unwrap();
        let mut out = vec![0.0f32; 128];
        core.process(&mut out, 2);
        assert!((core.fade_step - 1.0 / 4_800.0).abs() < 1e-9);

        core.set_rate(96_000);
        // Still 100 ms, now 9600 frames.
        assert!(
            (core.fade_step - 1.0 / 9_600.0).abs() < 1e-9,
            "crossfade became {} frames long",
            1.0 / core.fade_step
        );
        // ... and the de-click / volume / trim ramps scale too.
        assert!((core.env_step - 1.0 / (ENV_RAMP_SECS * 96_000.0)).abs() < 1e-9);
        assert!((core.volume_step - 1.0 / (VOLUME_RAMP_SECS * 96_000.0)).abs() < 1e-9);
        assert!((core.trim_step - 1.0 / (TRIM_RAMP_SECS * 96_000.0)).abs() < 1e-9);
    }

    /// A zero-length crossfade must switch on the very next sample rather than
    /// dividing by zero.
    #[test]
    fn a_zero_length_crossfade_is_an_instant_switch() {
        assert_eq!(crossfade_step(0.0, 48_000.0), 1.0);
        assert_eq!(crossfade_step(0.000_01, 48_000.0), 1.0);
        assert!((crossfade_step(0.008, 48_000.0) - 1.0 / 384.0).abs() < 1e-9);
    }

    /// The level-match trim glide must take the same *time* at any rate.
    #[test]
    fn trim_glide_takes_the_same_time_at_any_rate() {
        for rate in [44_100u32, 48_000, 96_000] {
            let (mut core, _shared, cmds) = make_core(rate);
            cmds.push(RtCmd::LoadDeck {
                deck: 0,
                pcm: dc(rate as usize, 1.0),
                trim: 1.0,
            })
            .unwrap();
            cmds.push(RtCmd::Play).unwrap();
            let mut out = vec![0.0f32; 64];
            core.process(&mut out, 2); // settle the play envelope
            cmds.push(RtCmd::SetTrim { deck: 0, trim: 0.0 }).unwrap();

            // Run for exactly TRIM_RAMP_SECS and check we have arrived.
            // Tolerance is 1e-4 (-80 dB) rather than exact zero: the glide is a
            // running f32 subtraction, so ~2000 steps accumulate a few tens of
            // microunits of rounding before the final clamp catches it.
            let frames = (TRIM_RAMP_SECS * rate as f32).ceil() as usize;
            let mut block = vec![0.0f32; frames * 2];
            core.process(&mut block, 2);
            let gain = core.decks[0].as_ref().unwrap().gain;
            assert!(gain.abs() < 1e-4, "at {rate} Hz the trim was still {gain}");

            // Half way through it must still be in transit, i.e. the glide is
            // not instantaneous.
            let (mut core2, _s2, cmds2) = make_core(rate);
            cmds2
                .push(RtCmd::LoadDeck {
                    deck: 0,
                    pcm: dc(rate as usize, 1.0),
                    trim: 1.0,
                })
                .unwrap();
            cmds2.push(RtCmd::Play).unwrap();
            core2.process(&mut out, 2);
            cmds2.push(RtCmd::SetTrim { deck: 0, trim: 0.0 }).unwrap();
            let mut half = vec![0.0f32; (frames / 2) * 2];
            core2.process(&mut half, 2);
            let mid = core2.decks[0].as_ref().unwrap().gain;
            assert!(
                (0.2..0.8).contains(&mid),
                "at {rate} Hz the trim was {mid} half way through the glide"
            );
        }
    }

    // -- real-time safety ---------------------------------------------------

    /// The hard rule for `RtCore::process`: no allocation, no deallocation, no
    /// locking — and no logging, which is all three at once. This test arms a
    /// counting global allocator (see `crate::test_alloc`) *and* installs a
    /// logger that allocates (see `crate::test_log`) around a realistic block -
    /// two loaded decks, an active EQ, a crossfade in flight and a queue full of
    /// commands - and fails if either is touched even once.
    ///
    /// Decks are *replaced* inside the measured region, not only loaded before
    /// it: dropping the buffer a deck used to hold is the one deallocation the
    /// callback is most likely to perform by accident (see `RtCore::recycle`),
    /// and a test that only loads at startup never sees it.
    #[test]
    fn the_callback_never_allocates() {
        // Without this the whole test is theatre: with no logger installed a
        // `log::warn!` in the callback never reaches the allocator.
        crate::test_log::install();
        let (mut core, _shared, cmds, garbage) = make_core_with_garbage(48_000, 256);
        cmds.push(RtCmd::LoadDeck {
            deck: 0,
            pcm: ramp(48_000),
            trim: 1.0,
        })
        .unwrap();
        cmds.push(RtCmd::LoadDeck {
            deck: 1,
            pcm: dc(48_000, 0.25),
            trim: 0.5,
        })
        .unwrap();
        cmds.push(RtCmd::SetEq(eq_setting(
            true,
            &[EqBand::bell(1, 900.0, 3.0, 1.0)],
        )))
        .unwrap();
        cmds.push(RtCmd::Play).unwrap();
        let mut out = vec![0.0f32; 512 * 2];
        // Warm everything up first - the first call may still touch lazily
        // initialised statics (log, TLS, ...), which is not what we measure.
        core.process(&mut out, 2);

        // Band lists are built up front: the *test* is allowed to allocate,
        // the callback is not. Adding and removing bands during playback is
        // the case that would tempt an implementation into a Vec.
        let settings: Vec<EqSetting> = (0..64)
            .map(|i: usize| {
                let bands: Vec<EqBand> = (0..=(i % 4))
                    .map(|k| {
                        EqBand::new(
                            k as u32,
                            if k % 2 == 0 {
                                FilterKind::Bell
                            } else {
                                FilterKind::HighPass
                            },
                            300.0 * (k as f32 + 1.0),
                            3.0,
                            1.0,
                        )
                    })
                    .collect();
                eq_setting(true, &bands)
            })
            .collect();

        let logged_before = crate::test_log::count();
        // Buffers to swap in during the measured region, allocated here where
        // allocation is allowed. Deck A keeps its own material so the swaps are
        // a genuine replacement of a *playing* deck.
        let replacements: Vec<Arc<SharedPcm>> = (0..8)
            .map(|i| {
                if i % 2 == 0 {
                    ramp(9_600)
                } else {
                    dc(9_600, 0.1)
                }
            })
            .collect();
        let (_, hits) = crate::test_alloc::count_allocations(|| {
            for (i, setting) in settings.iter().enumerate() {
                // Keep the command queue busy so `drain_commands` runs too.
                let _ = cmds.push(RtCmd::SetVolume(0.5 + (i % 2) as f32 * 0.25));
                let _ = cmds.push(RtCmd::SelectDeck(i % 2));
                let _ = cmds.push(RtCmd::SetEq(*setting));
                let _ = cmds.push(RtCmd::SetAudition {
                    freq_hz: if i % 3 == 0 { Some(1_000.0) } else { None },
                    q: 6.0,
                });
                let _ = cmds.push(RtCmd::SetMonitor(
                    MonitorMode::ALL[i % MonitorMode::ALL.len()],
                ));
                let _ = cmds.push(RtCmd::SetAbOffset((i as i64 % 7) - 3));
                let _ = cmds.push(RtCmd::SetInvert {
                    deck: 1,
                    invert: i % 2 == 0,
                });
                let _ = cmds.push(RtCmd::Seek(1_000 + i as u64));
                // Load over a deck that is already playing, and clear one, in
                // the middle of everything else. `Arc::clone` bumps a refcount
                // and allocates nothing; the drop of what was there is the
                // callback's problem to avoid.
                if i % 8 == 3 {
                    let _ = cmds.push(RtCmd::LoadDeck {
                        deck: 1,
                        pcm: Arc::clone(&replacements[(i / 8) % replacements.len()]),
                        trim: 0.7,
                    });
                }
                if i % 16 == 11 {
                    let _ = cmds.push(RtCmd::ClearDeck(1));
                }
                core.process(&mut out, 2);
            }
        });
        assert_eq!(
            hits, 0,
            "the output callback hit the allocator {hits} times"
        );
        assert_eq!(
            crate::test_log::count(),
            logged_before,
            "the output callback emitted a log record"
        );
        // The replacements really happened, and every displaced buffer came
        // back on the recycle queue instead of being freed above.
        assert!(
            garbage.len() >= 8,
            "only {} buffers were handed back",
            garbage.len()
        );
    }

    /// Even the multichannel and mono fan-out paths must stay allocation-free,
    /// and so must a block that runs off the end of the decoded region.
    #[test]
    fn the_callback_never_allocates_on_the_odd_paths() {
        crate::test_log::install();
        for channels in [1usize, 2, 6] {
            let (mut core, _shared, cmds) = make_core(48_000);
            cmds.push(RtCmd::LoadDeck {
                deck: 0,
                pcm: ramp(1_024),
                trim: 1.0,
            })
            .unwrap();
            cmds.push(RtCmd::SetLoopRegion(Some((100, 400)))).unwrap();
            cmds.push(RtCmd::SetLoop(true)).unwrap();
            cmds.push(RtCmd::Play).unwrap();
            let mut out = vec![0.0f32; 512 * channels];
            core.process(&mut out, channels);
            let logged_before = crate::test_log::count();
            let (_, hits) = crate::test_alloc::count_allocations(|| {
                for _ in 0..32 {
                    core.process(&mut out, channels);
                }
            });
            assert_eq!(hits, 0, "{channels}-channel path allocated {hits} times");
            assert_eq!(
                crate::test_log::count(),
                logged_before,
                "{channels}-channel path emitted a log record"
            );
        }
    }

    /// A fixture from `tests/fixtures`, which unit tests can reach as well as
    /// integration tests do.
    fn fixture(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    /// Decode a file the way the app does and hand back the finished buffer.
    fn decoded(path: &std::path::Path, rate: u32) -> crate::decode::DecodeHandle {
        let h = crate::decode::open(path, rate, crate::decode::DEFAULT_DECK_BUDGET_BYTES)
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        for _ in 0..4_000 {
            if h.status.is_finished() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(h.status.is_finished(), "{}: decode hung", path.display());
        assert!(h.pcm.frames_ready() > 0, "{}: no audio", path.display());
        h
    }

    /// Write the smallest MIDI file that makes a sound: one held note.
    fn one_note_midi() -> std::path::PathBuf {
        let mut body: Vec<u8> = vec![
            0x00, 0xFF, 0x51, 0x03, 0x07, 0xA1, 0x20, // 120 BPM
            0x00, 0xC0, 0x00, // piano
            0x00, 0x90, 0x3C, 0x64, // note on
            0x83, 0x60, 0x80, 0x3C, 0x00, // note off one beat (0.5 s) later
            0x00, 0xFF, 0x2F, 0x00, // end of track
        ];
        let mut bytes = b"MThd".to_vec();
        bytes.extend_from_slice(&6u32.to_be_bytes());
        bytes.extend_from_slice(&[0, 0, 0, 1, 0x01, 0xE0]); // format 0, 1 track, 480 tpqn
        bytes.extend_from_slice(b"MTrk");
        bytes.extend_from_slice(&(body.len() as u32).to_be_bytes());
        bytes.append(&mut body);
        let path = std::env::temp_dir().join(format!("onyx-rt-{}.mid", std::process::id()));
        std::fs::write(&path, &bytes).unwrap();
        path
    }

    /// The same rule, but on the buffers the app actually produces rather than
    /// on hand-built ramps: a source decoded at the device rate, one that went
    /// through the resampler to get there, and one the MIDI synthesiser
    /// rendered. They differ in length, in how the buffer was allocated and in
    /// whether the deck's material is a whole number of blocks — all of which
    /// the callback has to swallow without touching the allocator, including
    /// when one replaces another mid-playback.
    #[test]
    fn the_callback_never_allocates_on_decoded_or_rendered_sources() {
        crate::test_log::install();
        let midi_path = one_note_midi();
        // Built outside the measured region, where allocation is expected.
        let native = decoded(&fixture("tone.opus"), 48_000);
        let resampled = decoded(&fixture("tone.flac"), 48_000);
        let rendered = decoded(&midi_path, 48_000);
        let _ = std::fs::remove_file(&midi_path);
        assert!(native.bit_transparent, "tone.opus is already 48 kHz");
        assert!(
            !resampled.bit_transparent,
            "tone.flac is 44.1 kHz and must have been resampled"
        );
        assert!(
            rendered.info.synth_bank.is_some(),
            "the MIDI file was not rendered by the synthesiser"
        );
        let sources = [
            Arc::clone(&native.pcm),
            Arc::clone(&resampled.pcm),
            Arc::clone(&rendered.pcm),
        ];

        let (mut core, _shared, cmds, garbage) = make_core_with_garbage(48_000, 256);
        cmds.push(RtCmd::LoadDeck {
            deck: 0,
            pcm: Arc::clone(&sources[0]),
            trim: 1.0,
        })
        .unwrap();
        cmds.push(RtCmd::Play).unwrap();
        let mut out = vec![0.0f32; 512 * 2];
        // Warm-up outside the measurement, as everywhere else in this file.
        core.process(&mut out, 2);

        let logged_before = crate::test_log::count();
        let (_, hits) = crate::test_alloc::count_allocations(|| {
            for i in 0..48usize {
                let _ = cmds.push(RtCmd::LoadDeck {
                    deck: i % 2,
                    pcm: Arc::clone(&sources[i % sources.len()]),
                    trim: 0.8,
                });
                if i % 5 == 0 {
                    let _ = cmds.push(RtCmd::Seek(i as u64 * 977));
                }
                core.process(&mut out, 2);
            }
        });
        assert_eq!(hits, 0, "loading real sources allocated {hits} times");
        assert_eq!(
            crate::test_log::count(),
            logged_before,
            "the callback emitted a log record"
        );
        // Every buffer a deck let go of is on the recycle queue, so the host
        // frees it, not the audio thread.
        assert_eq!(
            garbage.len(),
            47,
            "48 loads over decks that were empty once each should recycle 47 buffers"
        );
        while garbage.pop().is_some() {}
        for (i, pcm) in sources.iter().enumerate() {
            assert!(
                Arc::strong_count(pcm) >= 2,
                "source {i} was freed somewhere it should not have been"
            );
        }
    }

    /// cpal's *error* callback is real-time too: CoreAudio and WASAPI call it
    /// from the device thread. It may only count, never log.
    #[test]
    fn the_stream_error_callback_neither_allocates_nor_logs() {
        crate::test_log::install();
        let shared = Arc::new(RtShared::new(48_000));
        let errors = [
            cpal::StreamError::DeviceNotAvailable,
            cpal::StreamError::BackendSpecific {
                err: cpal::BackendSpecificError {
                    description: "the interface fell over".to_string(),
                },
            },
        ];
        // Warm up: the first call may touch lazily initialised statics.
        note_stream_error(&shared, &errors[0]);
        shared.take_faults();
        // `underruns` is a session total, not a drainable count, so the warm-up
        // call above is already in it.
        let underruns_before = shared.underruns();

        let logged_before = crate::test_log::count();
        let (_, hits) = crate::test_alloc::count_allocations(|| {
            for _ in 0..64 {
                for err in &errors {
                    note_stream_error(&shared, err);
                }
            }
        });
        assert_eq!(hits, 0, "the error callback allocated {hits} times");
        assert_eq!(
            crate::test_log::count(),
            logged_before,
            "the error callback emitted a log record"
        );

        // 128 faults, one drained report: a device erroring on every callback
        // must not be able to write a line per callback.
        let faults = shared.take_faults();
        assert_eq!(faults.stream_faults, 128);
        assert_eq!(faults.last_stream_fault, Some(StreamFault::Backend));
        assert_eq!(shared.underruns() - underruns_before, 128);
        // Draining is a take: nothing is reported twice.
        assert!(shared.take_faults().is_empty());
    }

    /// A full command queue is counted, not logged: `push` runs at pointer rate.
    #[test]
    fn a_dropped_command_is_counted_and_drained_once() {
        crate::test_log::install();
        let shared = Arc::new(RtShared::new(48_000));
        assert!(shared.take_faults().is_empty());
        let logged_before = crate::test_log::count();
        for _ in 0..7 {
            shared.dropped_commands.fetch_add(1, Ordering::Relaxed);
        }
        let faults = shared.take_faults();
        assert_eq!(faults.dropped_commands, 7);
        assert!(!faults.is_empty());
        assert_eq!(faults.last_stream_fault, None, "no stream error happened");
        assert!(shared.take_faults().is_empty(), "drained twice");
        assert_eq!(crate::test_log::count(), logged_before);
    }

    /// Replacing a loaded deck must hand the old buffer to the garbage queue
    /// rather than dropping (and therefore freeing) it in the callback.
    #[test]
    fn replacing_a_deck_recycles_instead_of_freeing() {
        let (mut core, _shared, cmds, garbage) = make_core_with_garbage(48_000, 64);
        let first = ramp(1_024);
        let keepalive = Arc::clone(&first);
        cmds.push(RtCmd::LoadDeck {
            deck: 0,
            pcm: first,
            trim: 1.0,
        })
        .unwrap();
        let mut out = vec![0.0f32; 256];
        core.process(&mut out, 2);
        assert_eq!(garbage.len(), 0);

        cmds.push(RtCmd::LoadDeck {
            deck: 0,
            pcm: ramp(1_024),
            trim: 1.0,
        })
        .unwrap();
        core.process(&mut out, 2);
        assert_eq!(garbage.len(), 1, "old buffer was not recycled");

        // ClearDeck recycles too.
        cmds.push(RtCmd::ClearDeck(0)).unwrap();
        core.process(&mut out, 2);
        assert_eq!(garbage.len(), 2);
        assert_eq!(Arc::strong_count(&keepalive), 2, "test handle + garbage");
    }

    /// When the garbage queue is full the callback leaks rather than freeing.
    /// Ugly, but a bounded leak beats a missed deadline; see `RtCore::recycle`.
    #[test]
    fn a_full_garbage_queue_leaks_rather_than_freeing_in_the_callback() {
        let (mut core, _shared, cmds, garbage) = make_core_with_garbage(48_000, 1);
        let mut out = vec![0.0f32; 256];
        for _ in 0..3 {
            cmds.push(RtCmd::LoadDeck {
                deck: 0,
                pcm: ramp(64),
                trim: 1.0,
            })
            .unwrap();
            core.process(&mut out, 2);
        }
        assert_eq!(garbage.len(), 1, "queue is capacity 1 and stays full");
        let (_, hits) = crate::test_alloc::count_allocations(|| {
            cmds.push(RtCmd::ClearDeck(0)).unwrap();
            core.process(&mut out, 2);
        });
        assert_eq!(hits, 0, "overflow path must not free in the callback");
    }

    /// Teardown is the one place a deck buffer is genuinely *freed*, and it
    /// happens wherever the core is dropped — the host thread, after
    /// `drop(stream)` has already retired the callback (see
    /// [`spawn_host_thread`]). The callback itself only ever reaches a buffer
    /// through `recycle`, which the two tests above pin.
    ///
    /// What this adds is the other half of that bargain: the buffers a core
    /// still holds at teardown must not be leaked by the same
    /// `std::mem::forget` that keeps the overflow path real-time safe. Both
    /// decks, and the recycle queue, have to let go.
    #[test]
    fn dropping_the_core_frees_its_decks_instead_of_leaking_them() {
        let (mut core, _shared, cmds, garbage) = make_core_with_garbage(48_000, 64);
        let held = ramp(2_048);
        for deck in 0..2 {
            cmds.push(RtCmd::LoadDeck {
                deck,
                pcm: Arc::clone(&held),
                trim: 1.0,
            })
            .unwrap();
        }
        // A buffer on the recycle queue as well, so teardown has to deal with
        // both kinds of ownership.
        cmds.push(RtCmd::LoadDeck {
            deck: 1,
            pcm: Arc::clone(&held),
            trim: 1.0,
        })
        .unwrap();
        let mut out = vec![0.0f32; 256];
        core.process(&mut out, 2);
        assert_eq!(garbage.len(), 1, "the replaced buffer was not recycled");
        assert_eq!(
            Arc::strong_count(&held),
            4,
            "test handle + two decks + the recycle queue"
        );

        // Exactly what the host thread does when the engine shuts down.
        drop(core);
        assert_eq!(
            Arc::strong_count(&held),
            2,
            "teardown leaked a deck buffer instead of releasing it"
        );
        while garbage.pop().is_some() {}
        assert_eq!(
            Arc::strong_count(&held),
            1,
            "draining the recycle queue left a reference behind"
        );
    }

    // -- monitor matrix (SPEC §6) -------------------------------------------

    fn stereo_pcm(frames: usize, l: f32, r: f32) -> Arc<SharedPcm> {
        let mut s = Vec::with_capacity(frames * 2);
        for _ in 0..frames {
            s.push(l);
            s.push(r);
        }
        SharedPcm::from_interleaved(2, &s)
    }

    /// Something with structure in both channels, so a fold that quietly
    /// mangles the signal cannot pass by accident.
    fn asymmetric(frames: usize) -> Arc<SharedPcm> {
        let mut s = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let t = i as f32 / 480.0;
            s.push((t).sin() * 0.7);
            s.push((t * 1.37).sin() * 0.31 - 0.05);
        }
        SharedPcm::from_interleaved(2, &s)
    }

    /// Number of frames in one settling block. 2048 frames at 48 kHz is 42 ms,
    /// comfortably longer than the 5 ms de-click ramp and the 5 ms monitor
    /// crossfade, so every block boundary below is a settled state.
    const SETTLE: usize = 2_048;

    /// Play deck A through a core, walking a *schedule* of monitor modes: one
    /// settling block of [`SETTLE`] frames per step, pushing the mode (if any)
    /// just before that block. Then return `frames` frames of output.
    ///
    /// The schedule length - not just its contents - fixes where the playhead
    /// ends up, which is what makes two runs comparable sample-for-sample. A
    /// run of `k` steps always returns the window
    /// `[k * SETTLE, k * SETTLE + frames)` of the programme.
    fn play_with_monitor_schedule(schedule: &[Option<MonitorMode>], frames: usize) -> Vec<f32> {
        let (mut core, _shared, cmds) = make_core(48_000);
        cmds.push(RtCmd::LoadDeck {
            deck: 0,
            pcm: asymmetric(96_000),
            trim: 1.0,
        })
        .unwrap();
        cmds.push(RtCmd::Play).unwrap();
        let mut warm = vec![0.0f32; SETTLE * 2];
        for step in schedule {
            if let Some(m) = *step {
                cmds.push(RtCmd::SetMonitor(m)).unwrap();
            }
            core.process(&mut warm, 2);
        }
        let mut out = vec![0.0f32; frames * 2];
        core.process(&mut out, 2);
        out
    }

    /// `Stereo` is not "a matrix that happens to be the identity": it is an
    /// early-out, and its output has to be bit-identical to never having asked
    /// for a monitor mode at all.
    #[test]
    fn stereo_monitor_is_bit_identical_to_no_matrix() {
        // One step each, so both runs return the *same* window of the same
        // programme; the only difference is whether `SetMonitor(Stereo)` was
        // ever sent. Any deviation is the monitor stage touching the samples.
        let reference = play_with_monitor_schedule(&[None], 1_024);
        let explicit = play_with_monitor_schedule(&[Some(MonitorMode::Stereo)], 1_024);
        assert_eq!(reference, explicit);

        // ... and it must go back to bit-identical after a round trip through
        // another fold, once the 5 ms crossfade has finished. Two steps on both
        // sides again puts the two windows at the same playhead.
        let reference2 = play_with_monitor_schedule(&[None, None], 1_024);
        let round_trip = play_with_monitor_schedule(
            &[Some(MonitorMode::Mono), Some(MonitorMode::Stereo)],
            1_024,
        );
        assert_eq!(round_trip, reference2);
    }

    #[test]
    fn monitor_modes_fold_as_specified() {
        let cases = [
            (MonitorMode::Mono, (0.125f32, 0.125f32)),
            (MonitorMode::Left, (0.5, 0.5)),
            (MonitorMode::Right, (-0.25, -0.25)),
            (MonitorMode::Swap, (-0.25, 0.5)),
            (MonitorMode::Side, (0.375, 0.375)),
            (MonitorMode::FlipRight, (0.5, 0.25)),
        ];
        for (mode, (want_l, want_r)) in cases {
            let (mut core, _shared, cmds) = make_core(48_000);
            cmds.push(RtCmd::LoadDeck {
                deck: 0,
                pcm: stereo_pcm(96_000, 0.5, -0.25),
                trim: 1.0,
            })
            .unwrap();
            cmds.push(RtCmd::SetMonitor(mode)).unwrap();
            cmds.push(RtCmd::Play).unwrap();
            let mut out = vec![0.0f32; 4_096];
            core.process(&mut out, 2);
            let (l, r) = (out[out.len() - 2], out[out.len() - 1]);
            assert!(
                (l - want_l).abs() < 1e-4 && (r - want_r).abs() < 1e-4,
                "{mode:?} produced ({l}, {r}), expected ({want_l}, {want_r})"
            );
        }
    }

    /// Every mode, checked against [`MonitorMode::fold`] rather than against a
    /// second table of numbers.
    ///
    /// `monitor_modes_fold_as_specified` above states what SPEC §6 says each
    /// fold *is*; this states that the bus and the type agree about it. The two
    /// used to be able to drift — the fold was written out as a `match` inside
    /// the render loop as well as as coefficients on the type — and the way that
    /// showed up was a mode behaving differently depending on which of the two
    /// a reader happened to trust. With the loop reading `matrix()` there is one
    /// description left, and this is what keeps it that way: a fold added to
    /// `MonitorMode` with no thought for the bus, or special-cased in the loop,
    /// fails here.
    #[test]
    fn every_monitor_mode_matches_the_shared_fold() {
        for mode in MonitorMode::ALL {
            // Two asymmetric frames: a single pair could pass a matrix whose
            // rows or signs have been swapped.
            for (l, r) in [(0.5f32, -0.25f32), (-0.1, 0.8)] {
                let (mut core, _shared, cmds) = make_core(48_000);
                cmds.push(RtCmd::LoadDeck {
                    deck: 0,
                    pcm: stereo_pcm(96_000, l, r),
                    trim: 1.0,
                })
                .unwrap();
                cmds.push(RtCmd::SetMonitor(mode)).unwrap();
                cmds.push(RtCmd::Play).unwrap();
                // One settled block: past the de-click ramp and the monitor
                // glide, so the bus is sitting exactly on `mode.matrix()`.
                let mut out = vec![0.0f32; SETTLE * 2];
                core.process(&mut out, 2);
                let (got_l, got_r) = (out[out.len() - 2], out[out.len() - 1]);
                let (want_l, want_r) = mode.fold(l, r);
                assert!(
                    (got_l - want_l).abs() < 1e-6 && (got_r - want_r).abs() < 1e-6,
                    "{mode:?} on ({l}, {r}): bus gave ({got_l}, {got_r}), \
                     MonitorMode::fold says ({want_l}, {want_r})"
                );
            }
        }
    }

    /// The meter tap has to sit *before* the fold, otherwise switching to mono
    /// or side would change the LUFS read-out of the programme.
    #[test]
    fn the_monitor_fold_is_downstream_of_the_meter_tap() {
        let shared = Arc::new(RtShared::new(48_000));
        let cmds = Arc::new(ArrayQueue::new(512));
        let garbage = Arc::new(ArrayQueue::new(64));
        let (tx, mut rx) = rtrb::RingBuffer::<f32>::new(1 << 18);
        let mut core = RtCore::new(48_000, Arc::clone(&shared), Arc::clone(&cmds), garbage, tx);

        cmds.push(RtCmd::LoadDeck {
            deck: 0,
            pcm: stereo_pcm(96_000, 0.5, -0.25),
            trim: 1.0,
        })
        .unwrap();
        cmds.push(RtCmd::SetMonitor(MonitorMode::Mono)).unwrap();
        cmds.push(RtCmd::Play).unwrap();
        let mut out = vec![0.0f32; 8_192];
        core.process(&mut out, 2);

        // Output is folded to mono ...
        assert!((out[out.len() - 2] - 0.125).abs() < 1e-4);
        assert!((out[out.len() - 1] - 0.125).abs() < 1e-4);

        // ... but the meters still see the untouched programme.
        let n = rx.slots() & !1;
        let chunk = rx.read_chunk(n).unwrap();
        let (a, b) = chunk.as_slices();
        let tapped: Vec<f32> = a.iter().chain(b.iter()).copied().collect();
        let last_l = tapped[tapped.len() - 2];
        let last_r = tapped[tapped.len() - 1];
        assert!(
            (last_l - 0.5).abs() < 1e-4 && (last_r + 0.25).abs() < 1e-4,
            "meters saw the fold: ({last_l}, {last_r})"
        );
    }

    // -- A/B alignment (SPEC §11) -------------------------------------------

    /// Deck B reads at `playhead + offset`, and outside its material it is
    /// silent rather than holding a sample.
    #[test]
    fn ab_offset_shifts_deck_b_and_reads_outside_are_silent() {
        for offset in [480i64, -480] {
            let (mut core, _shared, cmds) = make_core(48_000);
            // Deck B is a ramp so the exact frame it reads is identifiable.
            let frames = 48_000usize;
            let mut b = Vec::with_capacity(frames * 2);
            for i in 0..frames {
                let v = i as f32 / frames as f32;
                b.push(v);
                b.push(v);
            }
            cmds.push(RtCmd::LoadDeck {
                deck: 0,
                pcm: stereo_pcm(frames, 0.0, 0.0),
                trim: 1.0,
            })
            .unwrap();
            cmds.push(RtCmd::LoadDeck {
                deck: 1,
                pcm: SharedPcm::from_interleaved(2, &b),
                trim: 1.0,
            })
            .unwrap();
            cmds.push(RtCmd::SetCrossfadeFrames(0)).unwrap();
            cmds.push(RtCmd::SelectDeck(1)).unwrap();
            cmds.push(RtCmd::SetAbOffset(offset)).unwrap();
            cmds.push(RtCmd::Seek(10_000)).unwrap();
            cmds.push(RtCmd::Play).unwrap();
            let mut out = vec![0.0f32; 2_048];
            core.process(&mut out, 2);
            // Last rendered frame is playhead 10_000 + 1_023.
            let want = (10_000 + 1_023 + offset) as f32 / frames as f32;
            let got = out[out.len() - 2];
            assert!(
                (got - want).abs() < 1e-3,
                "offset {offset}: got {got}, expected {want}"
            );
        }

        // A negative offset at the very start reads before frame zero: silence.
        let (mut core, _shared, cmds) = make_core(48_000);
        cmds.push(RtCmd::LoadDeck {
            deck: 0,
            pcm: stereo_pcm(48_000, 0.0, 0.0),
            trim: 1.0,
        })
        .unwrap();
        cmds.push(RtCmd::LoadDeck {
            deck: 1,
            pcm: stereo_pcm(48_000, 0.8, 0.8),
            trim: 1.0,
        })
        .unwrap();
        cmds.push(RtCmd::SetCrossfadeFrames(0)).unwrap();
        cmds.push(RtCmd::SelectDeck(1)).unwrap();
        cmds.push(RtCmd::SetAbOffset(-1_000)).unwrap();
        cmds.push(RtCmd::Play).unwrap();
        let mut out = vec![0.0f32; 1_024];
        core.process(&mut out, 2);
        assert!(
            out.iter().all(|s| s.abs() < 1e-6),
            "deck B held a sample instead of going silent before its start"
        );
    }

    /// A positive offset shortens the shared timeline by that much, so the end
    /// of the programme is still detected.
    #[test]
    fn ab_offset_is_accounted_for_at_the_end_of_the_timeline() {
        let (mut core, shared, cmds) = make_core(48_000);
        cmds.push(RtCmd::LoadDeck {
            deck: 1,
            pcm: stereo_pcm(4_000, 0.5, 0.5),
            trim: 1.0,
        })
        .unwrap();
        cmds.push(RtCmd::SelectDeck(1)).unwrap();
        cmds.push(RtCmd::SetAbOffset(1_000)).unwrap();
        cmds.push(RtCmd::Play).unwrap();
        let mut out = vec![0.0f32; 16_384];
        core.process(&mut out, 2);
        assert!(shared.take_ended());
        assert_eq!(shared.position_frames(), 3_000);
    }

    /// The other half of §11's "reads outside B are silence": a *positive*
    /// offset walks off the end of B's material long before A's, and the tail
    /// must be true silence rather than B's last frame held.
    #[test]
    fn deck_b_past_its_end_under_a_positive_offset_is_silent() {
        let (mut core, _shared, cmds) = make_core(48_000);
        // A is silent and long; B is loud and short.
        cmds.push(RtCmd::LoadDeck {
            deck: 0,
            pcm: stereo_pcm(48_000, 0.0, 0.0),
            trim: 1.0,
        })
        .unwrap();
        cmds.push(RtCmd::LoadDeck {
            deck: 1,
            pcm: stereo_pcm(2_000, 0.8, -0.8),
            trim: 1.0,
        })
        .unwrap();
        cmds.push(RtCmd::SetCrossfadeFrames(0)).unwrap();
        cmds.push(RtCmd::SelectDeck(1)).unwrap();
        // B's material covers playhead 0..1_000 only.
        cmds.push(RtCmd::SetAbOffset(1_000)).unwrap();
        cmds.push(RtCmd::Seek(1_200)).unwrap();
        cmds.push(RtCmd::Play).unwrap();
        let mut out = vec![0.0f32; 1_024];
        core.process(&mut out, 2);
        assert!(
            out.iter().all(|s| s.abs() < 1e-6),
            "deck B held its last frame past the end of its material"
        );

        // ... and just inside the material it really is audible, so the test
        // above is not passing because everything is silent.
        let (mut core, _shared, cmds) = make_core(48_000);
        cmds.push(RtCmd::LoadDeck {
            deck: 1,
            pcm: stereo_pcm(2_000, 0.8, -0.8),
            trim: 1.0,
        })
        .unwrap();
        cmds.push(RtCmd::SetCrossfadeFrames(0)).unwrap();
        cmds.push(RtCmd::SelectDeck(1)).unwrap();
        cmds.push(RtCmd::SetAbOffset(1_000)).unwrap();
        cmds.push(RtCmd::Play).unwrap();
        let mut out = vec![0.0f32; 1_024];
        core.process(&mut out, 2);
        assert!(out[out.len() - 2].abs() > 0.1, "nothing was audible at all");
    }

    /// An *unloaded* deck B must not extend the timeline through its offset.
    /// The regression: a negative offset on an empty slot reported an end of
    /// `-offset`, so the transport ran on through seconds of silence after deck
    /// A had finished.
    #[test]
    fn an_empty_deck_b_does_not_extend_the_timeline() {
        let (mut core, shared, cmds) = make_core(48_000);
        cmds.push(RtCmd::LoadDeck {
            deck: 0,
            pcm: stereo_pcm(4_000, 0.5, 0.5),
            trim: 1.0,
        })
        .unwrap();
        cmds.push(RtCmd::SetAbOffset(-20_000)).unwrap();
        cmds.push(RtCmd::Play).unwrap();
        let mut out = vec![0.0f32; 32_768];
        core.process(&mut out, 2);
        assert!(shared.take_ended(), "the programme never ended");
        assert_eq!(shared.position_frames(), 4_000);
    }

    // -- boundaries (SPEC §9.6) ----------------------------------------------

    /// Zero-length and single-frame decks: no hang, no panic, and the end is
    /// announced exactly once.
    #[test]
    fn degenerate_deck_lengths_end_immediately() {
        for frames in [0usize, 1, 2] {
            let (mut core, shared, cmds) = make_core(48_000);
            cmds.push(RtCmd::LoadDeck {
                deck: 0,
                pcm: stereo_pcm(frames, 0.5, 0.5),
                trim: 1.0,
            })
            .unwrap();
            cmds.push(RtCmd::Play).unwrap();
            let mut out = vec![0.0f32; 4_096];
            core.process(&mut out, 2);
            assert!(
                out.iter().all(|s| s.is_finite()),
                "{frames}-frame deck produced a non-finite sample"
            );
            assert!(shared.take_ended(), "{frames}-frame deck never ended");
            assert!(!shared.take_ended(), "the ended flag latched twice");
            assert!(
                shared.position_frames() <= frames as u64,
                "{frames}-frame deck ran to {}",
                shared.position_frames()
            );
        }
    }

    /// A seek to exactly the last frame, one past it, and absurdly far past it.
    /// A fully decoded programme must report the end; nothing may index out of
    /// bounds on the way.
    #[test]
    fn seeks_at_and_beyond_the_end_are_clamped() {
        for target in [4_000u64, 4_001, 1_000_000, u64::MAX / 2] {
            let (mut core, shared, cmds) = make_core(48_000);
            cmds.push(RtCmd::LoadDeck {
                deck: 0,
                pcm: stereo_pcm(4_000, 0.5, 0.5),
                trim: 1.0,
            })
            .unwrap();
            cmds.push(RtCmd::Play).unwrap();
            cmds.push(RtCmd::Seek(target)).unwrap();
            let mut out = vec![0.0f32; 2_048];
            core.process(&mut out, 2);
            assert!(out.iter().all(|s| s.is_finite()));
            assert_eq!(
                shared.position_frames(),
                4_000,
                "seek to {target} did not clamp to the end"
            );
            assert!(shared.take_ended(), "seek to {target} did not end");
        }
    }

    /// A degenerate loop region must be dropped, not honoured: wrapping happens
    /// on `pos >= end`, so `end <= start` would freeze the playhead and look
    /// like a hang.
    #[test]
    fn a_degenerate_loop_region_cannot_freeze_the_playhead() {
        for region in [Some((100u64, 100u64)), Some((400, 100)), Some((0, 0))] {
            let (mut core, shared, cmds) = make_core(48_000);
            cmds.push(RtCmd::LoadDeck {
                deck: 0,
                pcm: stereo_pcm(48_000, 0.5, 0.5),
                trim: 1.0,
            })
            .unwrap();
            cmds.push(RtCmd::SetLoopRegion(region)).unwrap();
            cmds.push(RtCmd::SetLoop(true)).unwrap();
            cmds.push(RtCmd::Play).unwrap();
            let mut out = vec![0.0f32; 2_048];
            core.process(&mut out, 2);
            assert_eq!(
                shared.position_frames(),
                1_024,
                "{region:?} stalled the playhead"
            );
        }
    }

    /// A non-finite gain must never reach the bus: NaN would poison the mix, the
    /// meters and the true-peak history for the rest of the session.
    #[test]
    fn non_finite_gains_never_reach_the_bus() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0] {
            let (mut core, _shared, cmds) = make_core(48_000);
            cmds.push(RtCmd::LoadDeck {
                deck: 0,
                pcm: stereo_pcm(48_000, 0.5, -0.25),
                trim: bad,
            })
            .unwrap();
            cmds.push(RtCmd::SetVolume(bad)).unwrap();
            cmds.push(RtCmd::SetTrim { deck: 0, trim: bad }).unwrap();
            cmds.push(RtCmd::Play).unwrap();
            let mut out = vec![0.0f32; 4_096];
            core.process(&mut out, 2);
            assert!(
                out.iter().all(|s| s.is_finite()),
                "a gain of {bad} produced a non-finite output sample"
            );
            assert!(
                out.iter().all(|s| s.abs() <= 4.0),
                "a gain of {bad} produced a wildly out-of-range sample"
            );
        }
    }

    #[test]
    fn inverting_a_deck_flips_its_polarity() {
        let (mut core, _shared, cmds) = make_core(48_000);
        cmds.push(RtCmd::LoadDeck {
            deck: 0,
            pcm: stereo_pcm(48_000, 0.5, 0.25),
            trim: 1.0,
        })
        .unwrap();
        cmds.push(RtCmd::Play).unwrap();
        let mut out = vec![0.0f32; 4_096];
        core.process(&mut out, 2);
        assert!((out[out.len() - 2] - 0.5).abs() < 1e-3);

        cmds.push(RtCmd::SetInvert {
            deck: 0,
            invert: true,
        })
        .unwrap();
        // Long enough for the trim glide to travel from +1 to -1.
        let mut out = vec![0.0f32; 32_768];
        core.process(&mut out, 2);
        assert!(
            (out[out.len() - 2] + 0.5).abs() < 1e-3,
            "expected -0.5, got {}",
            out[out.len() - 2]
        );
    }

    /// With unity trim, unity volume, no EQ and no fold, the deck's samples
    /// must reach the device untouched.
    #[test]
    fn the_default_path_is_bit_transparent() {
        let (mut core, _shared, cmds) = make_core(48_000);
        let pcm = asymmetric(96_000);
        cmds.push(RtCmd::LoadDeck {
            deck: 0,
            pcm: Arc::clone(&pcm),
            trim: 1.0,
        })
        .unwrap();
        cmds.push(RtCmd::Play).unwrap();
        let mut warm = vec![0.0f32; 4_096];
        core.process(&mut warm, 2); // let the de-click ramp finish
        let start = core.pos as usize;
        let mut out = vec![0.0f32; 2_048];
        core.process(&mut out, 2);
        for f in 0..1_024 {
            let src = pcm.frame_stereo(start + f);
            assert_eq!(
                (out[f * 2], out[f * 2 + 1]),
                (src[0], src[1]),
                "frame {f} was altered"
            );
        }
    }

    /// Regression: a device change (or following the source rate) re-opens the
    /// stream at a different rate. Everything held in *frames* - the playhead,
    /// the loop region, the A/B offset - has to be re-expressed, otherwise the
    /// playhead teleports by 8.8% on a 44.1 -> 48 kHz switch.
    #[test]
    fn a_rate_change_keeps_frame_state_in_time() {
        let (mut core, shared, cmds) = make_core(44_100);
        cmds.push(RtCmd::LoadDeck {
            deck: 0,
            pcm: stereo_pcm(44_100 * 4, 0.5, 0.5),
            trim: 1.0,
        })
        .unwrap();
        cmds.push(RtCmd::SetLoopRegion(Some((44_100, 88_200))))
            .unwrap();
        cmds.push(RtCmd::SetAbOffset(4_410)).unwrap();
        cmds.push(RtCmd::Seek(44_100)).unwrap(); // exactly 1.0 s
        let mut out = vec![0.0f32; 128];
        core.process(&mut out, 2);

        core.set_rate(48_000);
        assert_eq!(core.pos, 48_000, "playhead moved in time");
        assert_eq!(shared.position_frames(), 48_000);
        assert_eq!(core.loop_region, Some((48_000, 96_000)));
        assert_eq!(core.ab_offset, 4_800);
        assert_eq!(shared.engine_rate(), 48_000);
    }

    /// `Stop` returns to the start of the file. Only an *enabled* loop region
    /// redefines where that is.
    #[test]
    fn stop_returns_to_zero_unless_a_loop_is_armed() {
        let (mut core, shared, cmds) = make_core(48_000);
        cmds.push(RtCmd::LoadDeck {
            deck: 0,
            pcm: stereo_pcm(48_000, 0.5, 0.5),
            trim: 1.0,
        })
        .unwrap();
        cmds.push(RtCmd::SetLoopRegion(Some((10_000, 20_000))))
            .unwrap();
        cmds.push(RtCmd::Seek(15_000)).unwrap();
        cmds.push(RtCmd::Stop).unwrap();
        let mut out = vec![0.0f32; 64];
        core.process(&mut out, 2);
        assert_eq!(shared.position_frames(), 0);

        cmds.push(RtCmd::SetLoop(true)).unwrap();
        cmds.push(RtCmd::Seek(15_000)).unwrap();
        cmds.push(RtCmd::Stop).unwrap();
        core.process(&mut out, 2);
        assert_eq!(shared.position_frames(), 10_000);
    }

    // -- device config selection -------------------------------------------
    //
    // `pick_config_from` is the pure part of `pick_config`, so the rate/channel
    // rules can be exercised on a box with no sound card (which is exactly what
    // CI is).

    fn range(
        channels: u16,
        min: u32,
        max: u32,
        fmt: SampleFormat,
    ) -> cpal::SupportedStreamConfigRange {
        cpal::SupportedStreamConfigRange::new(
            channels,
            SampleRate(min),
            SampleRate(max),
            SupportedBufferSize::Range { min: 64, max: 4096 },
            fmt,
        )
    }

    #[test]
    fn picks_the_exact_source_rate_when_the_device_offers_it() {
        // Bit transparency: a 44.1 kHz file must not be resampled.
        let supported = vec![
            range(2, 44_100, 192_000, SampleFormat::F32),
            range(2, 48_000, 48_000, SampleFormat::F32),
        ];
        let (cfg, rate) = pick_config_from(&supported, 44_100, None).unwrap();
        assert_eq!(rate, 44_100);
        assert_eq!(cfg.sample_rate.0, 44_100);
        assert_eq!(cfg.channels, 2);
    }

    #[test]
    fn falls_back_to_the_nearest_available_rate() {
        // Device tops out at 48 kHz; a 96 kHz file has to be resampled down.
        let supported = vec![range(2, 44_100, 48_000, SampleFormat::F32)];
        let (_, rate) = pick_config_from(&supported, 96_000, None).unwrap();
        assert_eq!(rate, 48_000);

        // ... and a 22.05 kHz file has to be resampled up to the minimum.
        let (_, rate) = pick_config_from(&supported, 22_050, None).unwrap();
        assert_eq!(rate, 44_100);
    }

    #[test]
    fn nearest_rate_really_is_the_nearest_across_several_ranges() {
        let supported = vec![
            range(2, 8_000, 8_000, SampleFormat::F32),
            range(2, 44_100, 44_100, SampleFormat::F32),
            range(2, 192_000, 192_000, SampleFormat::F32),
        ];
        let (_, rate) = pick_config_from(&supported, 48_000, None).unwrap();
        assert_eq!(rate, 44_100);
        let (_, rate) = pick_config_from(&supported, 176_400, None).unwrap();
        assert_eq!(rate, 192_000);
    }

    #[test]
    fn prefers_stereo_then_multichannel_then_mono() {
        let supported = vec![
            range(1, 48_000, 48_000, SampleFormat::F32),
            range(8, 48_000, 48_000, SampleFormat::F32),
            range(2, 48_000, 48_000, SampleFormat::F32),
        ];
        let (cfg, _) = pick_config_from(&supported, 48_000, None).unwrap();
        assert_eq!(cfg.channels, 2);

        // No stereo: take a multichannel device and feed the front pair.
        let no_stereo = vec![
            range(1, 48_000, 48_000, SampleFormat::F32),
            range(8, 48_000, 48_000, SampleFormat::F32),
        ];
        let (cfg, _) = pick_config_from(&no_stereo, 48_000, None).unwrap();
        assert_eq!(cfg.channels, 8);

        // Mono-only device is still usable (render() downmixes).
        let mono = vec![range(1, 48_000, 48_000, SampleFormat::F32)];
        let (cfg, _) = pick_config_from(&mono, 48_000, None).unwrap();
        assert_eq!(cfg.channels, 1);
    }

    #[test]
    fn rejects_devices_without_an_f32_path() {
        // The whole engine is f32-internal; we never asked cpal to convert.
        let supported = vec![
            range(2, 48_000, 48_000, SampleFormat::I16),
            range(2, 48_000, 48_000, SampleFormat::U16),
        ];
        assert!(matches!(
            pick_config_from(&supported, 48_000, None),
            Err(Error::UnsupportedStreamConfig)
        ));
        assert!(matches!(
            pick_config_from(&[], 48_000, None),
            Err(Error::UnsupportedStreamConfig)
        ));
    }

    #[test]
    fn ignores_non_f32_ranges_when_choosing_a_rate() {
        // An I16 range that happens to cover the requested rate must not win.
        let supported = vec![
            range(2, 44_100, 192_000, SampleFormat::I16),
            range(2, 48_000, 48_000, SampleFormat::F32),
        ];
        let (cfg, rate) = pick_config_from(&supported, 96_000, None).unwrap();
        assert_eq!(rate, 48_000);
        assert_eq!(cfg.sample_rate.0, 48_000);
    }

    #[test]
    fn asks_for_a_small_fixed_buffer_when_the_device_allows_it() {
        let supported = vec![range(2, 48_000, 48_000, SampleFormat::F32)];
        let (cfg, _) = pick_config_from(&supported, 48_000, None).unwrap();
        assert!(matches!(cfg.buffer_size, cpal::BufferSize::Fixed(512)));

        // And clamps into the device's window rather than asking for the
        // impossible (which some backends reject outright).
        let coarse = vec![cpal::SupportedStreamConfigRange::new(
            2,
            SampleRate(48_000),
            SampleRate(48_000),
            SupportedBufferSize::Range {
                min: 1024,
                max: 2048,
            },
            SampleFormat::F32,
        )];
        let (cfg, _) = pick_config_from(&coarse, 48_000, None).unwrap();
        assert!(matches!(cfg.buffer_size, cpal::BufferSize::Fixed(1024)));
    }

    // -- SPEC §16 engine source -----------------------------------------
    //
    // There is no audio hardware on the machine that runs these, so what can
    // be tested is the arithmetic, the selection rules and the promise that
    // enumeration answers rather than panics when it finds nothing.

    #[test]
    fn a_chosen_buffer_size_is_honoured_and_clamped() {
        let supported = vec![range(2, 48_000, 48_000, SampleFormat::F32)];
        let (cfg, _) = pick_config_from(&supported, 48_000, Some(128)).unwrap();
        assert!(matches!(cfg.buffer_size, cpal::BufferSize::Fixed(128)));

        // Below the device's minimum and above its maximum: clamped, not
        // refused — the UI offers a list, the driver has the last word.
        let (cfg, _) = pick_config_from(&supported, 48_000, Some(1)).unwrap();
        assert!(matches!(cfg.buffer_size, cpal::BufferSize::Fixed(64)));
        let (cfg, _) = pick_config_from(&supported, 48_000, Some(1 << 20)).unwrap();
        assert!(matches!(cfg.buffer_size, cpal::BufferSize::Fixed(4096)));
    }

    #[test]
    fn a_device_with_no_buffer_range_keeps_its_own_default() {
        // cpal reports `Unknown` for backends that will not be told (some ALSA
        // and JACK configurations). Asking for a fixed size there is how you
        // get a backend error instead of audio.
        let supported = vec![cpal::SupportedStreamConfigRange::new(
            2,
            SampleRate(48_000),
            SampleRate(48_000),
            SupportedBufferSize::Unknown,
            SampleFormat::F32,
        )];
        let (cfg, _) = pick_config_from(&supported, 48_000, Some(256)).unwrap();
        assert!(matches!(cfg.buffer_size, cpal::BufferSize::Default));
    }

    #[test]
    fn latency_is_the_buffer_in_milliseconds() {
        assert!((latency_ms(512, 48_000) - 10.666_667).abs() < 1e-4);
        assert!((latency_ms(64, 44_100) - 1.451).abs() < 1e-3);
        // A rate of zero can reach this from a settings file; no division by
        // zero, no NaN in the UI.
        assert_eq!(latency_ms(512, 0), 0.0);
    }

    #[test]
    fn buffer_ranges_offer_the_usual_powers_of_two() {
        let r = BufferRange::new(64, 2_048);
        assert_eq!(r.options, vec![64, 128, 256, 512, 1_024, 2_048]);
        assert_eq!(r.clamp(16), 64);
        assert_eq!(r.clamp(4_096), 2_048);

        // An odd fixed size is still selectable rather than being rounded away.
        let odd = BufferRange::new(96, 96);
        assert_eq!(odd.options, vec![96]);
        assert_eq!(odd.clamp(512), 96);
    }

    /// Enumeration must be safe on a machine with no sound card — which is
    /// both CI and, occasionally, a real user with a dead interface.
    #[test]
    fn enumeration_answers_rather_than_panicking_without_hardware() {
        let hosts = list_hosts();
        for h in &hosts {
            assert!(!h.id.is_empty());
            assert_eq!(h.id, h.id.to_lowercase(), "ids are stable and lower case");
            assert!(!h.name.is_empty());
        }
        // cpal always compiles in at least one host, and exactly one of them
        // is the default.
        assert!(!hosts.is_empty());
        assert_eq!(hosts.iter().filter(|h| h.is_default).count(), 1);

        // Devices: possibly none here, but every entry must be self-describing.
        for d in list_output_devices() {
            assert!(!d.name.is_empty());
            assert!(!d.host_id.is_empty());
            if let Some(b) = &d.buffer_frames {
                assert!(b.min <= b.max && !b.options.is_empty());
            }
        }

        // An audio API this machine does not have is an empty list, not an
        // error and not a panic.
        let none = list_output_devices_for_host(Some("a-host-that-does-not-exist"));
        assert!(none.len() == list_output_devices().len());
    }

    /// A `SourceRequest` with nothing set must not silently change anything —
    /// the app layer sends partial updates from the settings panel.
    #[test]
    fn an_empty_source_request_changes_nothing() {
        let req = SourceRequest::default();
        assert!(req.host_id.is_none());
        assert!(req.device_name.is_none());
        assert!(!req.use_system_default_device);
        assert!(req.sample_rate.is_none());
        assert!(req.follow_source_rate.is_none());
        assert!(req.buffer_frames.is_none());
    }

    /// Following the source rate is the default, because it is what keeps the
    /// bit-transparent path (SPEC §3, SPEC §16).
    #[test]
    fn following_the_source_rate_is_the_default() {
        let cfg = EngineConfig::default();
        assert!(cfg.follow_source_rate);
        assert!(cfg.host_id.is_none());
        assert!(cfg.device_name.is_none());
        assert!(cfg.buffer_frames.is_none());
    }

    /// Starting the engine on a machine with no output device is an error, not
    /// a panic and not a hang.
    #[test]
    fn starting_without_a_device_is_a_clean_error() {
        match AudioEngine::new(EngineConfig::default()) {
            Ok(engine) => {
                // There *is* hardware (a developer's machine): then the engine
                // must be able to describe what it opened.
                let source = engine.current_source().expect("a stream was opened");
                assert_eq!(source.sample_rate, engine.engine_rate());
                if let Some(frames) = source.buffer_frames {
                    let expected = latency_ms(frames, source.sample_rate);
                    assert!((source.latency_ms.unwrap() - expected).abs() < 1e-6);
                }
            }
            Err(e) => {
                // No hardware: a named error the shell can show, and the
                // process is still alive to show it.
                assert!(
                    matches!(
                        e,
                        Error::NoOutputDevice | Error::Device(_) | Error::UnsupportedStreamConfig
                    ),
                    "unexpected error opening the engine: {e:?}"
                );
            }
        }
    }
}
