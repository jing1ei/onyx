//! Progressive waveform peaks.
//!
//! The decoder fills a single-resolution bucket list (min / max / RMS per
//! bucket) while it works, so the waveform lane draws itself in as the file
//! loads. Buckets are sized so that any file produces roughly
//! [`TARGET_BUCKETS`] of them, which keeps the payload to the UI constant no
//! matter how long the track is.

use parking_lot::RwLock;
use serde::Serialize;

/// Buckets we aim for across the whole file.
pub const TARGET_BUCKETS: usize = 2_400;
/// Never bucket fewer frames than this (keeps very short files cheap).
const MIN_BUCKET_FRAMES: usize = 32;

/// One waveform column.
#[derive(Clone, Copy, Debug, Default)]
pub struct Bucket {
    pub min: f32,
    pub max: f32,
    pub rms: f32,
}

/// Shared, progressively-filled waveform.
pub struct Waveform {
    bucket_frames: usize,
    sample_rate: u32,
    buckets: RwLock<Vec<Bucket>>,
    expected_buckets: usize,
}

/// Serialisable waveform payload for the UI.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WaveformData {
    /// Seconds represented by one bucket.
    pub bucket_secs: f64,
    /// Buckets currently available.
    pub count: usize,
    /// Buckets expected once decoding finishes (for a stable X axis).
    pub expected: usize,
    pub min: Vec<f32>,
    pub max: Vec<f32>,
    pub rms: Vec<f32>,
}

impl Waveform {
    pub fn new(sample_rate: u32, expected_frames: usize) -> Self {
        let bucket_frames = if expected_frames == 0 {
            (sample_rate as usize / 40).max(MIN_BUCKET_FRAMES)
        } else {
            (expected_frames / TARGET_BUCKETS).max(MIN_BUCKET_FRAMES)
        };
        let expected_buckets = if expected_frames == 0 {
            0
        } else {
            expected_frames.div_ceil(bucket_frames)
        };
        Waveform {
            bucket_frames,
            sample_rate: sample_rate.max(1),
            buckets: RwLock::new(Vec::with_capacity(expected_buckets.min(1 << 20) + 8)),
            expected_buckets,
        }
    }

    pub fn bucket_frames(&self) -> usize {
        self.bucket_frames
    }

    pub fn bucket_secs(&self) -> f64 {
        self.bucket_frames as f64 / self.sample_rate as f64
    }

    pub fn len(&self) -> usize {
        self.buckets.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn push(&self, b: Bucket) {
        self.buckets.write().push(b);
    }

    /// Snapshot for the UI. `from` allows incremental fetches while decoding.
    pub fn data(&self, from: usize) -> WaveformData {
        let guard = self.buckets.read();
        let from = from.min(guard.len());
        let slice = &guard[from..];
        WaveformData {
            bucket_secs: self.bucket_secs(),
            count: guard.len(),
            expected: self.expected_buckets.max(guard.len()),
            min: slice.iter().map(|b| b.min).collect(),
            max: slice.iter().map(|b| b.max).collect(),
            rms: slice.iter().map(|b| b.rms).collect(),
        }
    }
}

/// Accumulates samples into [`Bucket`]s. Lives on the decode thread.
pub struct WaveformBuilder {
    bucket_frames: usize,
    frames_in_bucket: usize,
    min: f32,
    max: f32,
    sum_sq: f64,
}

impl WaveformBuilder {
    pub fn new(bucket_frames: usize) -> Self {
        WaveformBuilder {
            bucket_frames: bucket_frames.max(1),
            frames_in_bucket: 0,
            min: 0.0,
            max: 0.0,
            sum_sq: 0.0,
        }
    }

    /// Push an interleaved block; completed buckets are appended to `out`.
    pub fn push_interleaved(&mut self, buf: &[f32], channels: usize, out: &Waveform) {
        let channels = channels.max(1);
        let frames = buf.len() / channels;
        for f in 0..frames {
            // Use the loudest channel for the outline and the mean square of
            // all channels for the body: that matches how engineers read a
            // waveform (transients from the peak, weight from the RMS).
            let mut fmin = f32::MAX;
            let mut fmax = f32::MIN;
            let mut sq = 0.0f64;
            for c in 0..channels {
                let s = buf[f * channels + c];
                if s < fmin {
                    fmin = s;
                }
                if s > fmax {
                    fmax = s;
                }
                sq += (s as f64) * (s as f64);
            }
            if self.frames_in_bucket == 0 {
                self.min = fmin;
                self.max = fmax;
                self.sum_sq = 0.0;
            } else {
                if fmin < self.min {
                    self.min = fmin;
                }
                if fmax > self.max {
                    self.max = fmax;
                }
            }
            self.sum_sq += sq / channels as f64;
            self.frames_in_bucket += 1;
            if self.frames_in_bucket >= self.bucket_frames {
                self.flush(out);
            }
        }
    }

    pub fn flush(&mut self, out: &Waveform) {
        if self.frames_in_bucket == 0 {
            return;
        }
        let rms = (self.sum_sq / self.frames_in_bucket as f64).sqrt() as f32;
        out.push(Bucket {
            min: self.min.clamp(-1.5, 1.5),
            max: self.max.clamp(-1.5, 1.5),
            rms: rms.clamp(0.0, 1.5),
        });
        self.frames_in_bucket = 0;
        self.min = 0.0;
        self.max = 0.0;
        self.sum_sq = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_capture_min_max_and_rms() {
        let wf = Waveform::new(48_000, 4_800);
        let mut b = WaveformBuilder::new(100);
        // 100 frames of +-1 square wave => min -1, max 1, rms 1
        let mut buf = Vec::new();
        for i in 0..100 {
            let s = if i % 2 == 0 { 1.0 } else { -1.0 };
            buf.push(s);
            buf.push(s);
        }
        b.push_interleaved(&buf, 2, &wf);
        let d = wf.data(0);
        assert_eq!(d.count, 1);
        assert_eq!(d.min[0], -1.0);
        assert_eq!(d.max[0], 1.0);
        assert!((d.rms[0] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn bucket_size_scales_with_length() {
        let short = Waveform::new(48_000, 48_000);
        let long = Waveform::new(48_000, 48_000 * 600);
        assert!(long.bucket_frames() > short.bucket_frames());
        assert!(short.bucket_frames() >= 32);
    }

    #[test]
    fn incremental_fetch_returns_only_new_buckets() {
        let wf = Waveform::new(48_000, 480);
        let mut b = WaveformBuilder::new(10);
        b.push_interleaved(&vec![0.5f32; 200], 2, &wf); // 100 frames -> 10 buckets
        assert_eq!(wf.data(0).count, 10);
        let tail = wf.data(8);
        assert_eq!(tail.min.len(), 2);
        assert_eq!(tail.count, 10);
    }

    #[test]
    fn partial_bucket_is_flushed() {
        let wf = Waveform::new(48_000, 4_800);
        let mut b = WaveformBuilder::new(1_000);
        b.push_interleaved(&[0.25f32; 20], 2, &wf);
        assert_eq!(wf.len(), 0);
        b.flush(&wf);
        assert_eq!(wf.len(), 1);
        assert!((wf.data(0).rms[0] - 0.25).abs() < 1e-6);
    }

    /// SPEC §9.6: `from` greater than `count` is a legitimate request from a
    /// UI that polled while the buffer was being replaced. It must return an
    /// empty tail with a truthful `count`, not slice out of bounds.
    #[test]
    fn from_beyond_count_is_empty_rather_than_a_panic() {
        let wf = Waveform::new(48_000, 480);
        let mut b = WaveformBuilder::new(10);
        b.push_interleaved(&vec![0.5f32; 200], 2, &wf); // 100 frames -> 10 buckets
        for from in [10usize, 11, 1_000, usize::MAX] {
            let d = wf.data(from);
            assert!(d.min.is_empty() && d.max.is_empty() && d.rms.is_empty());
            assert_eq!(d.count, 10, "count must stay truthful for from={from}");
            assert!(d.expected >= d.count);
        }
        // Exactly at the edge is empty too, and one before it is one bucket.
        assert_eq!(wf.data(9).min.len(), 1);
    }

    /// A zero-length file still has to produce a usable, non-degenerate
    /// waveform object: bucket size > 0 and no division by zero in
    /// `bucket_secs`.
    #[test]
    fn zero_length_and_zero_rate_stay_usable() {
        let wf = Waveform::new(48_000, 0);
        assert!(wf.bucket_frames() >= MIN_BUCKET_FRAMES);
        assert!(wf.bucket_secs() > 0.0);
        assert!(wf.is_empty());
        let d = wf.data(0);
        assert_eq!(d.count, 0);
        assert_eq!(d.expected, 0);

        // A rate of 0 would otherwise make `bucket_secs` infinite.
        let wf = Waveform::new(0, 0);
        assert!(wf.bucket_secs().is_finite());
        assert!(wf.bucket_frames() >= MIN_BUCKET_FRAMES);
    }
}
