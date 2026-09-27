//! A fixed-capacity, single-producer / multi-consumer PCM buffer that can be
//! read by the audio callback while it is still being written by the decoder.
//!
//! This is what makes "click a track, hear it now" possible: the decoder starts
//! filling the buffer and the engine begins playing after a few milliseconds,
//! long before the whole file has been decoded.
//!
//! # Safety model
//!
//! * The backing storage is allocated once, up front, and never moves or grows.
//! * There is exactly one writer ([`PcmWriter`], not `Clone`), and it only ever
//!   touches frames `>= frames_ready`.
//! * Readers only ever touch frames `< frames_ready`.
//! * `frames_ready` is published with `Release` and observed with `Acquire`, so
//!   a reader that sees frame `n` also sees the samples written before it.
//!
//! Raw pointers (rather than `&mut [f32]` / `&[f32]`) are used so that reader
//! and writer views never alias as Rust references.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

/// Interleaved f32 PCM at the engine sample rate.
pub struct SharedPcm {
    ptr: *mut f32,
    len: usize,
    channels: usize,
    capacity_frames: usize,
    /// Frames the decoder has published so far.
    frames_ready: AtomicUsize,
    /// Set once the decoder has finished (successfully or not).
    complete: AtomicBool,
    /// Best-effort total length from the container, used for progress display.
    expected_frames: usize,
}

// SAFETY: see the module-level safety model. All shared mutation goes through
// atomics; sample slots are strictly partitioned between writer and readers.
unsafe impl Send for SharedPcm {}
unsafe impl Sync for SharedPcm {}

impl SharedPcm {
    /// Allocate storage for `capacity_frames` of `channels`-channel audio.
    ///
    /// Returns the shared handle plus the unique writer.
    pub fn new(
        channels: usize,
        capacity_frames: usize,
        expected_frames: usize,
    ) -> (Arc<SharedPcm>, PcmWriter) {
        let channels = channels.max(1);
        let len = channels * capacity_frames.max(1);
        // Zeroed so that a reader racing slightly ahead reads silence, never
        // uninitialised memory.
        let boxed: Box<[f32]> = vec![0.0f32; len].into_boxed_slice();
        let ptr = Box::into_raw(boxed) as *mut f32;

        let pcm = Arc::new(SharedPcm {
            ptr,
            len,
            channels,
            capacity_frames: capacity_frames.max(1),
            frames_ready: AtomicUsize::new(0),
            complete: AtomicBool::new(false),
            expected_frames,
        });
        let writer = PcmWriter {
            pcm: Arc::clone(&pcm),
            write_frame: 0,
        };
        (pcm, writer)
    }

    /// Build a fully populated buffer from interleaved samples (used by tests
    /// and by the offline analysis path).
    pub fn from_interleaved(channels: usize, samples: &[f32]) -> Arc<SharedPcm> {
        let channels = channels.max(1);
        let frames = samples.len() / channels;
        let (pcm, mut writer) = SharedPcm::new(channels, frames, frames);
        writer.write_interleaved(&samples[..frames * channels]);
        writer.finish();
        pcm
    }

    #[inline]
    pub fn channels(&self) -> usize {
        self.channels
    }

    #[inline]
    pub fn capacity_frames(&self) -> usize {
        self.capacity_frames
    }

    #[inline]
    pub fn expected_frames(&self) -> usize {
        self.expected_frames
    }

    /// Frames that are safe to read right now.
    #[inline]
    pub fn frames_ready(&self) -> usize {
        self.frames_ready.load(Ordering::Acquire)
    }

    #[inline]
    pub fn is_complete(&self) -> bool {
        self.complete.load(Ordering::Acquire)
    }

    /// Decode progress in `0.0..=1.0`.
    #[inline]
    pub fn progress(&self) -> f32 {
        if self.is_complete() {
            return 1.0;
        }
        let total = if self.expected_frames > 0 {
            self.expected_frames
        } else {
            self.capacity_frames
        };
        if total == 0 {
            return 1.0;
        }
        (self.frames_ready() as f32 / total as f32).clamp(0.0, 1.0)
    }

    /// Read one frame, folded to stereo.
    ///
    /// * mono sources are copied to both legs (no -3 dB pan law: a mono file
    ///   should meter identically on both channels),
    /// * stereo passes straight through,
    /// * multichannel takes the front pair, which is what an engineer expects
    ///   from a quick-listen tool.
    ///
    /// Callers should only read frames `< frames_ready()`; a frame beyond the
    /// allocation reads as silence. That bounds check is deliberate rather than
    /// a `debug_assert`: this is a safe `pub fn` on a type built out of raw
    /// pointers, so an out-of-range index from anywhere - including the Tauri
    /// layer or a waveform request - must not be undefined behaviour. It costs
    /// one compare against a value that is already hot in cache.
    #[inline]
    pub fn frame_stereo(&self, frame: usize) -> [f32; 2] {
        if frame >= self.capacity_frames {
            return [0.0, 0.0];
        }
        let base = frame * self.channels;
        debug_assert!(base + self.channels <= self.len);
        unsafe {
            match self.channels {
                1 => {
                    let s = *self.ptr.add(base);
                    [s, s]
                }
                _ => [*self.ptr.add(base), *self.ptr.add(base + 1)],
            }
        }
    }

    /// Iterate raw interleaved samples for offline analysis. Only frames that
    /// are already published are visited.
    pub fn for_each_ready_frame<F: FnMut(&[f32])>(&self, mut f: F) {
        let ready = self.frames_ready();
        let ch = self.channels;
        let mut scratch = vec![0.0f32; ch];
        for frame in 0..ready {
            let base = frame * ch;
            for (c, slot) in scratch.iter_mut().enumerate() {
                *slot = unsafe { *self.ptr.add(base + c) };
            }
            f(&scratch);
        }
    }
}

impl std::fmt::Debug for SharedPcm {
    /// Deliberately does not touch the samples: this is only ever used for
    /// diagnostics / test failure messages.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedPcm")
            .field("channels", &self.channels)
            .field("capacity_frames", &self.capacity_frames)
            .field("frames_ready", &self.frames_ready())
            .field("complete", &self.is_complete())
            .finish()
    }
}

impl Drop for SharedPcm {
    fn drop(&mut self) {
        // SAFETY: `ptr`/`len` came from `Box<[f32]>::into_raw` and no writer or
        // reader can exist any more (we are the last owner).
        unsafe {
            let slice = std::slice::from_raw_parts_mut(self.ptr, self.len);
            drop(Box::from_raw(slice as *mut [f32]));
        }
    }
}

/// The unique producer for a [`SharedPcm`].
pub struct PcmWriter {
    pcm: Arc<SharedPcm>,
    write_frame: usize,
}

impl PcmWriter {
    /// Frames still available in the pre-allocated buffer.
    #[inline]
    pub fn remaining_frames(&self) -> usize {
        self.pcm.capacity_frames.saturating_sub(self.write_frame)
    }

    #[inline]
    pub fn channels(&self) -> usize {
        self.pcm.channels
    }

    #[inline]
    pub fn frames_written(&self) -> usize {
        self.write_frame
    }

    /// Append interleaved samples. Returns the number of frames accepted;
    /// anything past the pre-allocated capacity is dropped (this only happens
    /// when a container lies about its duration).
    pub fn write_interleaved(&mut self, samples: &[f32]) -> usize {
        let ch = self.pcm.channels;
        let frames = samples.len() / ch;
        let n = frames.min(self.remaining_frames());
        if n == 0 {
            return 0;
        }
        let base = self.write_frame * ch;
        // SAFETY: exclusive writer, and `base + n*ch <= len` by construction.
        unsafe {
            std::ptr::copy_nonoverlapping(samples.as_ptr(), self.pcm.ptr.add(base), n * ch);
        }
        self.write_frame += n;
        self.pcm
            .frames_ready
            .store(self.write_frame, Ordering::Release);
        n
    }

    /// Mark the buffer as final. Idempotent.
    pub fn finish(&mut self) {
        self.pcm
            .frames_ready
            .store(self.write_frame, Ordering::Release);
        self.pcm.complete.store(true, Ordering::Release);
    }

    pub fn handle(&self) -> Arc<SharedPcm> {
        Arc::clone(&self.pcm)
    }
}

impl Drop for PcmWriter {
    fn drop(&mut self) {
        // A dropped writer can never publish more audio, so readers must be
        // told to stop waiting.
        self.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publishes_progressively() {
        let (pcm, mut w) = SharedPcm::new(2, 8, 8);
        assert_eq!(pcm.frames_ready(), 0);
        assert!(!pcm.is_complete());

        w.write_interleaved(&[0.5, -0.5, 0.25, -0.25]);
        assert_eq!(pcm.frames_ready(), 2);
        assert_eq!(pcm.frame_stereo(0), [0.5, -0.5]);
        assert_eq!(pcm.frame_stereo(1), [0.25, -0.25]);

        w.finish();
        assert!(pcm.is_complete());
    }

    #[test]
    fn clamps_at_capacity() {
        let (pcm, mut w) = SharedPcm::new(1, 3, 3);
        let accepted = w.write_interleaved(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        assert_eq!(accepted, 3);
        assert_eq!(pcm.frames_ready(), 3);
        assert_eq!(pcm.frame_stereo(2), [3.0, 3.0]);
    }

    #[test]
    fn mono_is_duplicated_not_attenuated() {
        let pcm = SharedPcm::from_interleaved(1, &[0.75]);
        assert_eq!(pcm.frame_stereo(0), [0.75, 0.75]);
    }

    #[test]
    fn dropping_writer_completes_buffer() {
        let (pcm, w) = SharedPcm::new(2, 4, 4);
        drop(w);
        assert!(pcm.is_complete());
    }

    #[test]
    fn multichannel_folds_to_the_front_pair() {
        // 5.1 frame: L R C LFE Ls Rs. We want L/R only, untouched.
        let pcm = SharedPcm::from_interleaved(6, &[0.1, 0.2, 0.3, 0.4, 0.5, 0.6]);
        assert_eq!(pcm.channels(), 6);
        assert_eq!(pcm.frame_stereo(0), [0.1, 0.2]);
    }

    #[test]
    fn progress_uses_the_container_hint_then_snaps_to_one() {
        // A container that under-reports (expected 4, capacity 10): progress
        // must still be monotonic and never exceed 1.0.
        let (pcm, mut w) = SharedPcm::new(1, 10, 4);
        assert_eq!(pcm.progress(), 0.0);
        w.write_interleaved(&[0.0, 0.0]);
        assert!((pcm.progress() - 0.5).abs() < 1e-6);
        w.write_interleaved(&[0.0; 6]);
        assert_eq!(pcm.progress(), 1.0, "clamped, not >1");
        w.finish();
        assert_eq!(pcm.progress(), 1.0);
    }

    #[test]
    fn for_each_ready_frame_stops_at_the_published_edge() {
        let (pcm, mut w) = SharedPcm::new(2, 16, 16);
        w.write_interleaved(&[1.0, -1.0, 2.0, -2.0]);
        let mut seen = Vec::new();
        pcm.for_each_ready_frame(|f| seen.push(f.to_vec()));
        assert_eq!(seen, vec![vec![1.0, -1.0], vec![2.0, -2.0]]);
    }

    /// The whole point of `SharedPcm`: a reader (the audio callback) walks the
    /// buffer while the decoder is still appending to it. This hammers the
    /// hand-off for a while and asserts that a reader never sees a torn or
    /// stale frame - every frame below `frames_ready()` must hold exactly the
    /// value the writer put there.
    ///
    /// Run this under `cargo +nightly miri` or TSan if you touch `pcm.rs`.
    #[test]
    fn concurrent_read_while_write_never_tears() {
        const FRAMES: usize = 200_000;
        const CH: usize = 2;
        // Value that identifies its own frame index, so a torn read is visible.
        let expect = |frame: usize| -> [f32; 2] {
            let v = (frame % 4096) as f32;
            [v, -v]
        };

        let (pcm, mut w) = SharedPcm::new(CH, FRAMES, FRAMES);
        let reader = Arc::clone(&pcm);

        let stop = Arc::new(AtomicBool::new(false));
        let reader_stop = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            let mut highest = 0usize;
            let mut reads = 0u64;
            while !reader_stop.load(Ordering::Acquire) || highest < reader.frames_ready() {
                let ready = reader.frames_ready();
                // frames_ready must never run past the allocation.
                assert!(ready <= FRAMES);
                while highest < ready {
                    let got = reader.frame_stereo(highest);
                    assert_eq!(
                        got,
                        expect(highest),
                        "torn or stale frame at {highest} (ready={ready})"
                    );
                    highest += 1;
                    reads += 1;
                }
                std::hint::spin_loop();
            }
            (highest, reads)
        });

        // Writer: irregular chunk sizes, so the reader keeps catching up to a
        // moving edge rather than a nicely aligned one.
        let mut frame = 0usize;
        let mut chunk = 1usize;
        while frame < FRAMES {
            let n = chunk.min(FRAMES - frame);
            let mut buf = Vec::with_capacity(n * CH);
            for i in 0..n {
                let [l, r] = expect(frame + i);
                buf.push(l);
                buf.push(r);
            }
            assert_eq!(w.write_interleaved(&buf), n);
            frame += n;
            chunk = (chunk * 2 + 1) % 977 + 1;
        }
        w.finish();
        stop.store(true, Ordering::Release);

        let (highest, reads) = handle.join().expect("reader thread panicked");
        assert_eq!(highest, FRAMES, "reader did not observe every frame");
        assert_eq!(reads, FRAMES as u64);
        assert!(pcm.is_complete());
    }

    /// A reader that starts before the writer has published anything must see
    /// `frames_ready() == 0` and simply wait - it must never read the zeroed
    /// tail and mistake it for audio.
    #[test]
    fn reader_sees_nothing_before_the_first_publish() {
        let (pcm, mut w) = SharedPcm::new(2, 64, 64);
        let reader = Arc::clone(&pcm);
        let handle = std::thread::spawn(move || {
            while reader.frames_ready() == 0 {
                std::hint::spin_loop();
            }
            reader.frame_stereo(0)
        });
        std::thread::sleep(std::time::Duration::from_millis(5));
        w.write_interleaved(&[0.25, -0.25]);
        assert_eq!(handle.join().unwrap(), [0.25, -0.25]);
    }
}
