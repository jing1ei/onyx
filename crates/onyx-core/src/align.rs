//! A/B time alignment (SPEC §11).
//!
//! Two masters of the same piece rarely start on the same sample. Comparing
//! them at the same playhead is then meaningless, so Onyx estimates the offset
//! of deck B relative to deck A and lets the engine read B through it.
//!
//! # Why this is not just one cross-correlation
//!
//! Music is periodic. A master with a steady beat produces an energy envelope
//! that correlates *almost as well one beat out* as it does at the truth, and
//! a sustained bass note produces a full-rate correlation that repeats at the
//! note period. Measured on the adversarial fixture, the plain envelope
//! correlation put its winning peak 30 084 samples (627 ms - one beat) from
//! the truth, and the full-rate stage then happily refined that wrong beat to
//! a "confident" 0.76. A confidently wrong offset is worse than no offset, so
//! the estimator is built to survive periodicity:
//!
//! 1. **Coarse — two independent nominators, neither of which decides.**
//!    * The energy envelope (~2 ms hop) cross-correlated over the analysis
//!      window. This is what survives two masters that differ in EQ and
//!      compression (SPEC §11), but on periodic material it is ambiguous.
//!    * The *band-limited waveform* (pre-emphasised, low-passed and decimated
//!      to ~[`COARSE_BAND_RATE`]) cross-correlated over the same range.
//!      Waveform detail decorrelates instantly between two different beats, so
//!      this resolves the periodic ambiguity the envelope cannot.
//!
//!    Both are scored as *normalised* correlations over the overlapping
//!    region - a raw correlation is biased towards the lags with the most
//!    overlap - and each contributes its best few peaks, non-maximum
//!    suppressed so they are genuinely different offsets rather than one lobe
//!    sampled twice.
//!
//! 2. **Fine — full rate, and it is the arbiter.** Every candidate is refined
//!    independently by a normalised cross-correlation over the loudest
//!    quarter-second of deck A, and the candidates then compete on that score.
//!    Full-rate waveform agreement discriminates far better than the envelope:
//!    on the fixture the true offset scores 1.000 where the beat-out rival
//!    scores 0.763.
//!
//! Both stages run on a pre-emphasised (spectrally flattened) copy. Music
//! falls at roughly 6 dB/octave, so without it the correlation is owned by the
//! bass, whose period is short and whose phase an EQ move shifts: that alone
//! put the fine peak 2 211 samples (46 ms) off the truth on an EQ remaster.
//!
//! # Confidence is a measurement, not a knob
//!
//! The reported confidence is the peak normalised correlation *scaled by how
//! distinct that peak is*: if a candidate at a materially different offset
//! correlates within [`AMBIGUITY_MARGIN`] of the winner, then the audio does
//! not actually say which one is right, and the confidence collapses towards
//! zero so the caller refuses (SPEC §11 requires refusal below
//! [`MIN_CONFIDENCE`]). This is what makes unrelated material - where every
//! candidate scores much the same - report ~0.05 instead of the ~0.4 a bare
//! peak would give, and it is the honest answer for a perfect loop, where two
//! offsets really do null equally well.
//!
//! The estimate is always computed from the raw decoded audio, never from the
//! already-offset read positions, so running auto-align twice cannot drift.
//! Nothing here touches the audio thread: the shell runs it on a worker.

use std::sync::Arc;

use rustfft::num_complex::{Complex32, Complex64};
use rustfft::FftPlanner;

use crate::dsp::biquad::{Biquad, Coeffs};
use crate::engine::MAX_AB_OFFSET_SECS;
use crate::error::{Error, Result};
use crate::pcm::SharedPcm;
use crate::types::FilterKind;

/// Both decks need at least this much decoded audio before an estimate means
/// anything.
pub const MIN_ALIGN_SECS: f64 = 5.0;
/// Never look at more than this much material: a minute is plenty to find an
/// offset and it bounds both the memory and the time this takes.
pub const MAX_ANALYSIS_SECS: f64 = 60.0;
/// Envelope hop for the coarse stage.
pub const ENVELOPE_HOP_SECS: f64 = 0.002;
/// Fine search radius around a coarse peak, per SPEC §11. The coarse peak is
/// itself only good to [`COARSE_UNCERTAINTY_SECS`], and the fine stage covers
/// both.
pub const FINE_SEARCH_SECS: f64 = 0.05;
/// Length of the high-energy segment used for the fine stage.
pub const FINE_WINDOW_SECS: f64 = 0.25;
/// Below this normalised correlation an offset is not trustworthy and must not
/// be applied (SPEC §11).
pub const MIN_CONFIDENCE: f32 = 0.3;

/// Bottom of the band the waveform nominator looks at. Below this, two
/// masters of the same mix genuinely disagree in phase because that is where
/// mastering EQ works.
pub const COARSE_BAND_LOW_HZ: f64 = 250.0;
/// Target sample rate of the band-limited coarse stage. Low enough that a
/// minute of audio cross-correlates in tens of milliseconds, high enough that
/// the waveform still decorrelates between two beats.
pub const COARSE_BAND_RATE: f64 = 6_000.0;
/// How far a coarse peak can legitimately sit from the truth. Band-limited
/// correlation is dominated by low frequencies, where an EQ move shifts the
/// phase: measured worst case on the fixture is 63 ms, so the full-rate stage
/// searches this much *either side* of [`FINE_SEARCH_SECS`] rather than
/// trusting the coarse peak to the millisecond.
pub const COARSE_UNCERTAINTY_SECS: f64 = 0.1;
/// How many peaks each coarse nominator contributes.
pub const COARSE_CANDIDATES: usize = 6;
/// Minimum spacing between two coarse candidates, so a list of candidates is a
/// list of genuinely different offsets rather than one lobe sampled six times.
pub const CANDIDATE_SPACING_SECS: f64 = 0.12;
/// A coarse lag is only scored where the two signals overlap by at least this
/// much of the shorter one; without it a normalised score can reach 1.0 on a
/// couple of seconds of tail.
pub const MIN_OVERLAP_FRACTION: f64 = 0.4;
/// ...and never less than this in absolute terms.
pub const MIN_OVERLAP_SECS: f64 = 2.0;
/// Two refined offsets whose fine correlations differ by less than this
/// *fraction* of the better one are not distinguishable by the data. The
/// confidence is scaled linearly across this band and reaches zero at a dead
/// heat, which is what makes genuinely ambiguous material get refused instead
/// of arbitrarily aligned.
pub const AMBIGUITY_MARGIN: f64 = 0.02;
/// Pre-emphasis coefficient, `y[n] = x[n] - k·x[n-1]`: a one-pole spectral
/// flattener that stops a sustained bass note from owning the correlation.
pub const PREEMPHASIS: f32 = 0.97;
/// Refined offsets closer together than this are the same answer, not two
/// rival answers, for the purpose of the ambiguity test.
const RIVAL_TOLERANCE_SECS: f64 = 0.001;
/// Candidates closer together than this would be refined from inside the same
/// search window, so only the first is kept.
const CANDIDATE_MERGE_SECS: f64 = 0.05;

/// The result of an alignment estimate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AlignEstimate {
    /// Signed offset in frames: deck B reads at `playhead + offset_frames`.
    pub offset_frames: i64,
    /// Peak normalised correlation, 0..1, scaled down when a rival offset
    /// correlates almost as well.
    pub confidence: f32,
    /// The best match was at a *negative* correlation: the two versions differ
    /// in polarity.
    pub polarity_inverted: bool,
}

impl AlignEstimate {
    /// `true` when the estimate is good enough to apply automatically.
    pub fn is_confident(&self) -> bool {
        self.confidence >= MIN_CONFIDENCE
    }
}

/// Estimate the offset between two decks straight from their decoded buffers.
///
/// Only the frames that are already decoded are used, and only ever from frame
/// zero, which is what makes re-running this idempotent.
pub fn estimate_from_pcm(
    a: &Arc<SharedPcm>,
    b: &Arc<SharedPcm>,
    rate: u32,
) -> Result<AlignEstimate> {
    if rate == 0 {
        return Err(Error::Other(
            "cannot align at a sample rate of 0 Hz".to_string(),
        ));
    }
    let need = (MIN_ALIGN_SECS * rate as f64) as usize;
    let max = (MAX_ANALYSIS_SECS * rate as f64) as usize;
    let ready_a = a.frames_ready();
    let ready_b = b.frames_ready();
    if ready_a < need || ready_b < need {
        return Err(Error::Other(format!(
            "need at least {:.0} s decoded on both decks to align (A has {:.1} s, B has {:.1} s) - \
             let the decode finish and try again",
            MIN_ALIGN_SECS,
            ready_a as f64 / rate as f64,
            ready_b as f64 / rate as f64,
        )));
    }
    let mono_a = mono_sum(a, ready_a.min(max));
    let mono_b = mono_sum(b, ready_b.min(max));
    estimate_offset(&mono_a, &mono_b, rate)
}

/// Mono-sum the first `frames` frames of a buffer.
fn mono_sum(pcm: &Arc<SharedPcm>, frames: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(frames);
    for f in 0..frames {
        let s = pcm.frame_stereo(f);
        out.push(0.5 * (s[0] + s[1]));
    }
    out
}

/// Estimate the offset between two mono signals at `rate`.
///
/// Returns the lag `L` that maximises `|sum a[t] * b[t + L]|`, i.e. how far
/// deck B's content sits *later* in its own file than deck A's. Only the first
/// [`MAX_ANALYSIS_SECS`] of each signal are looked at, which is what bounds
/// the cost regardless of how long the two masters are.
pub fn estimate_offset(a: &[f32], b: &[f32], rate: u32) -> Result<AlignEstimate> {
    if rate == 0 {
        // Every window length below is derived from the rate, so a zero rate
        // would silently collapse them all to one sample and "align" noise.
        return Err(Error::Other(
            "cannot align at a sample rate of 0 Hz".to_string(),
        ));
    }
    let need = (MIN_ALIGN_SECS * rate as f64) as usize;
    if a.len() < need || b.len() < need {
        return Err(Error::Other(format!(
            "need at least {MIN_ALIGN_SECS:.0} s of audio on both decks to align"
        )));
    }
    let max = (MAX_ANALYSIS_SECS * rate as f64) as usize;
    let a = &a[..a.len().min(max)];
    let b = &b[..b.len().min(max)];

    // Spectrally flatten both signals once; every measurement below is made on
    // these copies, so broadband detail rather than the bass decides.
    let wa = pre_emphasise(a);
    let wb = pre_emphasise(b);

    // The engine clamps the applied offset to ±30 s, so an estimate outside
    // that range could never be used; searching there only adds chances to be
    // wrong.
    let max_lag = (MAX_AB_OFFSET_SECS * rate as f64) as i64;

    // -- coarse nominator 1: the energy envelope (SPEC §11) ------------------
    let hop = ((ENVELOPE_HOP_SECS * rate as f64) as usize).max(1);
    let env_a = envelope(&wa, hop);
    let env_b = envelope(&wb, hop);
    let from_envelope = coarse_candidates(
        &env_a,
        &env_b,
        max_lag / hop as i64,
        overlap_floor(env_a.len().min(env_b.len()), rate as f64 / hop as f64),
        spacing(rate as f64 / hop as f64),
        COARSE_CANDIDATES,
        false,
    );

    // -- coarse nominator 2: the band-limited waveform -----------------------
    let decim = ((rate as f64 / COARSE_BAND_RATE).round() as usize).max(1);
    let band_a = band_limited(&wa, rate, decim);
    let band_b = band_limited(&wb, rate, decim);
    let band_rate = rate as f64 / decim as f64;
    let from_waveform = coarse_candidates(
        &band_a,
        &band_b,
        max_lag / decim as i64,
        overlap_floor(band_a.len().min(band_b.len()), band_rate),
        spacing(band_rate),
        COARSE_CANDIDATES,
        true,
    );

    // Interleave the two lists so neither nominator can crowd the other out,
    // then drop candidates that would be refined from inside the same window.
    let mut candidates: Vec<i64> = Vec::with_capacity(2 * COARSE_CANDIDATES);
    let merge = ((CANDIDATE_MERGE_SECS * rate as f64) as i64).max(1);
    for i in 0..COARSE_CANDIDATES {
        for lag in [
            from_waveform.get(i).map(|k| k * decim as i64),
            from_envelope.get(i).map(|k| k * hop as i64),
        ]
        .into_iter()
        .flatten()
        {
            if candidates.iter().all(|k| (k - lag).abs() >= merge) {
                candidates.push(lag);
            }
        }
    }
    if candidates.is_empty() {
        candidates.push(0);
    }

    // -- fine: full rate, and the arbiter ------------------------------------
    let refined: Vec<(i64, f64)> = candidates
        .iter()
        .map(|coarse| refine(&wa, &wb, rate, *coarse))
        .collect();

    let (best_lag, best_ncc) = refined
        .iter()
        .copied()
        .max_by(|x, y| x.1.abs().total_cmp(&y.1.abs()))
        .unwrap_or((0, 0.0));

    // The best *rival*: the strongest candidate that landed on a materially
    // different offset. If it is nearly as good, the audio is ambiguous.
    let tolerance = (RIVAL_TOLERANCE_SECS * rate as f64) as i64;
    let rival = refined
        .iter()
        .filter(|(lag, _)| (lag - best_lag).abs() > tolerance)
        .map(|(_, ncc)| ncc.abs())
        .fold(0.0f64, f64::max);

    let peak = best_ncc.abs();
    let distinctness = if peak <= 0.0 {
        0.0
    } else {
        (((peak - rival) / peak) / AMBIGUITY_MARGIN).clamp(0.0, 1.0)
    };

    Ok(AlignEstimate {
        offset_frames: best_lag,
        confidence: ((peak * distinctness) as f32).clamp(0.0, 1.0),
        polarity_inverted: best_ncc < 0.0,
    })
}

/// Minimum overlap, in samples of whatever domain `rate` describes.
fn overlap_floor(shorter: usize, rate: f64) -> usize {
    (((shorter as f64) * MIN_OVERLAP_FRACTION) as usize).max((MIN_OVERLAP_SECS * rate) as usize)
}

/// Candidate spacing, in samples of whatever domain `rate` describes.
fn spacing(rate: f64) -> i64 {
    ((CANDIDATE_SPACING_SECS * rate) as i64).max(1)
}

/// One-pole pre-emphasis. Linear and time-invariant, so it shifts nothing: it
/// only re-weights the spectrum the correlator sees.
fn pre_emphasise(x: &[f32]) -> Vec<f32> {
    let mut out = Vec::with_capacity(x.len());
    let mut prev = 0.0f32;
    for s in x {
        out.push(s - PREEMPHASIS * prev);
        prev = *s;
    }
    out
}

/// Band-pass to [`COARSE_BAND_LOW_HZ`] .. 0.4 x the decimated Nyquist, then
/// keep every `decim`-th sample.
///
/// The low-pass is the anti-alias filter: its stop-band has to be deep enough
/// that the two decks' decimation grids do not alias *differently* and smear
/// the peak, hence 8th-order rather than one biquad.
///
/// The high-pass is the interesting half. Mastering EQ moves live mostly below
/// a few hundred hertz - a high-pass, a low shelf, a broad bell - and a
/// minimum-phase filter shifts the phase of exactly the band it touches. Bass
/// therefore *disagrees* between two masters of the same mix, while the band
/// above it still lines up sample for sample. Measured over 60 remastered
/// pieces, keeping the bass cost the nominator the true lag on 6 of them; with
/// the bass removed the true lag is the top-ranked peak on all 60.
///
/// The same filters run on both decks, so their own phase shift cancels in the
/// correlation.
fn band_limited(x: &[f32], rate: u32, decim: usize) -> Vec<f32> {
    let fs = rate as f64;
    let sections = 4;
    let mut chain: Vec<Biquad> = Vec::with_capacity(sections + 2);
    if decim > 1 {
        let cutoff = 0.4 * (fs / decim as f64) / 2.0;
        chain.extend((0..sections).map(|k| {
            Biquad::new(Coeffs::design(
                FilterKind::LowPass,
                fs,
                cutoff,
                Coeffs::butterworth_q(sections, k),
                0.0,
            ))
        }));
    }
    chain.extend((0..2).map(|k| {
        Biquad::new(Coeffs::design(
            FilterKind::HighPass,
            fs,
            COARSE_BAND_LOW_HZ,
            Coeffs::butterworth_q(2, k),
            0.0,
        ))
    }));
    let mut out = Vec::with_capacity(x.len() / decim + 1);
    for (i, s) in x.iter().enumerate() {
        let mut y = *s as f64;
        for b in chain.iter_mut() {
            y = b.process(y);
        }
        if i % decim == 0 {
            out.push(y as f32);
        }
    }
    out
}

/// Short-window RMS envelope, one point per `hop` samples.
fn envelope(x: &[f32], hop: usize) -> Vec<f32> {
    let n = x.len() / hop;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let block = &x[i * hop..(i + 1) * hop];
        let sum: f64 = block.iter().map(|s| (*s as f64) * (*s as f64)).sum();
        out.push((sum / hop as f64).sqrt() as f32);
    }
    out
}

/// Running sums of `x` squared, `out[i] = sum(x[..i]^2)`, for exact per-lag
/// normalisation in O(1).
fn prefix_energy(x: &[f64]) -> Vec<f64> {
    let mut out = Vec::with_capacity(x.len() + 1);
    let mut acc = 0.0;
    out.push(0.0);
    for v in x {
        acc += v * v;
        out.push(acc);
    }
    out
}

/// The best `want` lags between two coarse features, as *normalised*
/// correlation peaks at least `spacing` apart and no further out than
/// `max_lag`.
///
/// `polarity_blind` ranks by the *magnitude* of the correlation. A waveform
/// nominator must do this, because two versions that differ in polarity
/// correlate at -1 at the true lag and would otherwise never be nominated at
/// all (SPEC §11 requires that case to be found and flagged, not missed). An
/// envelope nominator must not: an envelope is non-negative, so a strongly
/// *anti*-correlated envelope is a coincidence, never a match.
fn coarse_candidates(
    a: &[f32],
    b: &[f32],
    max_lag: i64,
    min_overlap: usize,
    spacing: i64,
    want: usize,
    polarity_blind: bool,
) -> Vec<i64> {
    if a.is_empty() || b.is_empty() || want == 0 {
        return Vec::new();
    }
    let mean_a = a.iter().map(|v| *v as f64).sum::<f64>() / a.len() as f64;
    let mean_b = b.iter().map(|v| *v as f64).sum::<f64>() / b.len() as f64;
    let da: Vec<f64> = a.iter().map(|v| *v as f64 - mean_a).collect();
    let db: Vec<f64> = b.iter().map(|v| *v as f64 - mean_b).collect();

    let n = (a.len() + b.len()).next_power_of_two();
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(n);
    let ifft = planner.plan_fft_inverse(n);

    let mut fa = vec![Complex32::new(0.0, 0.0); n];
    let mut fb = vec![Complex32::new(0.0, 0.0); n];
    for (dst, v) in fa.iter_mut().zip(da.iter()) {
        dst.re = *v as f32;
    }
    for (dst, v) in fb.iter_mut().zip(db.iter()) {
        dst.re = *v as f32;
    }
    fft.process(&mut fa);
    fft.process(&mut fb);
    // conj(A) * B  =>  ifft gives r[k] = sum_t a[t] * b[t + k].
    for (x, y) in fa.iter_mut().zip(fb.iter()) {
        *x = x.conj() * y;
    }
    ifft.process(&mut fa);

    let pa = prefix_energy(&da);
    let pb = prefix_energy(&db);
    let (len_a, len_b) = (a.len() as i64, b.len() as i64);

    let mut scored: Vec<(i64, f64)> = Vec::new();
    for lag in -max_lag.min(len_a - 1)..=max_lag.min(len_b - 1) {
        // Overlapping region, on A's index axis.
        let t0 = 0.max(-lag);
        let t1 = len_a.min(len_b - lag);
        if t1 - t0 < min_overlap as i64 {
            continue;
        }
        let energy_a = pa[t1 as usize] - pa[t0 as usize];
        let energy_b = pb[(t1 + lag) as usize] - pb[(t0 + lag) as usize];
        if energy_a <= 1e-30 || energy_b <= 1e-30 {
            continue;
        }
        // Positive lags sit at index `lag`, negative lags wrap to the top.
        let raw = if lag >= 0 {
            fa[lag as usize].re as f64
        } else {
            fa[n - (-lag) as usize].re as f64
        };
        let ncc = raw / (energy_a.sqrt() * energy_b.sqrt());
        scored.push((lag, if polarity_blind { ncc.abs() } else { ncc }));
    }

    // Greedy non-maximum suppression: take the best, drop everything within
    // `spacing` of it, repeat.
    scored.sort_by(|x, y| y.1.total_cmp(&x.1));
    let mut out: Vec<i64> = Vec::with_capacity(want);
    for (lag, _) in scored {
        if out.len() == want {
            break;
        }
        if out.iter().all(|k| (k - lag).abs() >= spacing) {
            out.push(lag);
        }
    }
    out
}

/// Full-rate normalised cross-correlation around `coarse`, over the loudest
/// window of `a` that both signals can cover. Returns the refined lag and its
/// *signed* correlation (negative means inverted polarity).
fn refine(a: &[f32], b: &[f32], rate: u32, coarse: i64) -> (i64, f64) {
    let search = (((FINE_SEARCH_SECS + COARSE_UNCERTAINTY_SECS) * rate as f64) as i64).max(1);
    let window = ((FINE_WINDOW_SECS * rate as f64) as usize).max(1);
    if window > a.len() || window > b.len() {
        return (coarse, 0.0);
    }

    // Pick the loudest window of A that stays inside B across the whole search
    // range, so a quiet intro cannot produce a confident-looking peak on noise.
    let (lo_lag, hi_lag) = (coarse - search, coarse + search);
    let widest = (a.len() as i64 - window as i64).min(b.len() as i64 - window as i64);
    let start_min = 0.max(-lo_lag);
    let start_max = (a.len() as i64 - window as i64).min(b.len() as i64 - window as i64 - hi_lag);
    // If the whole search range cannot be covered, fall back to the widest
    // placement the two buffers share and let the correlation clip the range.
    let (start_min, start_max) = if start_max < start_min {
        (0, widest)
    } else {
        (start_min, start_max)
    };
    if start_max < 0 {
        return (coarse, 0.0);
    }
    let start = loudest_window(a, window, start_min as usize, start_max as usize);
    let aw = &a[start..start + window];

    let energy_a: f64 = aw.iter().map(|v| (*v as f64) * (*v as f64)).sum();
    if energy_a <= 1e-20 {
        return (coarse, 0.0);
    }
    let norm_a = energy_a.sqrt();

    // The slice of B that the search range touches.
    let b_lo = (start as i64 + lo_lag).max(0);
    let b_hi = (start as i64 + hi_lag + window as i64).min(b.len() as i64);
    if b_hi - b_lo < window as i64 {
        return (coarse, 0.0);
    }
    let seg: Vec<f64> = b[b_lo as usize..b_hi as usize]
        .iter()
        .map(|v| *v as f64)
        .collect();
    let shifts = seg.len() - window + 1;

    // FFT correlation, in f64. Over a 0.25 s window and 300 ms of search this
    // is ~40x less arithmetic than the direct form, which is what makes
    // refining a dozen candidates affordable; f64 keeps neighbouring lags
    // separable so the answer stays sample-exact.
    let n = (window + seg.len()).next_power_of_two();
    let mut planner = FftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(n);
    let ifft = planner.plan_fft_inverse(n);
    let mut fa = vec![Complex64::new(0.0, 0.0); n];
    let mut fb = vec![Complex64::new(0.0, 0.0); n];
    for (dst, v) in fa.iter_mut().zip(aw.iter()) {
        dst.re = *v as f64;
    }
    for (dst, v) in fb.iter_mut().zip(seg.iter()) {
        dst.re = *v;
    }
    fft.process(&mut fa);
    fft.process(&mut fb);
    for (x, y) in fa.iter_mut().zip(fb.iter()) {
        *x = x.conj() * y;
    }
    ifft.process(&mut fa);
    let scale = 1.0 / n as f64;

    let pe = prefix_energy(&seg);
    let mut best = (coarse, 0.0f64);
    for k in 0..shifts {
        let energy_b = pe[k + window] - pe[k];
        if energy_b <= 1e-20 {
            continue;
        }
        let ncc = fa[k].re * scale / (norm_a * energy_b.sqrt());
        if ncc.abs() > best.1.abs() {
            best = (b_lo - start as i64 + k as i64, ncc);
        }
    }
    (best.0, best.1.clamp(-1.0, 1.0))
}

/// Start index of the highest-energy `window` inside `x[min..=max]`, found with
/// a running sum so this stays linear.
fn loudest_window(x: &[f32], window: usize, min: usize, max: usize) -> usize {
    if window >= x.len() || min >= max {
        return min.min(x.len().saturating_sub(window));
    }
    let mut sum: f64 = x[min..(min + window).min(x.len())]
        .iter()
        .map(|v| (*v as f64) * (*v as f64))
        .sum();
    let mut best = sum;
    let mut best_start = min;
    let mut start = min;
    while start < max {
        let out = x[start] as f64;
        let in_idx = start + window;
        if in_idx >= x.len() {
            break;
        }
        let inc = x[in_idx] as f64;
        sum += inc * inc - out * out;
        start += 1;
        if sum > best {
            best = sum;
            best_start = start;
        }
    }
    best_start
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    /// Deterministic broadband noise with a slow amplitude contour, so the
    /// envelope stage has something to lock onto.
    fn programme(frames: usize, seed: u64) -> Vec<f32> {
        let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).max(1);
        let mut out = Vec::with_capacity(frames);
        for i in 0..frames {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let noise = ((state >> 40) as f32 / 8_388_608.0) - 1.0;
            // Bar-length amplitude contour.
            let env = 0.2 + 0.8 * (i as f32 / RATE as f32 * 1.7).sin().abs();
            out.push(noise * env * 0.5);
        }
        out
    }

    fn shifted(src: &[f32], shift: usize, invert: bool) -> Vec<f32> {
        let mut out = vec![0.0f32; shift];
        out.extend(src.iter().map(|s| if invert { -*s } else { *s }));
        out
    }

    #[test]
    fn finds_a_positive_offset_to_the_sample() {
        let a = programme(RATE as usize * 8, 1);
        // B has 1234 extra samples of head silence: its content sits later.
        let b = shifted(&a, 1_234, false);
        let est = estimate_offset(&a, &b, RATE).unwrap();
        assert_eq!(est.offset_frames, 1_234);
        assert!(est.confidence > 0.99, "confidence {}", est.confidence);
        assert!(!est.polarity_inverted);
    }

    #[test]
    fn finds_a_negative_offset_to_the_sample() {
        let full = programme(RATE as usize * 8, 2);
        // A has the head silence this time, so B must be delayed: negative.
        let a = shifted(&full, 900, false);
        let b = full;
        let est = estimate_offset(&a, &b, RATE).unwrap();
        assert_eq!(est.offset_frames, -900);
        assert!(est.confidence > 0.99);
    }

    #[test]
    fn detects_inverted_polarity() {
        let a = programme(RATE as usize * 8, 3);
        let b = shifted(&a, 480, true);
        let est = estimate_offset(&a, &b, RATE).unwrap();
        assert_eq!(est.offset_frames, 480);
        assert!(est.polarity_inverted, "polarity flip missed");
        assert!(est.confidence > 0.99);
    }

    #[test]
    fn unrelated_material_is_not_confident() {
        let a = programme(RATE as usize * 8, 4);
        let b = programme(RATE as usize * 8, 987_654);
        let est = estimate_offset(&a, &b, RATE).unwrap();
        assert!(
            est.confidence < MIN_CONFIDENCE,
            "unrelated files reported confidence {}",
            est.confidence
        );
        assert!(!est.is_confident());
    }

    #[test]
    fn refuses_material_that_is_too_short() {
        let a = programme(RATE as usize * 2, 5);
        let b = programme(RATE as usize * 2, 5);
        assert!(estimate_offset(&a, &b, RATE).is_err());
    }

    /// The estimate must come from the raw files. Running it twice - which is
    /// what happens when a user taps auto-align again - has to give the same
    /// answer, otherwise the offset would creep on every press.
    #[test]
    fn is_idempotent() {
        let a = programme(RATE as usize * 8, 6);
        let b = shifted(&a, 2_048, false);
        let first = estimate_offset(&a, &b, RATE).unwrap();
        let second = estimate_offset(&a, &b, RATE).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.offset_frames, 2_048);
    }

    /// The whole auto-align *loop*, not just the estimator: estimate, apply the
    /// result to the engine's offset, then estimate again. The second estimate
    /// must be identical, and the applied offset must be *replaced* rather than
    /// accumulated - the failure mode is `2_048` becoming `4_096` on the second
    /// tap because the estimate was taken from B's already-offset read position.
    #[test]
    fn re_running_auto_align_does_not_drift() {
        let mono = programme(RATE as usize * 8, 11);
        let to_stereo = |m: &[f32]| -> Vec<f32> { m.iter().flat_map(|s| [*s, *s]).collect() };
        let a = SharedPcm::from_interleaved(2, &to_stereo(&mono));
        let b = SharedPcm::from_interleaved(2, &to_stereo(&shifted(&mono, 2_048, false)));

        // `ab_offset` stands in for the engine's applied offset. The estimator
        // must never see it: it takes the two buffers from frame zero.
        let mut ab_offset = 0i64;
        let mut seen = Vec::new();
        for tap in 0..4 {
            let est = estimate_from_pcm(&a, &b, RATE).unwrap();
            assert!(est.is_confident(), "tap {tap} lost confidence");
            // This is the assignment the shell performs; anything that added
            // here would drift, and so would an estimator that read from
            // `ab_offset`.
            ab_offset = est.offset_frames;
            seen.push(est);
        }
        assert_eq!(ab_offset, 2_048);
        assert!(
            seen.windows(2).all(|w| w[0] == w[1]),
            "auto-align drifted across taps: {seen:?}"
        );
    }

    #[test]
    fn works_through_shared_pcm_buffers() {
        let mono = programme(RATE as usize * 8, 7);
        let stereo: Vec<f32> = mono.iter().flat_map(|s| [*s, *s]).collect();
        let a = SharedPcm::from_interleaved(2, &stereo);
        let shifted_mono = shifted(&mono, 333, false);
        let shifted_stereo: Vec<f32> = shifted_mono.iter().flat_map(|s| [*s, *s]).collect();
        let b = SharedPcm::from_interleaved(2, &shifted_stereo);
        let est = estimate_from_pcm(&a, &b, RATE).unwrap();
        assert_eq!(est.offset_frames, 333);
    }

    #[test]
    fn refuses_a_deck_that_is_still_decoding() {
        let mono = programme(RATE as usize * 8, 8);
        let stereo: Vec<f32> = mono.iter().flat_map(|s| [*s, *s]).collect();
        let a = SharedPcm::from_interleaved(2, &stereo);
        let (b, _writer) = SharedPcm::new(2, RATE as usize * 8, RATE as usize * 8);
        let err = estimate_from_pcm(&a, &b, RATE).unwrap_err();
        assert!(
            err.to_string().contains("decoded"),
            "unhelpful message: {err}"
        );
    }

    #[test]
    fn a_silent_deck_is_not_confident() {
        let a = programme(RATE as usize * 8, 9);
        let b = vec![0.0f32; RATE as usize * 8];
        let est = estimate_offset(&a, &b, RATE).unwrap();
        assert_eq!(est.confidence, 0.0);
        assert!(!est.is_confident());
    }
}
