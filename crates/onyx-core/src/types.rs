use serde::{Deserialize, Serialize};

/// Which comparison deck a track is loaded into.
///
/// Onyx always runs two decks. With A/B disabled only deck `A` is audible; the
/// UI simply hides deck `B`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Deck {
    /// Deck A is the default: with A/B disabled it is the only audible deck.
    #[default]
    A,
    B,
}

impl Deck {
    #[inline]
    pub fn index(self) -> usize {
        match self {
            Deck::A => 0,
            Deck::B => 1,
        }
    }

    #[inline]
    pub fn other(self) -> Deck {
        match self {
            Deck::A => Deck::B,
            Deck::B => Deck::A,
        }
    }

    #[inline]
    pub fn from_index(i: usize) -> Deck {
        if i == 0 {
            Deck::A
        } else {
            Deck::B
        }
    }
}

/// Everything we know about a file after probing / decoding it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackInfo {
    pub path: String,
    pub file_name: String,
    /// Duration in seconds, as reported by the container (may be 0 if unknown).
    pub duration_secs: f64,
    /// Source sample rate in Hz, before any conversion.
    pub sample_rate: u32,
    pub channels: u16,
    /// Bit depth when the codec exposes one (PCM/FLAC/ALAC), otherwise `None`.
    pub bits_per_sample: Option<u32>,
    pub codec: String,
    /// Container as detected **from the file's content**, not its extension
    /// (SPEC §17): a `.wav` holding an MP3 reads `MP3` here.
    pub container: String,
    pub bitrate_kbps: Option<u32>,
    pub is_lossless: bool,
    pub size_bytes: u64,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    /// Name of the SoundFont bank a MIDI file is rendered through
    /// (SPEC §18). `None` for every ordinary audio file.
    pub synth_bank: Option<String>,
    /// Extra material the app layer must fold into the loudness-cache key
    /// (SPEC §8) for sources whose audio is *rendered* rather than read.
    ///
    /// The cache key is `path|size|mtime`. That identifies a `.mid` file but
    /// not the audio it produces, which also depends on the SoundFont in use —
    /// so for MIDI this carries the bank's identity and the app should key on
    /// `format!("{path}|{size}|{mtime}|{}", info.render_key.as_deref().unwrap_or(""))`.
    /// `None` for ordinary files, where the file *is* the audio and the plain
    /// key is already complete.
    pub render_key: Option<String>,
}

impl TrackInfo {
    /// Short technical badge, e.g. `MP4 · AAC · 44.1 kHz · stereo`.
    ///
    /// The container leads because it is what the user recognises and because
    /// codec alone is ambiguous — AAC in an `.m4a` and AAC in a `.mov` are not
    /// the same thing to somebody chasing down a delivery problem. A MIDI file
    /// reads `MIDI · GM · <bank>`: it has no codec, and the bank is the part
    /// that determines what you actually hear.
    pub fn format_badge(&self) -> String {
        if let Some(bank) = self.synth_bank.as_deref() {
            return format!(
                "MIDI · GM · {bank} · {:.1} kHz · stereo",
                self.sample_rate as f64 / 1000.0
            );
        }
        let codec = self.codec.to_uppercase();
        let mut parts = Vec::new();
        // "FLAC · FLAC · ..." helps nobody; collapse the self-describing ones.
        if !self.container.is_empty() && self.container != codec {
            parts.push(self.container.clone());
        }
        parts.push(codec);
        if let Some(bits) = self.bits_per_sample {
            parts.push(format!("{bits} bit"));
        }
        parts.push(format!("{:.1} kHz", self.sample_rate as f64 / 1000.0));
        parts.push(match self.channels {
            1 => "mono".to_string(),
            2 => "stereo".to_string(),
            n => format!("{n}ch"),
        });
        parts.join(" · ")
    }
}

/// Offline loudness analysis of a whole file, used for A/B level matching.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoudnessAnalysis {
    /// Gated integrated loudness in LUFS (`-70.0` when effectively silent).
    pub integrated_lufs: f32,
    /// Loudness range in LU.
    pub lra: f32,
    /// True peak in dBTP (4x oversampled).
    pub true_peak_db: f32,
    /// Sample peak in dBFS.
    pub sample_peak_db: f32,
}

/// Biquad filter shapes offered by the EQ (SPEC §12).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FilterKind {
    /// Symmetric peaking filter. The default shape for a new node.
    #[default]
    Bell,
    LowShelf,
    HighShelf,
    HighPass,
    LowPass,
    Notch,
    BandPass,
}

impl FilterKind {
    /// `true` for the shapes whose steepness is set by `slope_db_oct` rather
    /// than by `q` alone.
    #[inline]
    pub fn has_slope(self) -> bool {
        matches!(self, FilterKind::HighPass | FilterKind::LowPass)
    }

    /// `true` for the shapes that use `gain_db`.
    #[inline]
    pub fn has_gain(self) -> bool {
        matches!(
            self,
            FilterKind::Bell | FilterKind::LowShelf | FilterKind::HighShelf
        )
    }
}

/// A single band of the dynamic EQ (SPEC §12).
///
/// The band list is owned by the front end; the engine only ever receives
/// complete configurations through [`EqConfig`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EqBand {
    /// Stable identity assigned by whoever owns the band list. The engine does
    /// not interpret it; it exists so the UI can track nodes across edits.
    #[serde(default)]
    pub id: u32,
    pub enabled: bool,
    pub kind: FilterKind,
    /// Centre / corner frequency in Hz, 20 .. 20 000 (and below Nyquist).
    pub freq_hz: f32,
    /// Gain in dB, -30 .. +30. Ignored by HP / LP / Notch / BandPass.
    pub gain_db: f32,
    /// Q / resonance, 0.1 .. 40.
    pub q: f32,
    /// HP / LP only: 12, 24 or 48 dB per octave, built from cascaded biquads.
    #[serde(default = "default_slope")]
    pub slope_db_oct: u8,
}

fn default_slope() -> u8 {
    12
}

impl Default for EqBand {
    fn default() -> Self {
        EqBand {
            id: 0,
            enabled: true,
            kind: FilterKind::Bell,
            freq_hz: 1_000.0,
            gain_db: 0.0,
            q: 1.0,
            slope_db_oct: 12,
        }
    }
}

impl EqBand {
    // Both constructors are kept deliberately, for different reasons.
    //
    // `bell` is not dead at all: `EqSetting::default` builds its inline band
    // array out of it, so the real-time EQ depends on it.
    //
    // `new` is called from this repository only by tests, which is worth being
    // explicit about rather than deleting it over. `EqBand` has seven fields,
    // two of which (`enabled`, `slope_db_oct`) have a correct default a caller
    // should not have to know, and `bell` covers only one of the shapes the EQ
    // offers — the one a click on the graph creates. The shell never calls
    // either because bands reach Rust as JSON from the front end and are
    // deserialised, not constructed; a second consumer of the crate (or the
    // next surface that builds a curve in Rust — an offline render, a preset)
    // would. `tests/public_api.rs` pins both as part of the published API for
    // that reason, and `sanitised` is what actually protects the engine either
    // way.

    /// A bell band, the shape a click on the EQ graph creates.
    pub const fn bell(id: u32, freq_hz: f32, gain_db: f32, q: f32) -> Self {
        EqBand {
            id,
            enabled: true,
            kind: FilterKind::Bell,
            freq_hz,
            gain_db,
            q,
            slope_db_oct: 12,
        }
    }

    /// A band of any [`FilterKind`], with the default 12 dB/oct slope.
    pub const fn new(id: u32, kind: FilterKind, freq_hz: f32, gain_db: f32, q: f32) -> Self {
        EqBand {
            id,
            enabled: true,
            kind,
            freq_hz,
            gain_db,
            q,
            slope_db_oct: 12,
        }
    }

    /// Clamp every parameter into the range the engine promises to honour, so
    /// that neither a hostile IPC payload nor a wild UI drag can produce an
    /// unstable filter. `sample_rate` bounds the frequency below Nyquist.
    pub fn sanitised(mut self, sample_rate: f64) -> Self {
        let nyquist_limit = (sample_rate * 0.49) as f32;
        let top = MAX_EQ_FREQ.min(nyquist_limit.max(MIN_EQ_FREQ));
        self.freq_hz = if self.freq_hz.is_finite() {
            self.freq_hz.clamp(MIN_EQ_FREQ, top)
        } else {
            1_000.0
        };
        self.gain_db = if self.gain_db.is_finite() {
            self.gain_db.clamp(-MAX_EQ_GAIN_DB, MAX_EQ_GAIN_DB)
        } else {
            0.0
        };
        self.q = if self.q.is_finite() {
            self.q.clamp(MIN_EQ_Q, MAX_EQ_Q)
        } else {
            1.0
        };
        self.slope_db_oct = match self.slope_db_oct {
            0..=17 => 12,
            18..=35 => 24,
            _ => 48,
        };
        self
    }

    /// Number of cascaded biquad sections this band needs.
    #[inline]
    pub fn sections(&self) -> usize {
        if self.kind.has_slope() {
            match self.slope_db_oct {
                0..=17 => 1,
                18..=35 => 2,
                _ => 4,
            }
        } else {
            1
        }
    }
}

/// Largest band count the realtime side pre-allocates for (SPEC §12).
pub const MAX_BANDS: usize = 16;
/// Steepest slope, in biquad sections (48 dB/oct = four cascaded sections).
pub const MAX_EQ_SECTIONS: usize = 4;
pub const MIN_EQ_FREQ: f32 = 20.0;
pub const MAX_EQ_FREQ: f32 = 20_000.0;
pub const MAX_EQ_GAIN_DB: f32 = 30.0;
pub const MIN_EQ_Q: f32 = 0.1;
pub const MAX_EQ_Q: f32 = 40.0;

/// Full EQ configuration: a dynamic list of 0..=[`MAX_BANDS`] bands.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EqConfig {
    pub enabled: bool,
    pub bands: Vec<EqBand>,
}

impl EqConfig {
    /// Clamp the band list to [`MAX_BANDS`] and every parameter to its legal
    /// range. Applied by the engine, so `eq_get` reports what is really running.
    pub fn sanitised(mut self, sample_rate: f64) -> Self {
        self.bands.truncate(MAX_BANDS);
        for b in self.bands.iter_mut() {
            *b = b.sanitised(sample_rate);
        }
        self
    }

    /// `true` when the EQ cannot change the signal and can be skipped entirely.
    pub fn is_transparent(&self) -> bool {
        !self.enabled || self.bands.iter().all(|b| !b.enabled)
    }
}

/// Monitoring fold applied on the master bus, **after** the meter tap
/// (SPEC §6), so the loudness read-outs always describe the programme.
///
/// There is deliberately no `Mid`: mid *is* `(L+R)/2`, i.e. exactly [`Mono`].
///
/// [`Mono`]: MonitorMode::Mono
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MonitorMode {
    /// L, R — the default, and a genuine bit-transparent no-op.
    #[default]
    Stereo,
    /// (L+R)/2 to both legs: mono-compatibility check.
    Mono,
    /// L to both legs.
    Left,
    /// R to both legs.
    Right,
    /// R, L.
    Swap,
    /// (L-R)/2 to both legs: side / difference solo.
    Side,
    /// L, -R: polarity check.
    FlipRight,
}

/// Identity monitor matrix: the bit-transparent [`MonitorMode::Stereo`] fold.
pub const MONITOR_IDENTITY: [f32; 4] = [1.0, 0.0, 0.0, 1.0];

impl MonitorMode {
    /// The fold as a 2x2 matrix `[l<-l, l<-r, r<-l, r<-r]`.
    ///
    /// Every mode is a *linear* map, which is what lets the engine crossfade
    /// between two modes by interpolating the four coefficients instead of
    /// computing both folds: for any matrices `M0`, `M1` and any `t`,
    /// `((1-t)M0 + t M1) x == (1-t)(M0 x) + t(M1 x)`. Interpolating the matrix
    /// is therefore *identical* to an equal-gain crossfade of the two folds,
    /// and it has the property the output-blend form lacks: the state at any
    /// instant is itself a matrix, so a second mode change part way through a
    /// crossfade can start from where the bus actually is instead of jumping
    /// back to the previous fold.
    #[inline(always)]
    pub const fn matrix(self) -> [f32; 4] {
        match self {
            MonitorMode::Stereo => MONITOR_IDENTITY,
            MonitorMode::Mono => [0.5, 0.5, 0.5, 0.5],
            MonitorMode::Left => [1.0, 0.0, 1.0, 0.0],
            MonitorMode::Right => [0.0, 1.0, 0.0, 1.0],
            MonitorMode::Swap => [0.0, 1.0, 1.0, 0.0],
            MonitorMode::Side => [0.5, -0.5, 0.5, -0.5],
            MonitorMode::FlipRight => [1.0, 0.0, 0.0, -1.0],
        }
    }

    /// Fold one stereo frame. Defined in terms of [`MonitorMode::matrix`] so
    /// that there is exactly one description of each fold in the codebase.
    #[inline(always)]
    pub fn fold(self, l: f32, r: f32) -> (f32, f32) {
        if let MonitorMode::Stereo = self {
            // Not "multiply by the identity": the default fold must be a true
            // no-op (SPEC §6).
            return (l, r);
        }
        let m = self.matrix();
        (m[0] * l + m[1] * r, m[2] * l + m[3] * r)
    }

    /// Wire/UI ordering, used by the settings round-trip test and the shell.
    pub const ALL: [MonitorMode; 7] = [
        MonitorMode::Stereo,
        MonitorMode::Mono,
        MonitorMode::Left,
        MonitorMode::Right,
        MonitorMode::Swap,
        MonitorMode::Side,
        MonitorMode::FlipRight,
    ];

    #[inline]
    pub fn as_u8(self) -> u8 {
        match self {
            MonitorMode::Stereo => 0,
            MonitorMode::Mono => 1,
            MonitorMode::Left => 2,
            MonitorMode::Right => 3,
            MonitorMode::Swap => 4,
            MonitorMode::Side => 5,
            MonitorMode::FlipRight => 6,
        }
    }

    #[inline]
    pub fn from_u8(v: u8) -> MonitorMode {
        MonitorMode::ALL
            .get(v as usize)
            .copied()
            .unwrap_or(MonitorMode::Stereo)
    }
}

/// Transport / engine snapshot handed to the UI on every frame.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransportState {
    pub playing: bool,
    /// Playhead in seconds on the shared A/B timeline.
    pub position_secs: f64,
    /// Length of the longest loaded deck, in seconds.
    pub duration_secs: f64,
    pub volume: f32,
    pub muted: bool,
    pub loop_enabled: bool,
    pub loop_region: Option<(f64, f64)>,
    /// Currently audible deck.
    pub active_deck: Deck,
    pub ab_enabled: bool,
    /// Engine (device) sample rate in Hz.
    pub engine_sample_rate: u32,
    /// True while the active deck is still being decoded.
    pub buffering: bool,
    /// Fraction of the active deck that has been decoded, 0..=1.
    pub decoded_fraction: f32,
    /// `true` when deck A plays through with no sample-rate conversion.
    pub bit_transparent: bool,
    pub output_underruns: u32,
    /// Monitoring fold currently applied to the output (SPEC §6).
    pub monitor_mode: MonitorMode,
}

/// Everything the meter bridge shows. One allocation-free snapshot per frame.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeterSnapshot {
    /// Fast peak with 20 dB/s ballistics, dBFS.
    pub peak_db: [f32; 2],
    /// 1 s max-hold peak, dBFS.
    pub peak_hold_db: [f32; 2],
    /// 300 ms RMS, dBFS.
    pub rms_db: [f32; 2],
    /// Session max true peak, dBTP.
    pub true_peak_db: [f32; 2],
    /// Momentary loudness, 400 ms window, LUFS.
    pub lufs_momentary: f32,
    /// Short-term loudness, 3 s window, LUFS.
    pub lufs_short: f32,
    /// Gated integrated loudness since the last meter reset, LUFS.
    pub lufs_integrated: f32,
    /// Loudness range, LU.
    pub lra: f32,
    /// Stereo correlation, -1..1.
    pub correlation: f32,
    /// Log-spaced spectrum magnitudes in dBFS.
    pub spectrum: Vec<f32>,
    /// Number of consecutive samples that hit or exceeded 0 dBFS.
    pub clip_count: u32,
}

impl Default for MeterSnapshot {
    fn default() -> Self {
        Self {
            peak_db: [crate::MIN_DB; 2],
            peak_hold_db: [crate::MIN_DB; 2],
            rms_db: [crate::MIN_DB; 2],
            true_peak_db: [crate::MIN_DB; 2],
            lufs_momentary: crate::LUFS_SILENCE,
            lufs_short: crate::LUFS_SILENCE,
            lufs_integrated: crate::LUFS_SILENCE,
            lra: 0.0,
            correlation: 0.0,
            spectrum: vec![crate::MIN_DB; crate::dsp::spectrum::BANDS],
            clip_count: 0,
        }
    }
}

/// An audio host (cpal backend / OS audio API) as offered to the user.
///
/// SPEC §16. `id` is the stable identifier to persist in `settings.json`
/// and to hand back to the engine; `name` is what the UI shows. Hosts that the
/// build knows about but that are not usable on this machine are still listed,
/// with `available: false`, so the Settings panel can grey out "ASIO" and say
/// why instead of silently omitting it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostInfo {
    /// Stable identifier, e.g. `coreaudio`, `wasapi`, `asio`, `alsa`.
    pub id: String,
    /// Display name, e.g. `CoreAudio`.
    pub name: String,
    /// True for the host cpal would pick on its own.
    pub is_default: bool,
    /// False when the host is compiled in but cannot be initialised here (no
    /// driver installed, no server running).
    pub available: bool,
    /// How many output devices it reports. `0` with `available: true` means
    /// the API works but there is nothing plugged in.
    pub device_count: usize,
}

/// An output device as offered to the user.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub name: String,
    pub is_default: bool,
    /// Sample rates the device advertises for 2-channel float output.
    pub sample_rates: Vec<u32>,
    /// Host this device belongs to, matching [`HostInfo::id`].
    pub host_id: String,
    /// Rate cpal reports as the device's own default, if it states one.
    pub default_sample_rate: Option<u32>,
    /// Buffer sizes the device supports, in frames. `None` when the backend
    /// only offers its own default size (cpal's `BufferSize::Unknown`), in
    /// which case the UI must not offer a buffer-size control for it.
    pub buffer_frames: Option<BufferRange>,
    /// Highest channel count advertised for float output.
    pub max_channels: u16,
}

/// The range of buffer sizes a device supports, plus the sizes worth offering.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BufferRange {
    pub min: u32,
    pub max: u32,
    /// Powers of two inside `[min, max]`, which is what every other audio
    /// application offers and what drivers are actually tuned for.
    pub options: Vec<u32>,
}

impl BufferRange {
    /// Powers of two from 32 to 8192 that fall inside `[min, max]`, with the
    /// endpoints included so a device with an odd fixed size is still
    /// selectable.
    pub fn new(min: u32, max: u32) -> BufferRange {
        let (min, max) = (min.max(1), max.max(min.max(1)));
        let mut options: Vec<u32> = (5..=13)
            .map(|shift| 1u32 << shift)
            .filter(|n| *n >= min && *n <= max)
            .collect();
        if !options.contains(&min) {
            options.insert(0, min);
        }
        if !options.contains(&max) {
            options.push(max);
        }
        options.sort_unstable();
        options.dedup();
        BufferRange { min, max, options }
    }

    /// Clamp a requested size into the range.
    pub fn clamp(&self, frames: u32) -> u32 {
        frames.clamp(self.min, self.max)
    }
}

/// Output latency in milliseconds for `frames` at `rate`.
///
/// This is the number the Settings panel shows next to the buffer size,
/// because "512 frames" means nothing and "10.7 ms" means everything. It is
/// the buffer's own contribution only: the driver and the hardware add more,
/// and Onyx does not pretend to know how much.
#[inline]
pub fn latency_ms(frames: u32, rate: u32) -> f32 {
    if rate == 0 {
        return 0.0;
    }
    1000.0 * frames as f32 / rate as f32
}

/// What the engine is actually playing through right now (SPEC §16).
///
/// Returned after every device/host/rate/buffer change so the UI can show the
/// result rather than the request — the two differ whenever a device refuses
/// what was asked for, which is exactly when the user needs to be told.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineSource {
    /// Host in use, matching [`HostInfo::id`].
    pub host_id: String,
    /// Device in use. `None` means the stream could not be opened at all.
    pub device_name: Option<String>,
    /// True when the device was chosen by following the OS default rather than
    /// by name, so the UI can show "System default (Studio Monitors)".
    pub following_system_default: bool,
    /// Rate the stream is running at.
    pub sample_rate: u32,
    /// Buffer size in frames if the backend let us choose one.
    pub buffer_frames: Option<u32>,
    /// [`latency_ms`] of `buffer_frames` at `sample_rate`, when both are known.
    pub latency_ms: Option<f32>,
    /// True when the engine follows the source sample rate (the default, and
    /// what keeps the bit-transparent path).
    pub follow_source_rate: bool,
}
