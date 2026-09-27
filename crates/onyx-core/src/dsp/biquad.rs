//! Direct-form-II transposed biquads with Robert Bristow-Johnson designs.
//!
//! Coefficients are computed in `f64` and the filter state is `f64` as well:
//! at 192 kHz with a high-Q 20 Hz filter, `f32` state audibly quantises.

use crate::types::FilterKind;
use std::f64::consts::PI;

/// Normalised biquad coefficients (`a0` divided out).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coeffs {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

impl Default for Coeffs {
    fn default() -> Self {
        Coeffs::bypass()
    }
}

impl Coeffs {
    /// Unity gain, no phase shift.
    pub const fn bypass() -> Self {
        Coeffs {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
        }
    }

    /// Design a filter. `freq` is clamped to a sane range for `sample_rate` so
    /// that a UI drag can never produce an unstable filter.
    pub fn design(kind: FilterKind, sample_rate: f64, freq: f64, q: f64, gain_db: f64) -> Self {
        if sample_rate <= 0.0 {
            return Coeffs::bypass();
        }
        // Keep at least a hair below Nyquist; RBJ formulas blow up at exactly fs/2.
        let nyq = sample_rate * 0.5;
        let f0 = freq.clamp(1.0, nyq * 0.995);
        let q = q.clamp(0.05, 40.0);
        let w0 = 2.0 * PI * f0 / sample_rate;
        let (sin_w0, cos_w0) = w0.sin_cos();
        let alpha = sin_w0 / (2.0 * q);

        match kind {
            FilterKind::Bell => {
                let a = 10f64.powf(gain_db / 40.0);
                let b0 = 1.0 + alpha * a;
                let b1 = -2.0 * cos_w0;
                let b2 = 1.0 - alpha * a;
                let a0 = 1.0 + alpha / a;
                let a1 = -2.0 * cos_w0;
                let a2 = 1.0 - alpha / a;
                Coeffs::normalise(b0, b1, b2, a0, a1, a2)
            }
            FilterKind::LowShelf => {
                let a = 10f64.powf(gain_db / 40.0);
                // Cookbook shelving term `2*sqrt(A)*alpha`, i.e. the *Q* form of
                // the shelf, so `q` is the RBJ shelf Q over the whole 0.1..40
                // range the band model advertises (SPEC §12). `q = 1/sqrt(2)`
                // is the steepest monotonic shelf; above that it resonates.
                let beta = 2.0 * a.sqrt() * alpha;
                let ap1 = a + 1.0;
                let am1 = a - 1.0;
                let b0 = a * (ap1 - am1 * cos_w0 + beta);
                let b1 = 2.0 * a * (am1 - ap1 * cos_w0);
                let b2 = a * (ap1 - am1 * cos_w0 - beta);
                let a0 = ap1 + am1 * cos_w0 + beta;
                let a1 = -2.0 * (am1 + ap1 * cos_w0);
                let a2 = ap1 + am1 * cos_w0 - beta;
                Coeffs::normalise(b0, b1, b2, a0, a1, a2)
            }
            FilterKind::HighShelf => {
                let a = 10f64.powf(gain_db / 40.0);
                let beta = 2.0 * a.sqrt() * alpha;
                let ap1 = a + 1.0;
                let am1 = a - 1.0;
                let b0 = a * (ap1 + am1 * cos_w0 + beta);
                let b1 = -2.0 * a * (am1 + ap1 * cos_w0);
                let b2 = a * (ap1 + am1 * cos_w0 - beta);
                let a0 = ap1 - am1 * cos_w0 + beta;
                let a1 = 2.0 * (am1 - ap1 * cos_w0);
                let a2 = ap1 - am1 * cos_w0 - beta;
                Coeffs::normalise(b0, b1, b2, a0, a1, a2)
            }
            FilterKind::LowPass => {
                let b0 = (1.0 - cos_w0) / 2.0;
                let b1 = 1.0 - cos_w0;
                let b2 = (1.0 - cos_w0) / 2.0;
                let a0 = 1.0 + alpha;
                let a1 = -2.0 * cos_w0;
                let a2 = 1.0 - alpha;
                Coeffs::normalise(b0, b1, b2, a0, a1, a2)
            }
            FilterKind::HighPass => {
                let b0 = (1.0 + cos_w0) / 2.0;
                let b1 = -(1.0 + cos_w0);
                let b2 = (1.0 + cos_w0) / 2.0;
                let a0 = 1.0 + alpha;
                let a1 = -2.0 * cos_w0;
                let a2 = 1.0 - alpha;
                Coeffs::normalise(b0, b1, b2, a0, a1, a2)
            }
            FilterKind::Notch => {
                let b0 = 1.0;
                let b1 = -2.0 * cos_w0;
                let b2 = 1.0;
                let a0 = 1.0 + alpha;
                let a1 = -2.0 * cos_w0;
                let a2 = 1.0 - alpha;
                Coeffs::normalise(b0, b1, b2, a0, a1, a2)
            }
            FilterKind::BandPass => {
                // Constant 0 dB peak gain.
                let b0 = alpha;
                let b1 = 0.0;
                let b2 = -alpha;
                let a0 = 1.0 + alpha;
                let a1 = -2.0 * cos_w0;
                let a2 = 1.0 - alpha;
                Coeffs::normalise(b0, b1, b2, a0, a1, a2)
            }
        }
    }

    /// Q of section `k` of a `sections`-deep Butterworth cascade.
    ///
    /// A 2N-th order Butterworth factors into N biquads whose poles sit on the
    /// unit circle at angles `(2k+1)*pi/(4N)`; the corresponding section Q is
    /// `1 / (2 cos theta)`. This is what makes a 48 dB/oct high-pass sound like
    /// one filter instead of four stacked resonances.
    pub fn butterworth_q(sections: usize, k: usize) -> f64 {
        let n = (sections.max(1) * 2) as f64; // filter order
        let theta = PI * (2.0 * k as f64 + 1.0) / (2.0 * n);
        1.0 / (2.0 * theta.cos())
    }

    /// Design section `k` of a cascaded high-/low-pass of `sections` biquads.
    ///
    /// The sections carry Butterworth Qs, and the band's own `q` scales the
    /// *first* section by `q / (1/sqrt 2)` so that raising Q adds the corner
    /// resonance the user asked for at every slope instead of being silently
    /// ignored above 12 dB/oct. At the Butterworth value `q = 1/sqrt(2)` the
    /// scale factor is exactly 1, so a maximally-flat cascade is unaffected.
    ///
    /// This is the same expansion the front end draws the curve from
    /// (`src/lib/eq.ts::bandStages`); the two must not diverge, because SPEC
    /// §12 requires the drawn composite to be the real cascade. It is pinned by
    /// `tests/eq_rbj.rs::engine_cascade_matches_the_front_end_expansion`, and
    /// the *actual* TypeScript (not a transcription of it) is checked against
    /// this crate's curve by `tests/eq_curve_contract.rs` together with
    /// `scripts/check-eq-curve.mjs`.
    pub fn design_cascade_section(
        kind: FilterKind,
        sample_rate: f64,
        freq: f64,
        q: f64,
        gain_db: f64,
        sections: usize,
        k: usize,
    ) -> Self {
        if sections <= 1 || !kind.has_slope() {
            return Coeffs::design(kind, sample_rate, freq, q, gain_db);
        }
        let mut section_q = Coeffs::butterworth_q(sections, k);
        if k == 0 {
            section_q *= q / std::f64::consts::FRAC_1_SQRT_2;
        }
        Coeffs::design(kind, sample_rate, freq, section_q, 0.0)
    }

    #[inline]
    fn normalise(b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) -> Self {
        if a0.abs() < f64::EPSILON || !a0.is_finite() {
            return Coeffs::bypass();
        }
        let c = Coeffs {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        };
        if c.b0.is_finite()
            && c.b1.is_finite()
            && c.b2.is_finite()
            && c.a1.is_finite()
            && c.a2.is_finite()
        {
            c
        } else {
            Coeffs::bypass()
        }
    }

    /// Magnitude response in dB at `freq`. Used to draw the EQ curve, so it
    /// must agree exactly with what the audio path does.
    pub fn magnitude_db(&self, sample_rate: f64, freq: f64) -> f64 {
        let w = 2.0 * PI * freq / sample_rate;
        let (s1, c1) = w.sin_cos();
        let (s2, c2) = (2.0 * w).sin_cos();
        let num_re = self.b0 + self.b1 * c1 + self.b2 * c2;
        let num_im = -(self.b1 * s1 + self.b2 * s2);
        let den_re = 1.0 + self.a1 * c1 + self.a2 * c2;
        let den_im = -(self.a1 * s1 + self.a2 * s2);
        let num = (num_re * num_re + num_im * num_im).sqrt();
        let den = (den_re * den_re + den_im * den_im).sqrt();
        if den <= f64::MIN_POSITIVE || num <= 1e-12 {
            -120.0
        } else {
            20.0 * (num / den).log10()
        }
    }
}

/// A single biquad section, transposed direct form II.
#[derive(Clone, Copy, Debug, Default)]
pub struct Biquad {
    pub coeffs: Coeffs,
    z1: f64,
    z2: f64,
}

impl Biquad {
    pub fn new(coeffs: Coeffs) -> Self {
        Self {
            coeffs,
            z1: 0.0,
            z2: 0.0,
        }
    }

    #[inline]
    pub fn set_coeffs(&mut self, coeffs: Coeffs) {
        self.coeffs = coeffs;
    }

    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    #[inline(always)]
    pub fn process(&mut self, x: f64) -> f64 {
        let c = &self.coeffs;
        let y = c.b0 * x + self.z1;
        self.z1 = c.b1 * x - c.a1 * y + self.z2;
        self.z2 = c.b2 * x - c.a2 * y;
        y
    }

    /// Flush denormals / NaNs that can creep in after a long silent passage.
    #[inline]
    pub fn sanitise(&mut self) {
        if !self.z1.is_finite() || !self.z2.is_finite() {
            self.reset();
            return;
        }
        if self.z1.abs() < 1e-30 {
            self.z1 = 0.0;
        }
        if self.z2.abs() < 1e-30 {
            self.z2 = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    const FS: f64 = 48_000.0;

    /// Measured steady-state gain in dB.
    ///
    /// The amplitude is recovered from the RMS of the settled output
    /// (`A = sqrt(2) * rms` for a pure tone) rather than from the largest
    /// *sample*: at 6 kHz / 48 kHz there are only 8 samples per period, so the
    /// grid can miss the crest by up to 22.5 degrees and under-read a perfectly
    /// correct filter by 20*log10(cos 22.5) = -0.69 dB.
    fn measure_gain_db(mut bq: Biquad, freq: f64) -> f64 {
        let n = 48_000;
        let mut sum_sq = 0.0f64;
        let mut counted = 0u32;
        for i in 0..n {
            let x = (2.0 * PI * freq * i as f64 / FS).sin();
            let y = bq.process(x);
            if i > n / 2 {
                sum_sq += y * y;
                counted += 1;
            }
        }
        let amp = (2.0 * sum_sq / counted as f64).sqrt();
        20.0 * amp.log10()
    }

    #[test]
    fn bell_hits_requested_gain_at_centre() {
        let c = Coeffs::design(FilterKind::Bell, FS, 1000.0, 1.0, 6.0);
        assert_abs_diff_eq!(measure_gain_db(Biquad::new(c), 1000.0), 6.0, epsilon = 0.05);
        assert_abs_diff_eq!(c.magnitude_db(FS, 1000.0), 6.0, epsilon = 0.01);
    }

    #[test]
    fn bell_is_transparent_far_from_centre() {
        let c = Coeffs::design(FilterKind::Bell, FS, 1000.0, 2.0, 9.0);
        assert_abs_diff_eq!(c.magnitude_db(FS, 40.0), 0.0, epsilon = 0.1);
        assert_abs_diff_eq!(c.magnitude_db(FS, 18_000.0), 0.0, epsilon = 0.1);
    }

    #[test]
    fn analytic_curve_matches_measured_response() {
        for &(kind, f, q, g) in &[
            (FilterKind::Bell, 250.0, 0.7, -8.0),
            (FilterKind::LowShelf, 120.0, 0.707, 5.0),
            (FilterKind::HighShelf, 8_000.0, 0.707, -4.0),
            (FilterKind::LowPass, 5_000.0, 0.707, 0.0),
            (FilterKind::HighPass, 80.0, 0.707, 0.0),
        ] {
            let c = Coeffs::design(kind, FS, f, q, g);
            for &probe in &[100.0, 1_000.0, 6_000.0] {
                let analytic = c.magnitude_db(FS, probe);
                let measured = measure_gain_db(Biquad::new(c), probe);
                assert_abs_diff_eq!(analytic, measured, epsilon = 0.15);
            }
        }
    }

    #[test]
    fn highpass_rejects_dc_and_passes_top() {
        let c = Coeffs::design(FilterKind::HighPass, FS, 100.0, 0.707, 0.0);
        assert!(c.magnitude_db(FS, 10.0) < -30.0);
        assert_abs_diff_eq!(c.magnitude_db(FS, 10_000.0), 0.0, epsilon = 0.05);
    }

    #[test]
    fn shelves_reach_their_plateau() {
        let low = Coeffs::design(FilterKind::LowShelf, FS, 200.0, 0.707, 6.0);
        assert_abs_diff_eq!(low.magnitude_db(FS, 20.0), 6.0, epsilon = 0.3);
        assert_abs_diff_eq!(low.magnitude_db(FS, 15_000.0), 0.0, epsilon = 0.1);

        let high = Coeffs::design(FilterKind::HighShelf, FS, 4_000.0, 0.707, -6.0);
        assert_abs_diff_eq!(high.magnitude_db(FS, 20_000.0), -6.0, epsilon = 0.3);
        assert_abs_diff_eq!(high.magnitude_db(FS, 60.0), 0.0, epsilon = 0.1);
    }

    #[test]
    fn extreme_parameters_stay_stable() {
        for freq in [0.0, 1.0, 24_000.0, 96_000.0, f64::NAN] {
            let c = Coeffs::design(FilterKind::Bell, FS, freq, 0.0001, 48.0);
            let mut bq = Biquad::new(c);
            let mut out = 0.0;
            for i in 0..10_000 {
                out = bq.process(((i % 7) as f64 - 3.0) / 3.0);
            }
            assert!(out.is_finite(), "unstable at freq={freq}");
        }
    }
}
