//! Digital signal processing building blocks.
//!
//! Everything here is `no_std`-friendly in spirit: no allocation in the hot
//! paths, `f64` state where numerical stability matters (biquads, loudness
//! integrators) and `f32` at the boundaries.

pub mod biquad;
pub mod eq;
pub mod loudness;
pub mod meters;
pub mod spectrum;
pub mod truepeak;

/// One-pole smoothing coefficient for a given time constant.
#[inline]
pub fn one_pole_coeff(time_secs: f32, sample_rate: f32) -> f32 {
    if time_secs <= 0.0 || sample_rate <= 0.0 {
        return 1.0;
    }
    1.0 - (-1.0 / (time_secs * sample_rate)).exp()
}
