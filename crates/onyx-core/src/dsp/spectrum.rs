//! Log-spaced FFT spectrum analyser for the EQ overlay and the spectrum meter.
//!
//! Runs on the analysis thread (never in the output callback): the callback only
//! pushes its finished output block into a lock-free ring.

use rustfft::{num_complex::Complex, Fft, FftPlanner};
use std::sync::Arc;

/// FFT size. 4096 @ 48 kHz gives ~11.7 Hz resolution, enough to see a bass note
/// while staying cheap enough to run 40 times a second.
pub const FFT_SIZE: usize = 4_096;
/// Number of log-spaced display bands.
pub const BANDS: usize = 96;
/// Lowest displayed frequency.
pub const F_MIN: f32 = 20.0;
/// Highest displayed frequency (clamped to just below Nyquist).
pub const F_MAX: f32 = 20_000.0;

pub struct SpectrumAnalyzer {
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    /// `1 / sqrt(N * sum(w^2))`: the power-preserving normalisation used when
    /// the magnitudes of several bins are summed into one display band.
    window_norm: f32,
    ring: Vec<f32>,
    write: usize,
    filled: usize,
    hop: usize,
    since_hop: usize,
    scratch: Vec<Complex<f32>>,
    band_ranges: Vec<(usize, usize)>,
    levels: Vec<f32>,
    sample_rate: f32,
    /// dB released per analysis frame when the level falls.
    release_db: f32,
}

impl SpectrumAnalyzer {
    pub fn new(sample_rate: f32) -> Self {
        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        let window: Vec<f32> = (0..FFT_SIZE)
            .map(|i| {
                // Hann
                0.5 * (1.0
                    - (2.0 * std::f32::consts::PI * i as f32 / (FFT_SIZE as f32 - 1.0)).cos())
            })
            .collect();
        let window_energy: f32 = window.iter().map(|w| w * w).sum();
        // Amplitude of a tone from the *summed power* of the bins it occupies:
        //   sum_{k>0} |X_k|^2 = N * sum(w^2) * A^2 / 4   (Parseval, one-sided)
        // so A = 2 * sqrt(sum |X_k|^2) / sqrt(N * sum(w^2)). Using the coherent
        // gain `sum(w)` instead would over-read a Hann-windowed sine by
        // sqrt(3/2) = +1.76 dB, because its energy is spread over three bins.
        let window_norm = 2.0 / (FFT_SIZE as f32 * window_energy).sqrt().max(1e-9);
        let mut a = SpectrumAnalyzer {
            fft,
            window,
            window_norm,
            ring: vec![0.0; FFT_SIZE],
            write: 0,
            filled: 0,
            hop: FFT_SIZE / 4,
            since_hop: 0,
            scratch: vec![Complex::new(0.0, 0.0); FFT_SIZE],
            band_ranges: Vec::with_capacity(BANDS),
            levels: vec![crate::MIN_DB; BANDS],
            sample_rate: sample_rate.max(8_000.0),
            release_db: 2.2,
        };
        a.set_sample_rate(sample_rate);
        a
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(8_000.0);
        self.hop = (self.sample_rate as usize / 40).clamp(256, FFT_SIZE);
        self.build_bands();
        self.reset();
    }

    pub fn reset(&mut self) {
        self.ring.iter_mut().for_each(|s| *s = 0.0);
        self.write = 0;
        self.filled = 0;
        self.since_hop = 0;
        self.levels.iter_mut().for_each(|l| *l = crate::MIN_DB);
    }

    fn build_bands(&mut self) {
        self.band_ranges.clear();
        let nyq = self.sample_rate * 0.5;
        let f_max = F_MAX.min(nyq * 0.95);
        let bin_hz = self.sample_rate / FFT_SIZE as f32;
        let usable_bins = FFT_SIZE / 2;
        let ratio = (f_max / F_MIN).ln();
        for b in 0..BANDS {
            let lo_f = F_MIN * (ratio * b as f32 / BANDS as f32).exp();
            let hi_f = F_MIN * (ratio * (b + 1) as f32 / BANDS as f32).exp();
            let mut lo = (lo_f / bin_hz).floor() as usize;
            let mut hi = (hi_f / bin_hz).ceil() as usize;
            lo = lo.clamp(1, usable_bins - 1);
            hi = hi.clamp(lo + 1, usable_bins);
            self.band_ranges.push((lo, hi));
        }
    }

    /// Centre frequency of each display band (handy for drawing the axis).
    pub fn band_frequencies(&self) -> Vec<f32> {
        let nyq = self.sample_rate * 0.5;
        let f_max = F_MAX.min(nyq * 0.95);
        let ratio = (f_max / F_MIN).ln();
        (0..BANDS)
            .map(|b| F_MIN * (ratio * (b as f32 + 0.5) / BANDS as f32).exp())
            .collect()
    }

    /// Feed an interleaved stereo block. Returns `true` if a new frame was
    /// analysed.
    pub fn process(&mut self, buf: &[f32]) -> bool {
        let frames = buf.len() / 2;
        let mut analysed = false;
        for f in 0..frames {
            // Mid of the stereo pair: what the listener localises centrally.
            let mono = 0.5 * (buf[f * 2] + buf[f * 2 + 1]);
            self.ring[self.write] = mono;
            self.write = (self.write + 1) % FFT_SIZE;
            self.filled = (self.filled + 1).min(FFT_SIZE);
            self.since_hop += 1;
            if self.since_hop >= self.hop && self.filled >= FFT_SIZE {
                self.since_hop = 0;
                self.analyse();
                analysed = true;
            }
        }
        analysed
    }

    fn analyse(&mut self) {
        for i in 0..FFT_SIZE {
            let idx = (self.write + i) % FFT_SIZE;
            self.scratch[i] = Complex::new(self.ring[idx] * self.window[i], 0.0);
        }
        self.fft.process(&mut self.scratch);

        // See `window_norm`: bins are summed as power, so the normalisation has
        // to be the energy one, not the coherent one.
        let norm = self.window_norm;
        for b in 0..BANDS {
            let (lo, hi) = self.band_ranges[b];
            let mut power = 0.0f32;
            for bin in lo..hi {
                let c = self.scratch[bin];
                power += c.re * c.re + c.im * c.im;
            }
            let amp = power.sqrt() * norm;
            let db = crate::lin_to_db(amp);
            let cur = self.levels[b];
            self.levels[b] = if db > cur {
                db
            } else {
                (cur - self.release_db).max(db)
            };
        }
    }

    /// Current smoothed magnitudes in dBFS, one per display band.
    pub fn levels(&self) -> &[f32] {
        &self.levels
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_sine(a: &mut SpectrumAnalyzer, freq: f32, amp: f32, sr: f32, frames: usize) {
        let buf: Vec<f32> = (0..frames)
            .flat_map(|i| {
                let s = amp * (2.0 * std::f32::consts::PI * freq * i as f32 / sr).sin();
                [s, s]
            })
            .collect();
        a.process(&buf);
    }

    #[test]
    fn peak_lands_in_the_right_band_at_the_right_level() {
        let sr = 48_000.0;
        let mut a = SpectrumAnalyzer::new(sr);
        feed_sine(&mut a, 1_000.0, 1.0, sr, 48_000);
        let freqs = a.band_frequencies();
        let (best, &level) = a
            .levels()
            .iter()
            .enumerate()
            .max_by(|x, y| x.1.partial_cmp(y.1).unwrap())
            .unwrap();
        assert!(
            (freqs[best] / 1_000.0).log2().abs() < 0.25,
            "peak band centre {} Hz",
            freqs[best]
        );
        assert!(level > -3.0 && level < 1.0, "level {level} dBFS");
    }

    #[test]
    fn level_tracks_amplitude() {
        let sr = 48_000.0;
        let mut loud = SpectrumAnalyzer::new(sr);
        feed_sine(&mut loud, 2_000.0, 1.0, sr, 48_000);
        let mut quiet = SpectrumAnalyzer::new(sr);
        feed_sine(&mut quiet, 2_000.0, 0.25, sr, 48_000);
        let l = loud.levels().iter().cloned().fold(f32::MIN, f32::max);
        let q = quiet.levels().iter().cloned().fold(f32::MIN, f32::max);
        assert!((l - q - 12.04).abs() < 0.6, "delta {}", l - q);
    }

    #[test]
    fn bands_never_exceed_nyquist() {
        for sr in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            let a = SpectrumAnalyzer::new(sr);
            for f in a.band_frequencies() {
                assert!(f < sr * 0.5, "band {f} above Nyquist for {sr}");
            }
        }
    }

    #[test]
    fn silence_decays_to_the_floor() {
        let sr = 48_000.0;
        let mut a = SpectrumAnalyzer::new(sr);
        feed_sine(&mut a, 1_000.0, 1.0, sr, 24_000);
        a.process(&vec![0.0f32; 48_000 * 2 * 4]);
        assert!(a.levels().iter().all(|&l| l < -80.0));
    }
}
