//! ITU-R BS.1770-4 / EBU R 128 loudness measurement.
//!
//! Provides momentary (400 ms), short-term (3 s), gated integrated loudness and
//! loudness range (LRA), all measured on the real output bus.
//!
//! The K-weighting filter is designed analytically rather than table-driven so
//! that 44.1 / 48 / 88.2 / 96 / 176.4 / 192 kHz all measure identically. At
//! 48 kHz the design reproduces the coefficient table printed in the
//! recommendation to 1e-9 (see the tests at the bottom of this file).
//!
//! Compliance is pinned by `tests/loudness_ebu.rs`, which synthesises the
//! EBU Tech 3341 Table 1 signals (tests 1-5 and 9-14) and the EBU Tech 3342
//! Table 1 signals (tests 1-4) and checks them against the published expected
//! values and tolerances, at 44.1, 48 and 96 kHz. Tech 3341 tests 7-8 and
//! Tech 3342 tests 5-6 use proprietary programme material that the EBU only
//! distributes as WAV files and are *not* covered. Tech 3341 test 6 is a 5.0
//! signal: Onyx measures the first two channels of any file, so surround
//! weighting (BS.1770-4 Table 3, G_Ls = G_Rs = 1.41) never comes into play and
//! that test cannot be satisfied.

use super::biquad::{Biquad, Coeffs};
use crate::LUFS_SILENCE;
use std::collections::VecDeque;

/// The `-0.691` calibration constant from BS.1770.
const LUFS_OFFSET: f64 = -0.691;

/// Sub-block length: the quantum the 400 ms / 3 s windows are built from, and
/// therefore the granularity at which the momentary and short-term read-outs
/// can be positioned in time.
///
/// 10 ms rather than a more obvious 50 or 100 ms because EBU Tech 3341 Table 1
/// tests 13 and 14 place a 400 ms burst at multiples of 20 ms and demand
/// `max M = -23.0 +/- 0.1 LUFS`. A window that can only start on a `T` grid
/// misses by up to `T/2`, i.e. `10*log10(1 - T/800)` LU: 0.27 LU at 50 ms
/// (which failed), 0.054 LU at 10 ms. Every rate Onyx supports gives an
/// integral number of samples per 10 ms.
const SUB_BLOCK_SECS: f64 = 0.01;
/// Sub-blocks per 400 ms momentary window.
const MOMENTARY_SUBS: usize = 40;
/// Sub-blocks per 3 s short-term window.
const SHORT_SUBS: usize = 300;
/// Gating hop: 100 ms == 10 sub-blocks (75 % overlap of 400 ms blocks).
const GATE_HOP_SUBS: usize = 10;
/// LRA hop, also 100 ms. EBU Tech 3342 s3.1 requires "a minimum block overlap
/// of 2.9 s between consecutive analysis windows (i.e. >= 10 Hz sampling of the
/// loudness level)"; a 3 s window stepped by 100 ms is exactly 10 Hz.
const LRA_HOP_SUBS: usize = 10;
/// Absolute gate.
const ABSOLUTE_GATE_LUFS: f64 = -70.0;
/// Relative gate offset for the integrated measurement.
const RELATIVE_GATE_LU: f64 = -10.0;
/// Relative gate offset for LRA.
const LRA_GATE_LU: f64 = -20.0;

/// The two-stage K-weighting pre-filter for one channel.
#[derive(Clone, Copy)]
pub struct KWeighting {
    shelf: Biquad,
    highpass: Biquad,
}

impl KWeighting {
    pub fn new(sample_rate: f64) -> Self {
        let (shelf, highpass) = k_weighting_coeffs(sample_rate);
        Self {
            shelf: Biquad::new(shelf),
            highpass: Biquad::new(highpass),
        }
    }

    #[inline(always)]
    pub fn process(&mut self, x: f64) -> f64 {
        self.highpass.process(self.shelf.process(x))
    }

    pub fn reset(&mut self) {
        self.shelf.reset();
        self.highpass.reset();
    }
}

/// Analytic K-weighting design: `(stage 1 high shelf, stage 2 RLB high pass)`.
pub fn k_weighting_coeffs(sample_rate: f64) -> (Coeffs, Coeffs) {
    let fs = sample_rate.max(8_000.0);

    // ---- Stage 1: +4 dB high shelf, unity at LF ----
    let f0 = 1_681.974_450_955_533_f64;
    let g = 3.999_843_853_973_347_f64;
    let q = 0.707_175_236_955_419_6_f64;
    let k = (std::f64::consts::PI * f0 / fs).tan();
    let vh = 10f64.powf(g / 20.0);
    let vb = vh.powf(0.499_666_774_154_541_6);
    let k2 = k * k;
    let a0 = 1.0 + k / q + k2;
    let shelf = Coeffs {
        b0: (vh + vb * k / q + k2) / a0,
        b1: 2.0 * (k2 - vh) / a0,
        b2: (vh - vb * k / q + k2) / a0,
        a1: 2.0 * (k2 - 1.0) / a0,
        a2: (1.0 - k / q + k2) / a0,
    };

    // ---- Stage 2: RLB high pass at ~38 Hz ----
    let f0 = 38.135_470_876_024_44_f64;
    let q = 0.500_327_037_323_877_3_f64;
    let k = (std::f64::consts::PI * f0 / fs).tan();
    let k2 = k * k;
    let den = 1.0 + k / q + k2;
    let highpass = Coeffs {
        b0: 1.0,
        b1: -2.0,
        b2: 1.0,
        a1: 2.0 * (k2 - 1.0) / den,
        a2: (1.0 - k / q + k2) / den,
    };

    (shelf, highpass)
}

/// Streaming loudness meter for a stereo bus.
pub struct LoudnessMeter {
    sample_rate: f64,
    filters: [KWeighting; 2],
    sub_len: usize,
    sub_pos: usize,
    sub_accum: f64,
    /// Mean square (K-weighted, channel-summed) per finished sub-block.
    history: VecDeque<f64>,
    subs_seen: u64,
    /// Powers of the 400 ms gating blocks above the absolute gate.
    gate_powers: Vec<f64>,
    /// Powers of the 3 s short-term blocks above the absolute gate (for LRA).
    lra_powers: Vec<f64>,
    cached_integrated: f32,
    cached_lra: f32,
    lra_dirty: bool,
    integrated_dirty: bool,
}

impl LoudnessMeter {
    pub fn new(sample_rate: f64) -> Self {
        let mut m = LoudnessMeter {
            sample_rate: sample_rate.max(8_000.0),
            filters: [KWeighting::new(sample_rate), KWeighting::new(sample_rate)],
            sub_len: 1,
            sub_pos: 0,
            sub_accum: 0.0,
            history: VecDeque::with_capacity(SHORT_SUBS + 1),
            subs_seen: 0,
            gate_powers: Vec::new(),
            lra_powers: Vec::new(),
            cached_integrated: LUFS_SILENCE,
            cached_lra: 0.0,
            lra_dirty: false,
            integrated_dirty: false,
        };
        m.set_sample_rate(sample_rate);
        m
    }

    pub fn set_sample_rate(&mut self, sample_rate: f64) {
        self.sample_rate = sample_rate.max(8_000.0);
        self.sub_len = ((self.sample_rate * SUB_BLOCK_SECS).round() as usize).max(1);
        self.filters = [
            KWeighting::new(self.sample_rate),
            KWeighting::new(self.sample_rate),
        ];
        self.reset();
    }

    pub fn reset(&mut self) {
        for f in self.filters.iter_mut() {
            f.reset();
        }
        self.sub_pos = 0;
        self.sub_accum = 0.0;
        self.history.clear();
        self.subs_seen = 0;
        self.gate_powers.clear();
        self.lra_powers.clear();
        self.cached_integrated = LUFS_SILENCE;
        self.cached_lra = 0.0;
        self.lra_dirty = false;
        self.integrated_dirty = false;
    }

    /// Feed an interleaved stereo block.
    pub fn process(&mut self, buf: &[f32]) {
        let frames = buf.len() / 2;
        for f in 0..frames {
            let l = self.filters[0].process(buf[f * 2] as f64);
            let r = self.filters[1].process(buf[f * 2 + 1] as f64);
            // G_L = G_R = 1.0 for the front pair.
            self.sub_accum += l * l + r * r;
            self.sub_pos += 1;
            if self.sub_pos >= self.sub_len {
                let mean_square = self.sub_accum / self.sub_len as f64;
                self.push_sub_block(mean_square);
                self.sub_accum = 0.0;
                self.sub_pos = 0;
            }
        }
    }

    fn push_sub_block(&mut self, mean_square: f64) {
        if self.history.len() == SHORT_SUBS {
            self.history.pop_front();
        }
        self.history.push_back(mean_square);
        self.subs_seen += 1;

        // 400 ms gating block every 100 ms.
        if self.history.len() >= MOMENTARY_SUBS
            && self.subs_seen.is_multiple_of(GATE_HOP_SUBS as u64)
        {
            let p = self.window_power(MOMENTARY_SUBS);
            if power_to_lufs(p) > ABSOLUTE_GATE_LUFS {
                self.gate_powers.push(p);
                self.integrated_dirty = true;
            }
        }

        // 3 s short-term block every 100 ms, used for LRA. EBU Tech 3342 s5
        // gates with `>=`, unlike the integrated measurement.
        if self.history.len() >= SHORT_SUBS && self.subs_seen.is_multiple_of(LRA_HOP_SUBS as u64) {
            let p = self.window_power(SHORT_SUBS);
            if power_to_lufs(p) >= ABSOLUTE_GATE_LUFS {
                self.lra_powers.push(p);
                self.lra_dirty = true;
            }
        }
    }

    /// Mean square across the most recent `n` sub-blocks.
    #[inline]
    fn window_power(&self, n: usize) -> f64 {
        let len = self.history.len();
        if len == 0 {
            return 0.0;
        }
        let n = n.min(len);
        let mut sum = 0.0;
        for i in (len - n)..len {
            sum += self.history[i];
        }
        sum / n as f64
    }

    /// Momentary loudness (400 ms). `-70` until the window is full.
    pub fn momentary(&self) -> f32 {
        if self.history.len() < MOMENTARY_SUBS {
            return LUFS_SILENCE;
        }
        clamp_lufs(power_to_lufs(self.window_power(MOMENTARY_SUBS)))
    }

    /// Short-term loudness (3 s). `-70` until the window is full.
    pub fn short_term(&self) -> f32 {
        if self.history.len() < SHORT_SUBS {
            return LUFS_SILENCE;
        }
        clamp_lufs(power_to_lufs(self.window_power(SHORT_SUBS)))
    }

    /// Gated integrated loudness over everything measured since the last reset.
    pub fn integrated(&mut self) -> f32 {
        if self.integrated_dirty {
            self.cached_integrated =
                clamp_lufs(gated_loudness(&self.gate_powers, RELATIVE_GATE_LU));
            self.integrated_dirty = false;
        }
        self.cached_integrated
    }

    /// Loudness range in LU (EBU Tech 3342).
    pub fn lra(&mut self) -> f32 {
        if self.lra_dirty {
            self.cached_lra = compute_lra(&self.lra_powers);
            self.lra_dirty = false;
        }
        self.cached_lra
    }

    /// Total audio time measured, in seconds.
    pub fn measured_secs(&self) -> f64 {
        self.subs_seen as f64 * SUB_BLOCK_SECS
    }
}

#[inline]
fn power_to_lufs(power: f64) -> f64 {
    if power <= 1e-15 {
        -200.0
    } else {
        LUFS_OFFSET + 10.0 * power.log10()
    }
}

#[inline]
fn clamp_lufs(v: f64) -> f32 {
    if !v.is_finite() || v < LUFS_SILENCE as f64 {
        LUFS_SILENCE
    } else {
        v as f32
    }
}

/// Two-pass gating from BS.1770-4 §5.
fn gated_loudness(powers: &[f64], relative_gate_lu: f64) -> f64 {
    if powers.is_empty() {
        return LUFS_SILENCE as f64;
    }
    // Blocks are already above the absolute gate.
    let mean: f64 = powers.iter().sum::<f64>() / powers.len() as f64;
    let threshold = power_to_lufs(mean) + relative_gate_lu;
    let mut sum = 0.0;
    let mut n = 0usize;
    for &p in powers {
        if power_to_lufs(p) > threshold {
            sum += p;
            n += 1;
        }
    }
    if n == 0 {
        return LUFS_SILENCE as f64;
    }
    power_to_lufs(sum / n as f64)
}

/// LRA = 95th percentile - 10th percentile of the gated short-term loudness
/// distribution (EBU Tech 3342 s3.1 and the MATLAB listing in s5).
fn compute_lra(powers: &[f64]) -> f32 {
    if powers.len() < 2 {
        return 0.0;
    }
    let mean: f64 = powers.iter().sum::<f64>() / powers.len() as f64;
    let threshold = power_to_lufs(mean) + LRA_GATE_LU;
    let mut values: Vec<f64> = powers
        .iter()
        .map(|&p| power_to_lufs(p))
        .filter(|&l| l >= threshold)
        .collect();
    if values.len() < 2 {
        return 0.0;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let low = percentile(&values, 0.10);
    let high = percentile(&values, 0.95);
    ((high - low).max(0.0)) as f32
}

/// Percentile exactly as Tech 3342 s5 defines it:
/// `stl_sorted_vec(round((n-1) * PRC / 100 + 1))` in 1-based MATLAB, i.e.
/// `sorted[round((n-1) * p)]` here. No interpolation between neighbours.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    // MATLAB `round` is half-away-from-zero, which is what f64::round does.
    let idx = (p * (sorted.len() - 1) as f64).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    fn stereo_sine(freq: f64, amp: f64, sr: f64, secs: f64) -> Vec<f32> {
        let n = (sr * secs) as usize;
        (0..n)
            .flat_map(|i| {
                let s = (amp * (2.0 * std::f64::consts::PI * freq * i as f64 / sr).sin()) as f32;
                [s, s]
            })
            .collect()
    }

    /// The recommendation prints the 48 kHz coefficients; our analytic design
    /// must reproduce them, otherwise every other sample rate is suspect too.
    #[test]
    fn k_weighting_matches_bs1770_table_at_48k() {
        let (shelf, hp) = k_weighting_coeffs(48_000.0);
        assert_abs_diff_eq!(shelf.b0, 1.535_124_859_586_97, epsilon = 1e-9);
        assert_abs_diff_eq!(shelf.b1, -2.691_696_189_406_38, epsilon = 1e-9);
        assert_abs_diff_eq!(shelf.b2, 1.198_392_810_852_85, epsilon = 1e-9);
        assert_abs_diff_eq!(shelf.a1, -1.690_659_293_182_41, epsilon = 1e-9);
        assert_abs_diff_eq!(shelf.a2, 0.732_480_774_215_85, epsilon = 1e-9);

        assert_abs_diff_eq!(hp.b0, 1.0, epsilon = 1e-12);
        assert_abs_diff_eq!(hp.b1, -2.0, epsilon = 1e-12);
        assert_abs_diff_eq!(hp.b2, 1.0, epsilon = 1e-12);
        assert_abs_diff_eq!(hp.a1, -1.990_047_454_833_98, epsilon = 1e-8);
        assert_abs_diff_eq!(hp.a2, 0.990_072_250_366_21, epsilon = 1e-8);
    }

    #[test]
    fn shelf_is_unity_at_low_frequency_and_plus_four_at_top() {
        let (shelf, _) = k_weighting_coeffs(48_000.0);
        assert_abs_diff_eq!(shelf.magnitude_db(48_000.0, 100.0), 0.0, epsilon = 0.05);
        assert_abs_diff_eq!(shelf.magnitude_db(48_000.0, 18_000.0), 4.0, epsilon = 0.15);
    }

    #[test]
    fn same_signal_measures_the_same_at_every_sample_rate() {
        let mut readings = Vec::new();
        for sr in [44_100.0, 48_000.0, 88_200.0, 96_000.0, 192_000.0] {
            let mut m = LoudnessMeter::new(sr);
            m.process(&stereo_sine(1_000.0, 0.5, sr, 5.0));
            readings.push(m.integrated());
        }
        let first = readings[0];
        for r in &readings {
            assert_abs_diff_eq!(*r, first, epsilon = 0.05);
        }
    }

    #[test]
    fn six_db_louder_reads_six_lu_louder() {
        let sr = 48_000.0;
        let mut a = LoudnessMeter::new(sr);
        a.process(&stereo_sine(1_000.0, 0.25, sr, 5.0));
        let mut b = LoudnessMeter::new(sr);
        b.process(&stereo_sine(1_000.0, 0.5, sr, 5.0));
        assert_abs_diff_eq!(b.integrated() - a.integrated(), 6.0206, epsilon = 0.05);
    }

    #[test]
    fn stereo_is_three_lu_louder_than_the_same_signal_on_one_leg() {
        let sr = 48_000.0;
        let stereo = stereo_sine(1_000.0, 0.5, sr, 5.0);
        let mut left_only = stereo.clone();
        for f in 0..left_only.len() / 2 {
            left_only[f * 2 + 1] = 0.0;
        }
        let mut a = LoudnessMeter::new(sr);
        a.process(&stereo);
        let mut b = LoudnessMeter::new(sr);
        b.process(&left_only);
        assert_abs_diff_eq!(a.integrated() - b.integrated(), 3.0103, epsilon = 0.05);
    }

    #[test]
    fn silence_is_gated_out_of_the_integrated_value() {
        let sr = 48_000.0;
        let mut with_silence = LoudnessMeter::new(sr);
        with_silence.process(&stereo_sine(1_000.0, 0.5, sr, 5.0));
        with_silence.process(&vec![0.0f32; (sr as usize) * 2 * 20]);
        let mut without = LoudnessMeter::new(sr);
        without.process(&stereo_sine(1_000.0, 0.5, sr, 5.0));
        assert_abs_diff_eq!(
            with_silence.integrated(),
            without.integrated(),
            epsilon = 0.2
        );
    }

    #[test]
    fn momentary_and_short_term_need_their_windows() {
        let sr = 48_000.0;
        let mut m = LoudnessMeter::new(sr);
        m.process(&stereo_sine(1_000.0, 0.5, sr, 0.2));
        assert_eq!(m.short_term(), LUFS_SILENCE);
        m.process(&stereo_sine(1_000.0, 0.5, sr, 0.3));
        assert!(m.momentary() > -20.0);
        m.process(&stereo_sine(1_000.0, 0.5, sr, 3.0));
        assert!(m.short_term() > -20.0);
        assert_abs_diff_eq!(m.momentary(), m.short_term(), epsilon = 0.05);
    }

    #[test]
    fn lra_reflects_dynamic_range() {
        let sr = 48_000.0;
        let mut quiet_then_loud = LoudnessMeter::new(sr);
        for _ in 0..4 {
            quiet_then_loud.process(&stereo_sine(1_000.0, 0.05, sr, 5.0));
            quiet_then_loud.process(&stereo_sine(1_000.0, 0.5, sr, 5.0));
        }
        let lra = quiet_then_loud.lra();
        assert!(lra > 12.0 && lra < 22.0, "LRA was {lra}");

        let mut steady = LoudnessMeter::new(sr);
        steady.process(&stereo_sine(1_000.0, 0.5, sr, 40.0));
        assert!(steady.lra() < 1.0, "steady LRA was {}", steady.lra());
    }

    #[test]
    fn digital_silence_reports_the_floor() {
        let mut m = LoudnessMeter::new(48_000.0);
        m.process(&vec![0.0f32; 48_000 * 2 * 5]);
        assert_eq!(m.momentary(), LUFS_SILENCE);
        assert_eq!(m.integrated(), LUFS_SILENCE);
        assert_eq!(m.lra(), 0.0);
    }
}
