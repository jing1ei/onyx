//! Dynamic parametric EQ (0..=[`MAX_BANDS`] nodes) on the stereo output bus,
//! plus the band-solo audition filter that sits after it (SPEC §12).
//!
//! Three details make this usable while music is playing:
//!
//! 1. **Control-rate smoothing.** Frequency / gain / Q glide towards their
//!    targets and coefficients are re-designed once per 32-sample control
//!    block, so dragging a node sounds like an analogue sweep instead of a
//!    stream of zipper noise.
//! 2. **Topology crossfade.** Adding, removing, re-typing or re-sloping a band
//!    cannot be smoothed - the filter state has to be rebuilt. Instead the wet
//!    bus is faded out over 8 ms, the new topology is installed while it is
//!    silent, and it is faded back in. During the dip the listener hears the
//!    dry signal, so there is never a step discontinuity.
//! 3. **No allocation, ever.** The whole processor is a fixed-size struct;
//!    configuration arrives as a `Copy` [`EqSetting`] with an inline array.

use super::biquad::{Biquad, Coeffs};
use crate::types::{EqBand, EqConfig, FilterKind, MAX_BANDS, MAX_EQ_SECTIONS};

/// Frames between coefficient updates.
const CONTROL_BLOCK: usize = 32;
/// Parameter glide time constant.
const GLIDE_SECS: f64 = 0.02;
/// Dry/wet crossfade time when the topology changes.
const XFADE_SECS: f32 = 0.008;
/// Crossfade time for the audition (band-solo) filter, per SPEC §12.
const AUDITION_XFADE_SECS: f32 = 0.005;

/// A whole EQ configuration in a fixed-size, `Copy` form so it can travel down
/// the lock-free command queue without allocating.
#[derive(Clone, Copy, Debug)]
pub struct EqSetting {
    pub enabled: bool,
    len: usize,
    bands: [EqBand; MAX_BANDS],
}

impl Default for EqSetting {
    fn default() -> Self {
        EqSetting {
            enabled: false,
            len: 0,
            bands: [EqBand {
                enabled: false,
                ..EqBand::bell(0, 1_000.0, 0.0, 1.0)
            }; MAX_BANDS],
        }
    }
}

impl EqSetting {
    /// Flatten a config into the inline array, dropping anything past
    /// [`MAX_BANDS`]. Callers are expected to have sanitised the config first
    /// (see [`EqConfig::sanitised`]).
    pub fn from_config(cfg: &EqConfig) -> Self {
        let mut s = EqSetting {
            enabled: cfg.enabled,
            len: cfg.bands.len().min(MAX_BANDS),
            ..Default::default()
        };
        for (dst, src) in s.bands.iter_mut().zip(cfg.bands.iter()) {
            *dst = *src;
        }
        s
    }

    #[inline]
    pub fn bands(&self) -> &[EqBand] {
        &self.bands[..self.len]
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// `true` when nothing in this configuration can alter the signal.
    #[inline]
    pub fn is_transparent(&self) -> bool {
        !self.enabled || !self.bands().iter().any(|b| b.enabled)
    }
}

#[derive(Clone, Copy)]
struct BandState {
    enabled: bool,
    kind: FilterKind,
    sections: usize,
    target_freq: f64,
    target_gain: f64,
    target_q: f64,
    freq: f64,
    gain: f64,
    q: f64,
    coeffs: [Coeffs; MAX_EQ_SECTIONS],
}

impl BandState {
    const fn idle() -> Self {
        BandState {
            enabled: false,
            kind: FilterKind::Bell,
            sections: 1,
            target_freq: 1_000.0,
            target_gain: 0.0,
            target_q: 1.0,
            freq: 1_000.0,
            gain: 0.0,
            q: 1.0,
            coeffs: [Coeffs::bypass(); MAX_EQ_SECTIONS],
        }
    }

    /// Same filter *shape*? If not, the state has to be rebuilt and the change
    /// has to be crossfaded.
    #[inline]
    fn same_topology(&self, b: &EqBand) -> bool {
        self.enabled == b.enabled && self.kind == b.kind && self.sections == b.sections()
    }

    fn adopt(&mut self, b: &EqBand, sample_rate: f64) {
        self.enabled = b.enabled;
        self.kind = b.kind;
        self.sections = b.sections().clamp(1, MAX_EQ_SECTIONS);
        self.target_freq = b.freq_hz as f64;
        self.target_gain = b.gain_db as f64;
        self.target_q = b.q as f64;
        self.freq = self.target_freq;
        self.gain = self.target_gain;
        self.q = self.target_q;
        self.redesign(sample_rate);
    }

    #[inline]
    fn retarget(&mut self, b: &EqBand) {
        self.target_freq = b.freq_hz as f64;
        self.target_gain = b.gain_db as f64;
        self.target_q = b.q as f64;
    }

    #[inline]
    fn redesign(&mut self, sample_rate: f64) {
        for k in 0..self.sections {
            self.coeffs[k] = if self.enabled {
                Coeffs::design_cascade_section(
                    self.kind,
                    sample_rate,
                    self.freq,
                    self.q,
                    self.gain,
                    self.sections,
                    k,
                )
            } else {
                Coeffs::bypass()
            };
        }
    }

    /// Glide one control block towards the target. Returns `true` if the
    /// coefficients changed enough to warrant a redesign.
    #[inline]
    fn glide(&mut self, alpha: f64) -> bool {
        let df = self.target_freq - self.freq;
        let dg = self.target_gain - self.gain;
        let dq = self.target_q - self.q;
        if df.abs() < 1e-4 && dg.abs() < 1e-5 && dq.abs() < 1e-5 {
            return false;
        }
        // Glide frequency geometrically: a linear sweep from 20 Hz to 20 kHz
        // spends almost all its time in the top octave, which sounds wrong.
        self.freq *= (self.target_freq.max(1.0) / self.freq.max(1.0)).powf(alpha);
        self.gain += dg * alpha;
        self.q += dq * alpha;
        true
    }
}

/// The stereo EQ processor owned by the audio callback.
pub struct StereoEq {
    sample_rate: f64,
    /// Number of band slots currently in use.
    len: usize,
    slots: [BandState; MAX_BANDS],
    filters: [[[Biquad; MAX_EQ_SECTIONS]; MAX_BANDS]; 2],
    /// Dry/wet position, 0 = bypassed, 1 = fully processed.
    wet: f32,
    wet_target: f32,
    wet_step: f32,
    /// A configuration waiting for the wet bus to reach zero before it is
    /// installed. `Copy`, so parking it here allocates nothing.
    pending: Option<EqSetting>,
    /// Filter memory is stale and must be cleared before the next fade-in.
    dirty_state: bool,
    control_phase: usize,
    glide_alpha: f64,
}

impl StereoEq {
    pub fn new(sample_rate: f64) -> Self {
        let mut eq = StereoEq {
            sample_rate: sample_rate.max(8_000.0),
            len: 0,
            slots: [BandState::idle(); MAX_BANDS],
            filters: [[[Biquad::default(); MAX_EQ_SECTIONS]; MAX_BANDS]; 2],
            wet: 0.0,
            wet_target: 0.0,
            wet_step: 1.0,
            pending: None,
            dirty_state: false,
            control_phase: 0,
            glide_alpha: 0.0,
        };
        eq.set_sample_rate(sample_rate);
        eq
    }

    pub fn set_sample_rate(&mut self, sample_rate: f64) {
        self.sample_rate = sample_rate.max(8_000.0);
        let block_secs = CONTROL_BLOCK as f64 / self.sample_rate;
        self.glide_alpha = 1.0 - (-block_secs / GLIDE_SECS).exp();
        self.wet_step = (CONTROL_BLOCK as f32 / (XFADE_SECS * self.sample_rate as f32)).min(1.0);
        for s in self.slots.iter_mut() {
            s.redesign(self.sample_rate);
        }
        self.reset_state();
    }

    pub fn reset_state(&mut self) {
        for ch in self.filters.iter_mut() {
            for band in ch.iter_mut() {
                for f in band.iter_mut() {
                    f.reset();
                }
            }
        }
        self.dirty_state = false;
    }

    /// Number of bands currently installed (for tests / introspection).
    pub fn band_count(&self) -> usize {
        self.len
    }

    /// Install a configuration. Parameter-only edits glide; anything that
    /// changes the filter topology is queued behind a dry/wet dip.
    pub fn apply(&mut self, setting: &EqSetting) {
        let bands = setting.bands();
        let topology_changed = setting.len() != self.len
            || bands
                .iter()
                .zip(self.slots.iter())
                .any(|(b, slot)| !slot.same_topology(b));

        if topology_changed {
            // Defer: fade the wet bus out first, install while inaudible.
            self.pending = Some(*setting);
            self.wet_target = 0.0;
            if self.wet <= 0.0 {
                self.install_pending();
            }
            return;
        }

        self.pending = None;
        for (slot, b) in self.slots.iter_mut().zip(bands.iter()) {
            slot.retarget(b);
        }
        self.wet_target = if setting.is_transparent() { 0.0 } else { 1.0 };
    }

    fn install_pending(&mut self) {
        let Some(setting) = self.pending.take() else {
            return;
        };
        self.len = setting.len();
        for (slot, b) in self.slots.iter_mut().zip(setting.bands().iter()) {
            slot.adopt(b, self.sample_rate);
        }
        for slot in self.slots.iter_mut().skip(self.len) {
            slot.enabled = false;
        }
        self.reset_state();
        // Push the coefficients into the biquads *now*. The control tick only
        // refreshes one band per block, so waiting for it would run the last
        // bands of a 16-band chain in bypass for the first few milliseconds.
        self.push_all_coeffs();
        self.control_phase = 0;
        self.wet_target = if setting.is_transparent() { 0.0 } else { 1.0 };
    }

    fn push_all_coeffs(&mut self) {
        for b in 0..self.len {
            let sections = self.slots[b].sections;
            for s in 0..sections {
                let c = self.slots[b].coeffs[s];
                self.filters[0][b][s].set_coeffs(c);
                self.filters[1][b][s].set_coeffs(c);
            }
        }
    }

    /// Process an interleaved stereo block in place.
    pub fn process(&mut self, buf: &mut [f32]) {
        let frames = buf.len() / 2;
        if frames == 0 {
            return;
        }
        // Silent wet bus with nothing pending: don't touch the audio at all.
        // This is what keeps the default (and the all-bands-off) path
        // bit-transparent.
        if self.wet <= 0.0 && self.wet_target <= 0.0 && self.pending.is_none() {
            if self.dirty_state {
                self.reset_state();
            }
            return;
        }

        let mut i = 0;
        while i < frames {
            let n = CONTROL_BLOCK.min(frames - i);
            // Interpolate the dry/wet position *within* the block. Stepping it
            // once per control block would put a 1.5 kHz staircase on the
            // crossfade, which is audible as a soft click.
            let wet_from = self.wet;
            self.control_tick();
            let wet_to = self.wet;
            let dw = (wet_to - wet_from) / n as f32;
            let len = self.len;
            for f in 0..n {
                let wet = wet_from + dw * f as f32;
                let dry = 1.0 - wet;
                let idx = (i + f) * 2;
                for ch in 0..2 {
                    let x = buf[idx + ch];
                    let mut y = x as f64;
                    for b in 0..len {
                        if self.slots[b].enabled {
                            let sections = self.slots[b].sections;
                            for s in 0..sections {
                                y = self.filters[ch][b][s].process(y);
                            }
                        }
                    }
                    buf[idx + ch] = (y as f32) * wet + x * dry;
                }
            }
            i += n;
        }
        self.dirty_state = true;
        for ch in self.filters.iter_mut() {
            for band in ch.iter_mut() {
                for f in band.iter_mut() {
                    f.sanitise();
                }
            }
        }
    }

    #[inline]
    fn control_tick(&mut self) {
        if self.wet < self.wet_target {
            self.wet = (self.wet + self.wet_step).min(self.wet_target);
        } else if self.wet > self.wet_target {
            self.wet = (self.wet - self.wet_step).max(self.wet_target);
        }

        if self.wet <= 0.0 && self.pending.is_some() {
            self.install_pending();
            return;
        }
        if self.len == 0 {
            return;
        }

        // Stagger redesigns so a full sweep costs one design per control block.
        let n = self.len;
        self.control_phase = (self.control_phase + 1) % n;
        let b = self.control_phase;
        let alpha = (self.glide_alpha * n as f64).min(1.0);
        if self.slots[b].glide(alpha) {
            self.slots[b].redesign(self.sample_rate);
        }
        let sections = self.slots[b].sections;
        for s in 0..sections {
            let c = self.slots[b].coeffs[s];
            self.filters[0][b][s].set_coeffs(c);
            self.filters[1][b][s].set_coeffs(c);
        }
    }
}

/// Combined magnitude response of a configuration, in dB, at each frequency.
///
/// Computed from the *config* rather than from the live filter state so that
/// the curve the user drags matches where the audio lands once the glide
/// settles. Runs on the UI/command thread, never on the audio thread.
pub fn curve_db(cfg: &EqConfig, sample_rate: f64, freqs: &[f32], out: &mut Vec<f32>) {
    out.clear();
    out.reserve(freqs.len());
    for &f in freqs {
        let mut sum = 0.0f64;
        if cfg.enabled {
            for band in cfg.bands.iter().filter(|b| b.enabled) {
                let sections = band.sections();
                for k in 0..sections {
                    let c = Coeffs::design_cascade_section(
                        band.kind,
                        sample_rate,
                        band.freq_hz as f64,
                        band.q as f64,
                        band.gain_db as f64,
                        sections,
                        k,
                    );
                    sum += c.magnitude_db(sample_rate, f as f64);
                }
            }
        }
        out.push(sum as f32);
    }
}

/// Band-solo audition filter (SPEC §12): two cascaded constant-peak
/// band-passes on the master, after the EQ, crossfaded in and out over 5 ms.
///
/// Used for cursor-following solo sweeps, so it is allocation-free and its
/// centre frequency glides at control rate rather than jumping per event.
pub struct AuditionFilter {
    sample_rate: f64,
    active: bool,
    freq: f64,
    target_freq: f64,
    q: f64,
    target_q: f64,
    wet: f32,
    wet_target: f32,
    wet_step: f32,
    coeffs: Coeffs,
    filters: [[Biquad; 2]; 2],
    glide_alpha: f64,
}

impl AuditionFilter {
    pub fn new(sample_rate: f64) -> Self {
        let mut a = AuditionFilter {
            sample_rate: sample_rate.max(8_000.0),
            active: false,
            freq: 1_000.0,
            target_freq: 1_000.0,
            q: 4.0,
            target_q: 4.0,
            wet: 0.0,
            wet_target: 0.0,
            wet_step: 1.0,
            coeffs: Coeffs::bypass(),
            filters: [[Biquad::default(); 2]; 2],
            glide_alpha: 0.0,
        };
        a.set_sample_rate(sample_rate);
        a
    }

    pub fn set_sample_rate(&mut self, sample_rate: f64) {
        self.sample_rate = sample_rate.max(8_000.0);
        let block_secs = CONTROL_BLOCK as f64 / self.sample_rate;
        self.glide_alpha = 1.0 - (-block_secs / GLIDE_SECS).exp();
        self.wet_step =
            (CONTROL_BLOCK as f32 / (AUDITION_XFADE_SECS * self.sample_rate as f32)).min(1.0);
        self.redesign();
        self.reset_state();
    }

    pub fn reset_state(&mut self) {
        for ch in self.filters.iter_mut() {
            for f in ch.iter_mut() {
                f.reset();
            }
        }
    }

    /// `freq_hz: None` disables the audition. Enabling snaps the centre
    /// frequency (the filter is silent at that moment anyway); moving it while
    /// already active glides, which is what a cursor sweep needs.
    pub fn set(&mut self, freq_hz: Option<f32>, q: f32) {
        let q = if q.is_finite() {
            q.clamp(0.3, 40.0)
        } else {
            4.0
        };
        match freq_hz {
            Some(f) if f.is_finite() && f > 0.0 => {
                let f = (f as f64).clamp(10.0, self.sample_rate * 0.49);
                let was_active = self.active;
                self.active = true;
                self.target_freq = f;
                self.target_q = q as f64;
                self.wet_target = 1.0;
                if !was_active {
                    self.freq = f;
                    self.q = q as f64;
                    self.redesign();
                    self.reset_state();
                }
            }
            _ => {
                self.active = false;
                self.wet_target = 0.0;
            }
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    #[inline]
    fn redesign(&mut self) {
        self.coeffs = Coeffs::design(
            FilterKind::BandPass,
            self.sample_rate,
            self.freq,
            self.q,
            0.0,
        );
        let c = self.coeffs;
        for ch in self.filters.iter_mut() {
            for f in ch.iter_mut() {
                f.set_coeffs(c);
            }
        }
    }

    /// Process an interleaved stereo block in place.
    pub fn process(&mut self, buf: &mut [f32]) {
        let frames = buf.len() / 2;
        if frames == 0 {
            return;
        }
        if !self.active && self.wet <= 0.0 {
            return;
        }

        let mut i = 0;
        while i < frames {
            let n = CONTROL_BLOCK.min(frames - i);
            let wet_from = self.wet;
            self.control_tick();
            let wet_to = self.wet;
            let dw = (wet_to - wet_from) / n as f32;
            for f in 0..n {
                let wet = wet_from + dw * f as f32;
                let dry = 1.0 - wet;
                let idx = (i + f) * 2;
                for ch in 0..2 {
                    let x = buf[idx + ch];
                    let mut y = x as f64;
                    y = self.filters[ch][0].process(y);
                    y = self.filters[ch][1].process(y);
                    buf[idx + ch] = (y as f32) * wet + x * dry;
                }
            }
            i += n;
        }
        for ch in self.filters.iter_mut() {
            for f in ch.iter_mut() {
                f.sanitise();
            }
        }
    }

    #[inline]
    fn control_tick(&mut self) {
        if self.wet < self.wet_target {
            self.wet = (self.wet + self.wet_step).min(self.wet_target);
        } else if self.wet > self.wet_target {
            self.wet = (self.wet - self.wet_step).max(self.wet_target);
        }
        let df = self.target_freq - self.freq;
        let dq = self.target_q - self.q;
        if df.abs() > 1e-4 || dq.abs() > 1e-5 {
            let alpha = self.glide_alpha;
            self.freq *= (self.target_freq.max(1.0) / self.freq.max(1.0)).powf(alpha);
            self.q += dq * alpha;
            self.redesign();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::FilterKind;

    fn rms(buf: &[f32]) -> f32 {
        (buf.iter().map(|s| s * s).sum::<f32>() / buf.len().max(1) as f32).sqrt()
    }

    fn sine(freq: f32, sr: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let s = (2.0 * std::f32::consts::PI * freq * i as f32 / sr).sin();
                [s, s]
            })
            .collect()
    }

    fn setting(enabled: bool, bands: &[EqBand]) -> EqSetting {
        EqSetting::from_config(&EqConfig {
            enabled,
            bands: bands.to_vec(),
        })
    }

    /// Drive the EQ until any pending topology swap has faded fully back in.
    fn settle(eq: &mut StereoEq, sr: f32) {
        let mut buf = sine(1_000.0, sr, 4_096);
        eq.process(&mut buf);
    }

    #[test]
    fn empty_band_list_is_bit_transparent() {
        let mut eq = StereoEq::new(48_000.0);
        eq.apply(&setting(true, &[]));
        let original = sine(1_000.0, 48_000.0, 1_024);
        let mut buf = original.clone();
        eq.process(&mut buf);
        assert_eq!(buf, original, "an empty EQ must not alter a single sample");
    }

    #[test]
    fn all_bands_disabled_is_bit_transparent() {
        let mut eq = StereoEq::new(48_000.0);
        eq.apply(&setting(
            true,
            &[EqBand {
                enabled: false,
                ..EqBand::bell(1, 1_000.0, 12.0, 1.0)
            }],
        ));
        settle(&mut eq, 48_000.0);
        let original = sine(1_000.0, 48_000.0, 1_024);
        let mut buf = original.clone();
        eq.process(&mut buf);
        assert_eq!(buf, original);
    }

    #[test]
    fn disabled_config_is_bit_transparent() {
        let mut eq = StereoEq::new(48_000.0);
        eq.apply(&setting(false, &[EqBand::bell(1, 1_000.0, 12.0, 1.0)]));
        settle(&mut eq, 48_000.0);
        let original = sine(1_000.0, 48_000.0, 1_024);
        let mut buf = original.clone();
        eq.process(&mut buf);
        assert_eq!(buf, original);
    }

    #[test]
    fn boost_raises_level_at_centre_frequency() {
        let mut eq = StereoEq::new(48_000.0);
        eq.apply(&setting(true, &[EqBand::bell(1, 1_000.0, 9.0, 1.0)]));
        let mut buf = sine(1_000.0, 48_000.0, 32_768);
        let before = rms(&buf);
        eq.process(&mut buf);
        let after = rms(&buf[buf.len() / 2..]);
        let gain_db = 20.0 * (after / before).log10();
        assert!(
            (gain_db - 9.0).abs() < 0.4,
            "expected ~+9 dB, measured {gain_db}"
        );
    }

    #[test]
    fn sixteen_bands_all_apply() {
        let mut eq = StereoEq::new(48_000.0);
        let bands: Vec<EqBand> = (0..MAX_BANDS)
            .map(|i| EqBand::bell(i as u32, 1_000.0, 1.0, 0.7))
            .collect();
        eq.apply(&setting(true, &bands));
        assert_eq!(eq.band_count(), MAX_BANDS);
        let mut buf = sine(1_000.0, 48_000.0, 65_536);
        let before = rms(&buf);
        eq.process(&mut buf);
        let after = rms(&buf[buf.len() / 2..]);
        let gain_db = 20.0 * (after / before).log10();
        // 16 x +1 dB bells stacked at the same frequency.
        assert!((gain_db - 16.0).abs() < 0.6, "measured {gain_db}");
    }

    #[test]
    fn config_beyond_max_bands_is_truncated() {
        let cfg = EqConfig {
            enabled: true,
            bands: (0..MAX_BANDS + 8)
                .map(|i| EqBand::bell(i as u32, 1_000.0, 1.0, 1.0))
                .collect(),
        };
        let s = EqSetting::from_config(&cfg);
        assert_eq!(s.len(), MAX_BANDS);
    }

    #[test]
    fn slopes_are_cascaded() {
        // One octave below a 1 kHz high-pass: 12 dB/oct should lose ~12 dB,
        // 24 ~24 dB and 48 ~48 dB.
        for (slope, expect) in [(12u8, 12.0f64), (24, 24.0), (48, 48.0)] {
            let cfg = EqConfig {
                enabled: true,
                bands: vec![EqBand {
                    kind: FilterKind::HighPass,
                    slope_db_oct: slope,
                    ..EqBand::bell(1, 1_000.0, 0.0, 0.707)
                }],
            };
            let mut curve = Vec::new();
            curve_db(&cfg, 48_000.0, &[500.0], &mut curve);
            assert!(
                (curve[0] as f64 + expect).abs() < 1.5,
                "{slope} dB/oct gave {} dB at half the corner",
                curve[0]
            );
        }
    }

    #[test]
    fn curve_matches_processing() {
        let cfg = EqConfig {
            enabled: true,
            bands: vec![EqBand::new(2, FilterKind::Bell, 500.0, -6.0, 1.2)],
        };
        let mut eq = StereoEq::new(48_000.0);
        eq.apply(&EqSetting::from_config(&cfg));

        let mut curve = Vec::new();
        curve_db(&cfg, 48_000.0, &[500.0], &mut curve);

        let mut buf = sine(500.0, 48_000.0, 32_768);
        let before = rms(&buf);
        eq.process(&mut buf);
        let after = rms(&buf[buf.len() / 2..]);
        let measured = 20.0 * (after / before).log10();
        assert!(
            (curve[0] - measured).abs() < 0.4,
            "curve {} vs measured {measured}",
            curve[0]
        );
    }

    #[test]
    fn adding_and_removing_bands_does_not_click() {
        let mut eq = StereoEq::new(48_000.0);
        let mut bands = vec![EqBand::bell(1, 200.0, 6.0, 1.0)];
        eq.apply(&setting(true, &bands));
        // One continuous sine, processed in blocks: a discontinuity in the
        // *output* is then unambiguously the EQ's fault.
        let source = sine(200.0, 48_000.0, 512 * 40);
        let mut prev = source[0];
        let mut worst = 0.0f32;
        for step in 0..40 {
            if step % 4 == 0 {
                if bands.len() < 4 {
                    bands.push(EqBand::bell(step as u32 + 2, 800.0, -4.0, 2.0));
                } else {
                    bands.pop();
                }
                eq.apply(&setting(true, &bands));
            }
            let mut buf = source[step * 1_024..(step + 1) * 1_024].to_vec();
            eq.process(&mut buf);
            for s in buf.iter().step_by(2) {
                worst = worst.max((s - prev).abs());
                prev = *s;
            }
        }
        // A 200 Hz sine at 48 kHz moves at most ~0.027 per sample; a click from
        // a reset filter would be an order of magnitude larger.
        assert!(worst < 0.1, "discontinuity of {worst} between samples");
    }

    #[test]
    fn survives_parameter_storms() {
        let mut eq = StereoEq::new(44_100.0);
        let mut buf = sine(220.0, 44_100.0, 4_096);
        for step in 0..200 {
            let f = 20.0 + (step as f32 * 97.0) % 19_000.0;
            let n = 1 + step % MAX_BANDS;
            let bands: Vec<EqBand> = (0..n)
                .map(|i| {
                    EqBand::new(
                        i as u32,
                        [FilterKind::Bell, FilterKind::HighPass, FilterKind::Notch][i % 3],
                        f,
                        ((step % 13) as f32) - 6.0,
                        0.3,
                    )
                })
                .collect();
            eq.apply(&setting(true, &bands));
            eq.process(&mut buf);
            assert!(buf.iter().all(|s| s.is_finite()), "NaN at step {step}");
        }
    }

    #[test]
    fn audition_is_transparent_when_off() {
        let mut a = AuditionFilter::new(48_000.0);
        let original = sine(1_000.0, 48_000.0, 1_024);
        let mut buf = original.clone();
        a.process(&mut buf);
        assert_eq!(buf, original);
        a.set(Some(1_000.0), 6.0);
        a.set(None, 6.0);
        // After the fade-out completes it must go bit-transparent again.
        let mut warm = sine(1_000.0, 48_000.0, 4_096);
        a.process(&mut warm);
        let mut buf = original.clone();
        a.process(&mut buf);
        assert_eq!(buf, original);
    }

    #[test]
    fn audition_passes_its_band_and_rejects_others() {
        let mut a = AuditionFilter::new(48_000.0);
        a.set(Some(1_000.0), 6.0);
        let mut inside = sine(1_000.0, 48_000.0, 32_768);
        let before = rms(&inside);
        a.process(&mut inside);
        let pass = rms(&inside[inside.len() / 2..]);
        assert!(
            20.0 * (pass / before).log10() > -1.0,
            "band centre should pass"
        );

        let mut a = AuditionFilter::new(48_000.0);
        a.set(Some(1_000.0), 6.0);
        let mut outside = sine(100.0, 48_000.0, 32_768);
        let before = rms(&outside);
        a.process(&mut outside);
        let stop = rms(&outside[outside.len() / 2..]);
        assert!(
            20.0 * (stop / before).log10() < -20.0,
            "two octaves away should be rejected"
        );
    }

    #[test]
    fn audition_sweep_is_zipper_free() {
        let mut a = AuditionFilter::new(48_000.0);
        a.set(Some(300.0), 5.0);
        let source = sine(300.0, 48_000.0, 256 * 200);
        let mut prev = source[0];
        let mut worst = 0.0f32;
        for step in 0..200 {
            // Cursor sweeping across the graph, one event per block.
            a.set(Some(300.0 + step as f32 * 40.0), 5.0);
            let mut buf = source[step * 512..(step + 1) * 512].to_vec();
            a.process(&mut buf);
            for s in buf.iter().step_by(2) {
                worst = worst.max((s - prev).abs());
                prev = *s;
            }
        }
        assert!(worst < 0.1, "zipper of {worst}");
    }

    /// SPEC §12: a cursor-following solo sweep sends one `set` per pointer
    /// event, so neither `set` nor `process` may touch the allocator.
    #[test]
    fn audition_sweep_is_allocation_free() {
        let mut a = AuditionFilter::new(48_000.0);
        let mut buf = sine(300.0, 48_000.0, 512);
        // Warm up: the first block may still touch lazily initialised statics.
        a.set(Some(300.0), 5.0);
        a.process(&mut buf);

        let (_, hits) = crate::test_alloc::count_allocations(|| {
            for step in 0..256 {
                // Continuous sweep in both frequency and Q, plus the on/off
                // transitions that hold-to-solo produces.
                a.set(Some(200.0 + step as f32 * 70.0), 2.0 + (step % 22) as f32);
                a.process(&mut buf);
                if step % 32 == 31 {
                    a.set(None, 4.0);
                    a.process(&mut buf);
                }
            }
        });
        assert_eq!(hits, 0, "the audition filter allocated {hits} times");
    }

    /// SPEC §12: the EQ itself must be allocation-free across band adds,
    /// removes *and* retypes - the cases that tempt an implementation into a
    /// `Vec` of filter sections.
    #[test]
    fn eq_band_edits_are_allocation_free() {
        let mut eq = StereoEq::new(48_000.0);
        let mut buf = sine(500.0, 48_000.0, 512);
        eq.apply(&setting(true, &[EqBand::bell(1, 500.0, 3.0, 1.0)]));
        eq.process(&mut buf);

        // Build every setting up front: the *test* may allocate, `apply` and
        // `process` may not.
        let kinds = [
            FilterKind::Bell,
            FilterKind::HighPass,
            FilterKind::LowPass,
            FilterKind::Notch,
            FilterKind::HighShelf,
        ];
        let settings: Vec<EqSetting> = (0..96)
            .map(|i: usize| {
                let bands: Vec<EqBand> = (0..(i % (MAX_BANDS + 1)))
                    .map(|k| EqBand {
                        kind: kinds[(i + k) % kinds.len()],
                        slope_db_oct: [12u8, 24, 48][(i + k) % 3],
                        ..EqBand::new(k as u32, FilterKind::Bell, 100.0 * (k + 1) as f32, 2.0, 1.0)
                    })
                    .collect();
                setting(true, &bands)
            })
            .collect();

        let (_, hits) = crate::test_alloc::count_allocations(|| {
            for s in settings.iter() {
                eq.apply(s);
                eq.process(&mut buf);
            }
        });
        assert_eq!(hits, 0, "the EQ allocated {hits} times while editing bands");
    }

    /// Changing a band's *kind* is a topology change, and topology changes are
    /// exactly what clicks if the cascade is swapped in without a crossfade.
    #[test]
    fn retyping_a_band_does_not_click() {
        let mut eq = StereoEq::new(48_000.0);
        let kinds = [
            FilterKind::Bell,
            FilterKind::LowShelf,
            FilterKind::HighPass,
            FilterKind::Notch,
            FilterKind::BandPass,
            FilterKind::HighShelf,
            FilterKind::LowPass,
        ];
        eq.apply(&setting(true, &[EqBand::new(1, kinds[0], 200.0, 6.0, 1.0)]));
        let source = sine(200.0, 48_000.0, 512 * 56);
        // Settle first, so the measurement starts from a steady state. This must
        // consume exactly the blocks the loop below skips (steps 0 and 1),
        // otherwise the *input* jumps and we measure our own splice.
        let mut warm = source[..2 * 1_024].to_vec();
        eq.process(&mut warm);

        let mut prev = warm[warm.len() - 2];
        let mut worst = 0.0f32;
        for step in 2..56 {
            if step % 4 == 0 {
                // Same frequency and Q, only the shape changes: any output jump
                // is the topology swap, not a parameter move.
                let kind = kinds[(step / 4) % kinds.len()];
                eq.apply(&setting(true, &[EqBand::new(1, kind, 200.0, 6.0, 1.0)]));
            }
            let mut buf = source[step * 1_024..(step + 1) * 1_024].to_vec();
            eq.process(&mut buf);
            for s in buf.iter().step_by(2) {
                worst = worst.max((s - prev).abs());
                prev = *s;
            }
        }
        // A +6 dB-boosted 200 Hz sine at 48 kHz moves at most
        // 2 * 2*pi*200/48000 = 0.0524 per sample, and that is exactly what this
        // measures (0.0523) - i.e. the 8 ms topology crossfade contributes
        // nothing measurable. Splicing a fresh cascade in instead measures ~0.29.
        assert!(worst < 0.07, "retype produced a step of {worst}");
    }

    /// SPEC §12: the cascaded slopes must be real, measured through the
    /// filters rather than only agreeing with our own analytic curve. One
    /// octave below a high-pass corner, 12/24/48 dB/oct must lose 12/24/48 dB;
    /// one octave above a low-pass corner, the same.
    #[test]
    fn cascaded_slopes_are_measured_an_octave_out() {
        for (kind, corner, probe) in [
            (FilterKind::HighPass, 1_000.0f32, 500.0f32),
            (FilterKind::LowPass, 1_000.0, 2_000.0),
        ] {
            for (slope, expect) in [(12u8, 12.0f32), (24, 24.0), (48, 48.0)] {
                let mut eq = StereoEq::new(48_000.0);
                eq.apply(&setting(
                    true,
                    &[EqBand {
                        kind,
                        slope_db_oct: slope,
                        ..EqBand::new(1, kind, corner, 0.0, 0.707)
                    }],
                ));
                // Long enough that the topology crossfade and the filter
                // transient are both far behind the measurement window.
                let mut buf = sine(probe, 48_000.0, 1 << 17);
                let before = rms(&buf);
                eq.process(&mut buf);
                let after = rms(&buf[buf.len() / 2..]);
                let measured = -20.0 * (after / before).log10();
                assert!(
                    (measured - expect).abs() < 1.5,
                    "{kind:?} {slope} dB/oct measured {measured} dB at {probe} Hz \
                     (expected ~{expect})"
                );
            }
        }
    }
}
