//! Onyx audio core.
//!
//! A self-contained, real-time-safe audio playback / comparison / metering
//! engine. The crate deliberately knows nothing about the UI shell so that it
//! can be unit tested (and reused) on its own.
//!
//! Layout:
//! - [`align`]    A/B time-alignment estimation (envelope + full-rate NCC)
//! - [`pcm`]      lock-free, progressively-filled PCM buffer (play while decoding)
//! - [`decode`]   Symphonia-backed decoder thread + sample-rate conversion
//! - [`waveform`] multi-resolution peak pyramid used by the waveform lanes
//! - [`dsp`]      biquads, parametric EQ, ITU-R BS.1770-4 loudness, true peak, FFT
//! - [`engine`]   CPAL output stream, dual decks, transport, A/B switching
//!
//! Design notes that matter for "professional, fast, accurate":
//! - The output callback never allocates, never locks and never blocks.
//! - Playback starts as soon as the first ~150 ms of audio is decoded.
//! - The engine prefers to run the output device at the *source* sample rate so
//!   that no resampling is applied to deck A (bit-transparent path).
//! - All metering is measured on the real post-EQ output bus, not on the file.

pub mod align;
pub mod container;
pub mod deck;
pub mod decode;
pub mod dsp;
pub mod engine;
pub mod error;
pub mod midi;
/// Opus decoder shim registered into [`decode::codecs`]. Not public: callers
/// reach it through the ordinary decode path.
mod opus;
pub mod pcm;
pub mod types;
pub mod waveform;

pub use error::{Error, Result};
pub use types::*;

/// Convert a linear amplitude to dBFS, floored at [`MIN_DB`].
///
/// A non-finite input reads as the floor rather than propagating a NaN into a
/// meter read-out.
#[inline]
pub fn lin_to_db(v: f32) -> f32 {
    if !v.is_finite() || v <= 1.0e-7 {
        MIN_DB
    } else {
        20.0 * v.log10()
    }
}

/// Convert dB to a linear amplitude. A non-finite input is silence, so a NaN
/// can never reach a gain stage.
#[inline]
pub fn db_to_lin(db: f32) -> f32 {
    if !db.is_finite() || db <= MIN_DB {
        0.0
    } else {
        10f32.powf(db / 20.0)
    }
}

/// Meter floor. Anything quieter is reported as this value.
pub const MIN_DB: f32 = -144.0;

/// Loudness floor used by the LUFS read-outs (matches EBU practice).
pub const LUFS_SILENCE: f32 = -70.0;

/// A pass-through global allocator that can count allocations on the calling
/// thread. Only compiled for the test binary; it exists so that
/// `engine::tests::the_callback_never_allocates` can *prove* the claim in the
/// module docs instead of asserting it in a comment.
#[cfg(test)]
pub(crate) mod test_alloc {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    thread_local! {
        /// Counting is opt-in per thread, so the rest of the suite (which runs
        /// in parallel on other threads) is unaffected.
        static ARMED: Cell<bool> = const { Cell::new(false) };
        static HITS: Cell<usize> = const { Cell::new(0) };
    }

    // NOTE: both TLS slots hold a `Cell` of a `Copy` type with no destructor,
    // so touching them from inside the allocator cannot itself allocate (no
    // lazy destructor registration happens).
    #[inline]
    fn hit() {
        let _ = ARMED.try_with(|armed| {
            if armed.get() {
                let _ = HITS.try_with(|h| h.set(h.get() + 1));
            }
        });
    }

    pub struct Counting;

    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            hit();
            unsafe { System.alloc(layout) }
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            hit();
            unsafe { System.dealloc(ptr, layout) }
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            hit();
            unsafe { System.alloc_zeroed(layout) }
        }
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            hit();
            unsafe { System.realloc(ptr, layout, new_size) }
        }
    }

    /// Run `f` with allocation counting armed and return how many calls into
    /// the allocator (alloc **or** dealloc) it made.
    pub fn count_allocations<R>(f: impl FnOnce() -> R) -> (R, usize) {
        HITS.with(|h| h.set(0));
        ARMED.with(|a| a.set(true));
        let out = f();
        ARMED.with(|a| a.set(false));
        (out, HITS.with(|h| h.get()))
    }
}

#[cfg(test)]
#[global_allocator]
static GLOBAL_TEST_ALLOC: test_alloc::Counting = test_alloc::Counting;

/// A `log` implementation for the test binary that *allocates on purpose*.
///
/// Without a logger installed, `log::warn!` is a level check and a branch: it
/// never reaches the allocator, so the allocation-counting tests would happily
/// pass with a `log::warn!` sitting in the middle of the audio callback. This
/// logger is installed by those tests (and turned up to `Trace`) so that any
/// `log::` call on the real-time path formats into a `String` — which the
/// counting allocator sees, and which fails the test.
#[cfg(test)]
pub(crate) mod test_log {
    use std::cell::Cell;
    use std::sync::Once;

    thread_local! {
        /// Counted per thread, exactly like the allocation counter above. A
        /// process-global count would be useless here: `cargo test` runs the
        /// suite on a thread pool, and the decode tests legitimately log
        /// warnings while an allocation test is measuring, so a global
        /// before/after pair fails at random.
        ///
        /// `Cell<usize>` is `Copy` and has no destructor, so touching the slot
        /// from inside `log()` cannot itself allocate.
        static RECORDS: Cell<usize> = const { Cell::new(0) };
    }

    struct AllocatingLogger;

    impl log::Log for AllocatingLogger {
        fn enabled(&self, _: &log::Metadata) -> bool {
            true
        }
        fn log(&self, record: &log::Record) {
            // Exactly what a real logger does, and exactly what the real-time
            // path must never trigger: format the message into a fresh String.
            let rendered = format!("[{}] {}", record.level(), record.args());
            if !rendered.is_empty() {
                let _ = RECORDS.try_with(|r| r.set(r.get() + 1));
            }
        }
        fn flush(&self) {}
    }

    /// Install the logger once per test binary and turn every level on.
    pub fn install() {
        static LOGGER: AllocatingLogger = AllocatingLogger;
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            // `set_logger`, not `set_boxed_logger`: the latter lives behind the
            // `log/alloc` feature, which this crate does not ask for and only
            // happens to get through workspace feature unification — so using it
            // broke a plain `cargo test -p onyx-core`.
            // If something else got there first the assertions below still
            // hold: what matters is that *a* logger is live.
            let _ = log::set_logger(&LOGGER);
            log::set_max_level(log::LevelFilter::Trace);
        });
        assert!(
            log::log_enabled!(log::Level::Trace),
            "the test logger is not live, so this test would prove nothing"
        );
    }

    /// Records emitted **on this thread** so far. Tests compare a before/after
    /// pair around the code under test rather than an absolute value.
    pub fn count() -> usize {
        RECORDS.with(|r| r.get())
    }
}
