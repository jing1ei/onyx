//! The meter bridge: peak / RMS / true-peak / loudness / correlation /
//! spectrum, all measured on the post-EQ output bus.
//!
//! Ballistics are chosen to match what mastering engineers expect:
//! * peak      - instant attack, 20 dB/s fallback (digital peak programme meter)
//! * hold      - 1.5 s freeze on the highest peak, then released
//! * RMS       - 300 ms exponential average
//! * true peak - session max hold, 4x oversampled (dBTP)
//! * loudness  - BS.1770-4 momentary / short / integrated + LRA
//! * balance   - 300 ms exponential Pearson correlation of L and R

use super::loudness::LoudnessMeter;
use super::spectrum::SpectrumAnalyzer;
use super::truepeak::TruePeak;
use crate::types::MeterSnapshot;

const PEAK_FALL_DB_PER_SEC: f32 = 20.0;
const HOLD_SECS: f32 = 1.5;
const RMS_TIME_SECS: f32 = 0.3;
const CORR_TIME_SECS: f32 = 0.3;
/// A sample at or above this magnitude counts as clipped.
const CLIP_THRESHOLD: f32 = 0.999_85;

pub struct MeterBank {
    sample_rate: f32,
    loudness: LoudnessMeter,
    true_peak: TruePeak,
    spectrum: SpectrumAnalyzer,
    /// The FFT is the most expensive thing in the bank; a closed analyser
    /// panel switches it off entirely (SPEC §12).
    spectrum_enabled: bool,

    peak: [f32; 2],
    peak_fall_per_sample: f32,
    hold: [f32; 2],
    hold_samples_left: [u32; 2],
    hold_samples: u32,

    ms: [f64; 2],
    rms_alpha: f64,

    corr_lr: f64,
    corr_ll: f64,
    corr_rr: f64,
    corr_alpha: f64,

    clip_count: u32,
}

impl MeterBank {
    pub fn new(sample_rate: f32) -> Self {
        let mut m = MeterBank {
            sample_rate: sample_rate.max(8_000.0),
            loudness: LoudnessMeter::new(sample_rate as f64),
            true_peak: TruePeak::new(2),
            spectrum: SpectrumAnalyzer::new(sample_rate),
            spectrum_enabled: true,
            peak: [0.0; 2],
            peak_fall_per_sample: 1.0,
            hold: [0.0; 2],
            hold_samples_left: [0; 2],
            hold_samples: 0,
            ms: [0.0; 2],
            rms_alpha: 0.0,
            corr_lr: 0.0,
            corr_ll: 0.0,
            corr_rr: 0.0,
            corr_alpha: 0.0,
            clip_count: 0,
        };
        m.set_sample_rate(sample_rate);
        m
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        let sr = sample_rate.max(8_000.0);
        self.sample_rate = sr;
        self.peak_fall_per_sample = 10f32.powf(-PEAK_FALL_DB_PER_SEC / (20.0 * sr));
        self.hold_samples = (HOLD_SECS * sr) as u32;
        self.rms_alpha = 1.0 - (-1.0f64 / (RMS_TIME_SECS as f64 * sr as f64)).exp();
        self.corr_alpha = 1.0 - (-1.0f64 / (CORR_TIME_SECS as f64 * sr as f64)).exp();
        self.loudness.set_sample_rate(sr as f64);
        self.spectrum.set_sample_rate(sr);
        self.true_peak = TruePeak::new(2);
        self.reset();
    }

    /// Clear every read-out. Called on load, on seek and from the UI.
    pub fn reset(&mut self) {
        self.peak = [0.0; 2];
        self.hold = [0.0; 2];
        self.hold_samples_left = [0; 2];
        self.ms = [0.0; 2];
        self.corr_lr = 0.0;
        self.corr_ll = 0.0;
        self.corr_rr = 0.0;
        self.clip_count = 0;
        self.loudness.reset();
        self.true_peak.reset();
        self.spectrum.reset();
    }

    /// Reset only the things that must not survive a jump in the timeline.
    /// True peak and integrated loudness are deliberately kept: engineers use
    /// them as "worst case so far" read-outs.
    pub fn reset_transient(&mut self) {
        self.peak = [0.0; 2];
        self.hold = [0.0; 2];
        self.hold_samples_left = [0; 2];
        self.ms = [0.0; 2];
        self.spectrum.reset();
    }

    /// Feed an interleaved stereo block.
    pub fn process(&mut self, buf: &[f32]) {
        let frames = buf.len() / 2;
        if frames == 0 {
            return;
        }

        for f in 0..frames {
            let l = buf[f * 2];
            let r = buf[f * 2 + 1];

            for (ch, s) in [l, r].iter().enumerate() {
                let a = s.abs();
                if a > self.peak[ch] {
                    self.peak[ch] = a;
                } else {
                    self.peak[ch] *= self.peak_fall_per_sample;
                }
                if a >= self.hold[ch] {
                    self.hold[ch] = a;
                    self.hold_samples_left[ch] = self.hold_samples;
                } else if self.hold_samples_left[ch] > 0 {
                    self.hold_samples_left[ch] -= 1;
                } else {
                    self.hold[ch] *= self.peak_fall_per_sample;
                }
                if a >= CLIP_THRESHOLD {
                    self.clip_count = self.clip_count.saturating_add(1);
                }
            }

            let (lf, rf) = (l as f64, r as f64);
            self.ms[0] += (lf * lf - self.ms[0]) * self.rms_alpha;
            self.ms[1] += (rf * rf - self.ms[1]) * self.rms_alpha;

            self.corr_lr += (lf * rf - self.corr_lr) * self.corr_alpha;
            self.corr_ll += (lf * lf - self.corr_ll) * self.corr_alpha;
            self.corr_rr += (rf * rf - self.corr_rr) * self.corr_alpha;
        }

        self.true_peak.process(buf);
        self.loudness.process(buf);
        if self.spectrum_enabled {
            self.spectrum.process(buf);
        }
    }

    /// Enable / disable the FFT. Disabling parks the analyser at the meter
    /// floor so a stale curve is never shown as if it were live.
    pub fn set_spectrum_enabled(&mut self, enabled: bool) {
        if enabled == self.spectrum_enabled {
            return;
        }
        self.spectrum_enabled = enabled;
        self.spectrum.reset();
    }

    pub fn spectrum_enabled(&self) -> bool {
        self.spectrum_enabled
    }

    pub fn correlation(&self) -> f32 {
        let den = (self.corr_ll * self.corr_rr).sqrt();
        if den < 1e-12 {
            0.0
        } else {
            ((self.corr_lr / den) as f32).clamp(-1.0, 1.0)
        }
    }

    /// Build the snapshot handed to the UI. Reuses the caller's `Vec` for the
    /// spectrum so the 40 Hz update loop does not churn the allocator.
    pub fn fill_snapshot(&mut self, out: &mut MeterSnapshot) {
        out.peak_db = [
            crate::lin_to_db(self.peak[0]),
            crate::lin_to_db(self.peak[1]),
        ];
        out.peak_hold_db = [
            crate::lin_to_db(self.hold[0]),
            crate::lin_to_db(self.hold[1]),
        ];
        out.rms_db = [
            crate::lin_to_db((self.ms[0].max(0.0).sqrt()) as f32),
            crate::lin_to_db((self.ms[1].max(0.0).sqrt()) as f32),
        ];
        out.true_peak_db = [self.true_peak.peak_db(0), self.true_peak.peak_db(1)];
        out.lufs_momentary = self.loudness.momentary();
        out.lufs_short = self.loudness.short_term();
        out.lufs_integrated = self.loudness.integrated();
        out.lra = self.loudness.lra();
        out.correlation = self.correlation();
        out.clip_count = self.clip_count;
        out.spectrum.clear();
        out.spectrum.extend_from_slice(self.spectrum.levels());
    }

    pub fn snapshot(&mut self) -> MeterSnapshot {
        let mut s = MeterSnapshot::default();
        self.fill_snapshot(&mut s);
        s
    }

    pub fn band_frequencies(&self) -> Vec<f32> {
        self.spectrum.band_frequencies()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    fn sine(freq: f32, amp: f32, sr: f32, secs: f32) -> Vec<f32> {
        let n = (sr * secs) as usize;
        (0..n)
            .flat_map(|i| {
                let s = amp * (2.0 * std::f32::consts::PI * freq * i as f32 / sr).sin();
                [s, s]
            })
            .collect()
    }

    #[test]
    fn peak_and_rms_are_three_db_apart_for_a_sine() {
        let sr = 48_000.0;
        let mut m = MeterBank::new(sr);
        m.process(&sine(1_000.0, 1.0, sr, 2.0));
        let s = m.snapshot();
        assert_abs_diff_eq!(s.peak_db[0], 0.0, epsilon = 0.2);
        assert_abs_diff_eq!(s.rms_db[0], -3.01, epsilon = 0.15);
    }

    #[test]
    fn peak_falls_back_at_twenty_db_per_second() {
        let sr = 48_000.0;
        let mut m = MeterBank::new(sr);
        m.process(&sine(1_000.0, 1.0, sr, 0.5));
        m.process(&vec![0.0f32; (sr as usize) * 2]); // 1 s of silence
        let s = m.snapshot();
        assert_abs_diff_eq!(s.peak_db[0], -20.0, epsilon = 1.0);
    }

    #[test]
    fn correlation_detects_mono_and_out_of_phase() {
        let sr = 48_000.0;
        let mut mono = MeterBank::new(sr);
        mono.process(&sine(500.0, 0.5, sr, 2.0));
        assert_abs_diff_eq!(mono.correlation(), 1.0, epsilon = 0.02);

        let mut anti = MeterBank::new(sr);
        let mut buf = sine(500.0, 0.5, sr, 2.0);
        for f in 0..buf.len() / 2 {
            buf[f * 2 + 1] = -buf[f * 2 + 1];
        }
        anti.process(&buf);
        assert_abs_diff_eq!(anti.correlation(), -1.0, epsilon = 0.02);
    }

    #[test]
    fn clipping_is_counted() {
        let sr = 48_000.0;
        let mut m = MeterBank::new(sr);
        m.process(&vec![1.0f32; 200]);
        assert_eq!(m.snapshot().clip_count, 200);
    }

    #[test]
    fn silence_reports_floors_everywhere() {
        let mut m = MeterBank::new(48_000.0);
        m.process(&vec![0.0f32; 48_000 * 2]);
        let s = m.snapshot();
        assert_eq!(s.peak_db[0], crate::MIN_DB);
        assert_eq!(s.true_peak_db[0], crate::MIN_DB);
        assert_eq!(s.lufs_momentary, crate::LUFS_SILENCE);
        assert_eq!(s.correlation, 0.0);
    }

    #[test]
    fn snapshot_spectrum_has_the_expected_shape() {
        let mut m = MeterBank::new(48_000.0);
        m.process(&sine(1_000.0, 0.5, 48_000.0, 1.0));
        let s = m.snapshot();
        assert_eq!(s.spectrum.len(), super::super::spectrum::BANDS);
    }
}
