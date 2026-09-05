//! True-peak measurement (dBTP) via 4x polyphase oversampling, as required by
//! ITU-R BS.1770-4 Annex 2.
//!
//! # Why this is not the recommendation's coefficient table
//!
//! BS.1770-4 Annex 2 prints *one* 48-tap / 4-phase FIR that "would satisfy the
//! requirements", and `recommends 4` explicitly permits "a method that gives
//! similar or superior results". That table is not normative, and it is not
//! very good: its prototype has an even length, so its four polyphase branches
//! sit at 1/8, 3/8, 5/8 and 7/8 of a sample rather than on the natural
//! 0, 1/4, 1/2, 3/4 grid, and none of them reproduces the input sample itself.
//! Measured against a band-limited impulse whose analytic peak is known (see
//! `tests/truepeak_bs1770.rs`) the printed table under-reads by up to 0.80 dB.
//!
//! This implementation therefore uses an odd-length (129-tap) Kaiser-windowed
//! sinc prototype decomposed into 4 branches of 33 taps. Two consequences:
//!
//! * branch 0 collapses to a unit impulse (the sinc zeros land exactly on the
//!   non-zero multiples of 4), so the raw input samples are part of the search
//!   grid for free and the reported true peak can never fall below the sample
//!   peak;
//! * branches 1..3 are the 1/4, 1/2 and 3/4 fractional-sample delays.
//!
//! Measured accuracy (see `tests/truepeak_bs1770.rs`; `f` is relative to the
//! input sample rate, the error is the deviation of the reconstructed
//! magnitude from the analytic value of the continuous waveform):
//!
//! | band          | interpolation error |
//! |---------------|---------------------|
//! | f <= 0.25 fs  | +/- 0.014 dB        |
//! | f <= 0.40 fs  | +/- 0.025 dB        |
//! | f <= 0.45 fs  | +/- 0.040 dB        |
//!
//! What remains is the error of the 4x sampling *grid* itself, which no 4x
//! meter can beat: BS.1770-4 Annex 2, Appendix 1 tabulates it as 0.554 dB at
//! `f_norm = 0.45` and 0.688 dB at `f_norm = 0.5`. On a band-limited impulse
//! filling 90 % of the band this implementation measures -0.177 dB against a
//! grid limit of -0.182 dB, i.e. the filter itself contributes ~0.005 dB.

/// Oversampling factor (BS.1770-4 Annex 2).
const PHASES: usize = 4;
/// Taps per polyphase branch. The prototype length is
/// `PHASES * (TAPS - 1) + 1`, which is odd so that branch 0 is a unit impulse.
const TAPS: usize = 33;
/// Kaiser beta, chosen by sweeping beta against the reconstruction error of the
/// four branches; 5.0 minimises the worst case below 0.45 fs.
const KAISER_BETA: f64 = 5.0;

/// Frames that must pass before the delay line holds nothing but real audio.
/// Until then the interpolator is convolving the signal with the zeros it was
/// primed with, which manufactures an overshoot that is not in the material.
/// Those frames still contribute their *sample* peak; only the interpolated
/// branches are suppressed.
const WARMUP_FRAMES: usize = TAPS - 1;

/// Per-channel 4x true-peak detector.
#[derive(Clone)]
pub struct TruePeak {
    /// `(PHASES - 1) * TAPS` coefficients for branches 1..3, phase-major and
    /// time-reversed so each branch is a straight dot product with the delay
    /// window. Branch 0 is the identity and is not stored.
    coeffs: Vec<f32>,
    /// `channels * 2 * TAPS` delay line; every sample is written twice so the
    /// most recent `TAPS` samples are always contiguous.
    delay: Vec<f32>,
    channels: usize,
    pos: usize,
    /// Frames still to be ignored while the delay line fills up.
    warmup: usize,
    /// Session maximum per channel, linear.
    max: Vec<f32>,
}

impl TruePeak {
    pub fn new(channels: usize) -> Self {
        let channels = channels.max(1);
        Self {
            coeffs: design_polyphase(),
            delay: vec![0.0; channels * 2 * TAPS],
            channels,
            pos: 0,
            warmup: WARMUP_FRAMES,
            max: vec![0.0; channels],
        }
    }

    pub fn reset(&mut self) {
        self.delay.fill(0.0);
        self.pos = 0;
        self.warmup = WARMUP_FRAMES;
        for m in self.max.iter_mut() {
            *m = 0.0;
        }
    }

    /// Feed an interleaved block with `self.channels()` channels.
    pub fn process(&mut self, buf: &[f32]) {
        let ch = self.channels;
        if ch == 0 {
            return;
        }
        let frames = buf.len() / ch;
        for f in 0..frames {
            // Advance the shared write position once per frame.
            self.pos = if self.pos + 1 == TAPS {
                0
            } else {
                self.pos + 1
            };
            let pos = self.pos;
            let priming = self.warmup > 0;
            if priming {
                self.warmup -= 1;
            }
            for c in 0..ch {
                let x = buf[f * ch + c];
                let line = &mut self.delay[c * 2 * TAPS..(c + 1) * 2 * TAPS];
                line[pos] = x;
                line[pos + TAPS] = x;
                // Branch 0 of the polyphase bank is a unit impulse, so the raw
                // sample is one of the four grid points and always counts.
                let mut local = x.abs();
                if !priming {
                    // `win` is [x[n-TAPS+1] .. x[n]], oldest first.
                    let win = &line[pos + 1..pos + 1 + TAPS];
                    for p in 0..PHASES - 1 {
                        let coeffs = &self.coeffs[p * TAPS..(p + 1) * TAPS];
                        let mut acc = 0.0f32;
                        for t in 0..TAPS {
                            acc += coeffs[t] * win[t];
                        }
                        let a = acc.abs();
                        if a > local {
                            local = a;
                        }
                    }
                }
                if local > self.max[c] {
                    self.max[c] = local;
                }
            }
        }
    }

    /// Session maximum true peak for `channel`, in dBTP.
    pub fn peak_db(&self, channel: usize) -> f32 {
        crate::lin_to_db(self.max.get(channel).copied().unwrap_or(0.0))
    }

    pub fn peak_lin(&self, channel: usize) -> f32 {
        self.max.get(channel).copied().unwrap_or(0.0)
    }

    pub fn channels(&self) -> usize {
        self.channels
    }
}

/// Build the fractional-delay branches 1..3, time-reversed for a dot product.
fn design_polyphase() -> Vec<f32> {
    let proto = prototype();
    let total = proto.len();
    let mut out = vec![0.0f32; (PHASES - 1) * TAPS];
    for p in 1..PHASES {
        // Branch p is proto[t * PHASES + p], t = 0..TAPS-1; for p > 0 the last
        // index runs past the end of the odd-length prototype, i.e. it is zero.
        let mut branch = [0.0f64; TAPS];
        let mut sum = 0.0f64;
        for (t, b) in branch.iter_mut().enumerate() {
            let idx = t * PHASES + p;
            if idx < total {
                *b = proto[idx];
                sum += *b;
            }
        }
        // Unity DC gain per branch, so steady signals are not rescaled.
        let norm = if sum.abs() < 1e-12 { 1.0 } else { 1.0 / sum };
        for t in 0..TAPS {
            // Time-reversed: out[..][t] multiplies x[n - TAPS + 1 + t].
            out[(p - 1) * TAPS + t] = (branch[TAPS - 1 - t] * norm) as f32;
        }
    }
    out
}

/// The Kaiser-windowed sinc prototype at the 4x rate.
fn prototype() -> Vec<f64> {
    let total = PHASES * (TAPS - 1) + 1;
    let centre = (total - 1) as f64 / 2.0;
    // Cutoff exactly at the input Nyquist. This is what makes branch 0 a unit
    // impulse: the sinc zeros then fall on every non-zero multiple of PHASES.
    let cutoff = 0.5 / PHASES as f64;
    (0..total)
        .map(|n| {
            let x = n as f64 - centre;
            let sinc = if x.abs() < 1e-12 {
                2.0 * cutoff
            } else {
                (2.0 * std::f64::consts::PI * cutoff * x).sin() / (std::f64::consts::PI * x)
            };
            sinc * kaiser(n as f64 / (total - 1) as f64, KAISER_BETA)
        })
        .collect()
}

/// Kaiser window, `t` in `0..=1`.
fn kaiser(t: f64, beta: f64) -> f64 {
    let x = 2.0 * t - 1.0;
    let arg = beta * (1.0 - x * x).max(0.0).sqrt();
    bessel_i0(arg) / bessel_i0(beta)
}

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let half = x / 2.0;
    for k in 1..40 {
        term *= (half / k as f64) * (half / k as f64);
        sum += term;
        if term < 1e-16 * sum {
            break;
        }
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measure(signal: &[f32]) -> f32 {
        let mut tp = TruePeak::new(1);
        tp.process(signal);
        tp.peak_db(0)
    }

    #[test]
    fn dc_is_measured_at_its_own_level() {
        let db = measure(&vec![0.5f32; 4_096]);
        assert!(
            (db - crate::lin_to_db(0.5)).abs() < 0.1,
            "DC measured {db} dBTP"
        );
    }

    #[test]
    fn full_scale_sine_is_about_zero_dbtp() {
        let sr = 48_000.0f32;
        let sig: Vec<f32> = (0..48_000)
            .map(|i| (2.0 * std::f32::consts::PI * 1_000.0 * i as f32 / sr).sin())
            .collect();
        let db = measure(&sig);
        assert!(db.abs() < 0.2, "measured {db} dBTP");
    }

    /// The classic case: a sine that lands between samples reads ~0 dBFS on a
    /// sample-peak meter but clips on reconstruction. True peak must catch it.
    #[test]
    fn catches_inter_sample_peaks() {
        let sr = 48_000.0f32;
        // 12 kHz = fs/4, phase-shifted so samples straddle the crest.
        let sig: Vec<f32> = (0..48_000)
            .map(|i| {
                (2.0 * std::f32::consts::PI * 12_000.0 * i as f32 / sr + std::f32::consts::PI / 4.0)
                    .sin()
                    * 0.707
            })
            .collect();
        // Sampled at fs/4 with a 45 degree offset the crest is never sampled:
        // sample peak sits at 0.5 (-6.02 dBFS) while the real waveform reaches
        // 0.707 (-3.01 dBTP).
        let sample_peak = sig.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        let sample_peak_db = crate::lin_to_db(sample_peak);
        let true_peak_db = measure(&sig);
        assert!(
            true_peak_db > sample_peak_db + 2.0,
            "true peak {true_peak_db} should exceed sample peak {sample_peak_db} by ~3 dB"
        );
        assert!(
            (true_peak_db + 3.01).abs() < 0.6,
            "expected ~-3.01 dBTP, got {true_peak_db}"
        );
    }

    #[test]
    fn silence_reads_the_floor() {
        assert_eq!(measure(&vec![0.0f32; 1_024]), crate::MIN_DB);
    }

    #[test]
    fn interleaved_channels_are_independent() {
        let mut tp = TruePeak::new(2);
        let mut buf = Vec::new();
        for i in 0..4_096 {
            let s = (2.0 * std::f32::consts::PI * 500.0 * i as f32 / 48_000.0).sin();
            buf.push(s);
            buf.push(s * 0.25);
        }
        tp.process(&buf);
        let diff = tp.peak_db(0) - tp.peak_db(1);
        assert!((diff - 12.04).abs() < 0.3, "channel delta {diff}");
    }

    /// Branch 0 must be a unit impulse. If it ever stops being one the raw
    /// samples leave the search grid and the meter can report *less* than the
    /// sample peak, which is physically impossible.
    #[test]
    fn branch_zero_is_a_unit_impulse() {
        let proto = prototype();
        let centre = (proto.len() - 1) / 2;
        assert_eq!(centre % PHASES, 0, "branch 0 must contain the centre tap");
        let mut t = 0;
        while t * PHASES < proto.len() {
            let expect = if t * PHASES == centre {
                1.0 / PHASES as f64
            } else {
                0.0
            };
            assert!(
                (proto[t * PHASES] - expect).abs() < 1e-15,
                "branch-0 tap {t} is {}, expected {expect}",
                proto[t * PHASES]
            );
            t += 1;
        }
    }

    /// The branches must be the 1/4, 1/2 and 3/4 fractional delays, i.e. the
    /// prototype must be symmetric and branch 1 the mirror of branch 3.
    #[test]
    fn prototype_is_linear_phase() {
        let proto = prototype();
        let n = proto.len();
        for i in 0..n / 2 {
            assert!(
                (proto[i] - proto[n - 1 - i]).abs() < 1e-15,
                "prototype is not symmetric at {i}"
            );
        }
    }
}
