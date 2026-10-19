//! EQ validation against the closed-form RBJ designs, SPEC §12.
//!
//! Reference: Robert Bristow-Johnson, "Cookbook formulae for audio equalizer
//! biquad filter coefficients" (the "RBJ audio EQ cookbook"), archived at
//! <https://www.w3.org/TR/audio-eq-cookbook/>. The cookbook gives, for every
//! shape, both the difference-equation coefficients *and* the analogue
//! prototype `H(s)` they are the bilinear transform of. This file checks the
//! implementation against the **analogue prototype**, which is genuinely
//! independent evidence: transcribing the same coefficient lines a second time
//! would only prove that the copy was faithful.
//!
//! The bridge between the two is exact rather than approximate. RBJ pre-warps
//! at `f0` (`alpha = sin(w0)/2Q`), so the digital filter is the bilinear image
//! of the prototype under
//!
//! ```text
//!     Omega(f) = tan(pi f / fs) / tan(pi f0 / fs)
//! ```
//!
//! and `|H_digital(f)| == |H_analog(j Omega(f))|` must hold to numerical
//! precision at *every* frequency, including up against Nyquist where the
//! warping is severe. That is the property tested here, to 1e-6 dB.
//!
//! Everything else is measured: sine through the real `Biquad`/`StereoEq` and
//! back out, compared against the same closed form.

use onyx_core::dsp::biquad::{Biquad, Coeffs};
use onyx_core::dsp::eq::{curve_db, EqSetting, StereoEq};
use onyx_core::types::{EqBand, EqConfig, FilterKind};
use std::f64::consts::{FRAC_1_SQRT_2, PI};

/* ── the closed form ─────────────────────────────────────────────────────── */

/// `|H(j*omega)|` of the cookbook analogue prototype, normalised to `w0 = 1`.
///
/// Transcribed from the cookbook's "H(s) =" lines, not from its coefficient
/// lines.
fn analog_mag(kind: FilterKind, omega: f64, q: f64, gain_db: f64) -> f64 {
    let s = rustfft::num_complex::Complex64::new(0.0, omega);
    let a = 10f64.powf(gain_db / 40.0);
    let sq_a = a.sqrt();
    let h = match kind {
        // H(s) = 1 / (s^2 + s/Q + 1)
        FilterKind::LowPass => 1.0 / (s * s + s / q + 1.0),
        // H(s) = s^2 / (s^2 + s/Q + 1)
        FilterKind::HighPass => s * s / (s * s + s / q + 1.0),
        // H(s) = (s/Q) / (s^2 + s/Q + 1)   -- constant 0 dB peak gain
        FilterKind::BandPass => (s / q) / (s * s + s / q + 1.0),
        // H(s) = (s^2 + 1) / (s^2 + s/Q + 1)
        FilterKind::Notch => (s * s + 1.0) / (s * s + s / q + 1.0),
        // H(s) = (s^2 + s*(A/Q) + 1) / (s^2 + s/(A*Q) + 1)
        FilterKind::Bell => (s * s + s * (a / q) + 1.0) / (s * s + s / (a * q) + 1.0),
        // H(s) = A * (s^2 + (sqrt(A)/Q)s + A) / (A*s^2 + (sqrt(A)/Q)s + 1)
        FilterKind::LowShelf => {
            a * (s * s + (sq_a / q) * s + a) / (a * (s * s) + (sq_a / q) * s + 1.0)
        }
        // H(s) = A * (A*s^2 + (sqrt(A)/Q)s + 1) / (s^2 + (sqrt(A)/Q)s + A)
        FilterKind::HighShelf => {
            a * (a * (s * s) + (sq_a / q) * s + 1.0) / (s * s + (sq_a / q) * s + a)
        }
    };
    h.norm()
}

/// The exact bilinear image of the prototype: what the digital filter designed
/// by RBJ at `f0` *must* do at `f`.
fn expected_db(kind: FilterKind, fs: f64, f0: f64, f: f64, q: f64, gain_db: f64) -> f64 {
    let omega = (PI * f / fs).tan() / (PI * f0 / fs).tan();
    20.0 * analog_mag(kind, omega, q, gain_db).log10()
}

/// The frequency `Coeffs::design` actually uses after its own clamping.
fn design_freq(fs: f64, freq: f64) -> f64 {
    freq.clamp(1.0, fs * 0.5 * 0.995)
}

const ALL_KINDS: [FilterKind; 7] = [
    FilterKind::Bell,
    FilterKind::LowShelf,
    FilterKind::HighShelf,
    FilterKind::LowPass,
    FilterKind::HighPass,
    FilterKind::Notch,
    FilterKind::BandPass,
];

/* ── 1. coefficients vs the analogue prototype ───────────────────────────── */

/// Every shape, every rate Onyx runs at, across the whole documented parameter
/// range (SPEC §12: freq 20 Hz..20 kHz, gain +/-30 dB, Q 0.1..40).
#[test]
fn every_filter_kind_matches_its_analogue_prototype() {
    let mut worst = 0.0f64;
    let mut worst_case = String::new();
    for &fs in &[
        44_100.0f64,
        48_000.0,
        88_200.0,
        96_000.0,
        176_400.0,
        192_000.0,
    ] {
        for &kind in &ALL_KINDS {
            for &f0 in &[20.0f64, 100.0, 1_000.0, 5_000.0, 20_000.0, fs * 0.49] {
                if f0 >= fs * 0.5 {
                    continue;
                }
                for &q in &[0.1f64, 0.5, FRAC_1_SQRT_2, 1.0, 4.0, 40.0] {
                    let gains: &[f64] = if kind.has_gain() {
                        &[-30.0, -12.0, -3.0, 3.0, 12.0, 30.0]
                    } else {
                        &[0.0]
                    };
                    for &g in gains {
                        let c = Coeffs::design(kind, fs, f0, q, g);
                        let fd = design_freq(fs, f0);
                        let mut f = 5.0;
                        while f < fs * 0.4999 {
                            let got = c.magnitude_db(fs, f);
                            let want = expected_db(kind, fs, fd, f, q, g);
                            // The -120 dB floor in `magnitude_db` legitimately
                            // clips the notch null and the HP/LP zeros.
                            if want > -119.0 {
                                let e = (got - want).abs();
                                if e > worst {
                                    worst = e;
                                    worst_case =
                                        format!("{kind:?} fs={fs} f0={f0} q={q} g={g} at {f} Hz");
                                }
                            }
                            f *= 1.05;
                        }
                    }
                }
            }
        }
    }
    println!("worst |coeffs - RBJ analogue prototype| = {worst:.3e} dB ({worst_case})");
    assert!(
        worst < 1e-6,
        "coefficients depart from the cookbook prototype by {worst} dB ({worst_case})"
    );
}

/// The named points the cookbook guarantees, to the last decimal a reader could
/// check by hand.
#[test]
fn cookbook_reference_points() {
    let fs = 48_000.0;

    // A bell is exactly `gain_db` at f0, whatever Q or f0.
    for &f0 in &[30.0, 1_000.0, 19_000.0] {
        for &q in &[0.1, 1.0, 40.0] {
            for &g in &[-30.0, -6.0, 6.0, 30.0] {
                let c = Coeffs::design(FilterKind::Bell, fs, f0, q, g);
                assert!((c.magnitude_db(fs, f0) - g).abs() < 1e-9);
            }
        }
    }

    // A shelf is exactly half its gain at f0 (the "midpoint" definition).
    for kind in [FilterKind::LowShelf, FilterKind::HighShelf] {
        for &g in &[-24.0, -6.0, 6.0, 24.0] {
            for &q in &[0.2, FRAC_1_SQRT_2, 3.0] {
                let c = Coeffs::design(kind, fs, 800.0, q, g);
                assert!(
                    (c.magnitude_db(fs, 800.0) - g / 2.0).abs() < 1e-9,
                    "{kind:?} q={q} g={g}: {} dB at f0",
                    c.magnitude_db(fs, 800.0)
                );
            }
        }
    }

    // Q = 1/sqrt(2) puts a low-/high-pass exactly -10*log10(2) dB at the corner,
    // and a band-pass is exactly 0 dB at its centre for any Q.
    for kind in [FilterKind::LowPass, FilterKind::HighPass] {
        let c = Coeffs::design(kind, fs, 1_000.0, FRAC_1_SQRT_2, 0.0);
        assert!((c.magnitude_db(fs, 1_000.0) + 3.010_299_956_639_812).abs() < 1e-9);
    }
    for &q in &[0.1, 1.0, 12.0] {
        let c = Coeffs::design(FilterKind::BandPass, fs, 1_000.0, q, 0.0);
        assert!(c.magnitude_db(fs, 1_000.0).abs() < 1e-9);
    }

    // The notch is a true zero on the unit circle: the read-out floors at -120.
    let c = Coeffs::design(FilterKind::Notch, fs, 1_000.0, 4.0, 0.0);
    assert!(c.magnitude_db(fs, 1_000.0) <= -119.0);
}

/// The RBJ shelf Q is a real Q: `1/sqrt(2)` is the steepest monotonic shelf and
/// anything above it overshoots by a known amount. Before this was fixed the
/// engine used `beta = sqrt(A/S)*sin(w0)`, which is not a cookbook form: it
/// delivered `Q_eff = sqrt(q)` and silently clamped `q` at 2.0, so the top of
/// the advertised 0.1..40 range did nothing and the shelf disagreed with the
/// curve the UI draws by up to 28 dB.
#[test]
fn shelf_q_has_the_rbj_meaning() {
    let fs = 48_000.0;
    let f: Vec<f64> = (0..2_000).map(|i| 10.0 * 1.004f64.powi(i)).collect();
    for kind in [FilterKind::LowShelf, FilterKind::HighShelf] {
        for &g in &[6.0f64, 12.0, -12.0] {
            let flat = Coeffs::design(kind, fs, 500.0, FRAC_1_SQRT_2, g);
            let over: f64 = f
                .iter()
                .filter(|f| **f < fs * 0.4)
                .map(|f| {
                    let m = flat.magnitude_db(fs, *f);
                    if g > 0.0 {
                        m - g
                    } else {
                        g - m
                    }
                })
                .fold(f64::NEG_INFINITY, f64::max);
            assert!(
                over < 1e-6,
                "{kind:?} g={g} at Q=1/sqrt(2) overshoots by {over} dB - not monotonic"
            );
        }
        // Q = 40 has to be reachable and audibly resonant.
        let sharp = Coeffs::design(kind, fs, 500.0, 40.0, 12.0);
        let peak = f
            .iter()
            .filter(|f| **f < fs * 0.4)
            .map(|f| sharp.magnitude_db(fs, *f))
            .fold(f64::NEG_INFINITY, f64::max);
        println!("{kind:?} 12 dB shelf at Q=40 peaks at {peak:.3} dB");
        assert!(peak > 25.0, "{kind:?} Q=40 shelf only reached {peak} dB");
    }
}

/* ── 2. measured, through the filters ────────────────────────────────────── */

/// Steady-state gain of a real `Biquad`, in dB.
///
/// `freq` is an integer number of cycles in the measurement window, so the RMS
/// of a settled sine is exactly `A/sqrt(2)` with no leakage and no crest-miss.
fn measure_biquad_db(c: Coeffs, fs: f64, freq: f64) -> f64 {
    let settle = (4.0 * fs) as usize;
    let measure = fs as usize;
    let mut bq = Biquad::new(c);
    for i in 0..settle {
        bq.process((2.0 * PI * freq * i as f64 / fs).sin());
    }
    let mut sum = 0.0f64;
    for i in settle..settle + measure {
        let y = bq.process((2.0 * PI * freq * i as f64 / fs).sin());
        sum += y * y;
    }
    20.0 * (2.0 * sum / measure as f64).sqrt().log10()
}

#[test]
fn measured_response_matches_the_closed_form() {
    let fs = 48_000.0;
    let mut worst = 0.0f64;
    for &kind in &ALL_KINDS {
        for &(f0, q, g) in &[
            (100.0f64, 0.7f64, 6.0f64),
            (1_000.0, 2.0, -9.0),
            (1_000.0, 0.3, 12.0),
            (6_000.0, 8.0, -3.0),
            (15_000.0, 1.0, 4.0),
        ] {
            let g = if kind.has_gain() { g } else { 0.0 };
            let c = Coeffs::design(kind, fs, f0, q, g);
            for &probe in &[50.0f64, 200.0, 997.0, 3_000.0, 9_000.0, 17_000.0] {
                let want = expected_db(kind, fs, f0, probe, q, g);
                if want < -80.0 {
                    continue; // near a null the measurement is noise-limited
                }
                let got = measure_biquad_db(c, fs, probe);
                let e = (got - want).abs();
                assert!(
                    e < 0.002,
                    "{kind:?} f0={f0} q={q} g={g} at {probe} Hz: measured {got:.6} dB, \
                     closed form {want:.6} dB"
                );
                worst = worst.max(e);
            }
        }
    }
    println!("worst |measured - closed form| through a real biquad = {worst:.2e} dB");
}

/* ── 3. cascaded slopes ──────────────────────────────────────────────────── */

fn sine(freq: f64, fs: f64, frames: usize) -> Vec<f32> {
    (0..frames)
        .flat_map(|i| {
            let s = (2.0 * PI * freq * i as f64 / fs).sin() as f32;
            [s, s]
        })
        .collect()
}

/// Gain in dB measured through the whole `StereoEq`, i.e. the audio path.
fn measure_eq_db(cfg: &EqConfig, fs: f64, freq: f64) -> f64 {
    let mut eq = StereoEq::new(fs);
    eq.apply(&EqSetting::from_config(cfg));
    let frames = (2.0 * fs) as usize;
    let mut buf = sine(freq, fs, frames);
    eq.process(&mut buf);
    // Second half only: past the 8 ms topology crossfade and the transient.
    let tail = &buf[frames..];
    let sum: f64 = tail
        .iter()
        .step_by(2)
        .map(|s| (*s as f64) * (*s as f64))
        .sum();
    let n = tail.len() / 2;
    20.0 * (2.0 * sum / n as f64).sqrt().log10()
}

fn hp_lp_band(kind: FilterKind, f0: f32, slope: u8, q: f32) -> EqConfig {
    EqConfig {
        enabled: true,
        bands: vec![EqBand {
            kind,
            slope_db_oct: slope,
            ..EqBand::new(1, kind, f0, 0.0, q)
        }],
    }
}

/// A Butterworth cascade of order `2*sections`, in the warped domain, is the
/// external reference for the slope claim. One octave out from a 1 kHz corner
/// the ideal analogue figures are 12.30 / 24.10 / 48.16 dB, not 12 / 24 / 48 -
/// and two octaves out 24.08 / 48.16 / 96.33 dB. The bilinear warping then
/// moves them by a further few hundredths at 48 kHz. Both effects are in the
/// closed form below, so the tolerance can be tight.
#[test]
fn cascaded_slopes_measure_butterworth() {
    let fs = 48_000.0;
    for (kind, f0) in [
        (FilterKind::HighPass, 1_000.0f64),
        (FilterKind::LowPass, 1_000.0),
    ] {
        for (slope, sections) in [(12u8, 1usize), (24, 2), (48, 4)] {
            let cfg = hp_lp_band(kind, f0 as f32, slope, FRAC_1_SQRT_2 as f32);
            let order = 2 * sections;
            for octaves in [1.0f64, 2.0] {
                let probe = if kind == FilterKind::HighPass {
                    f0 / 2f64.powf(octaves)
                } else {
                    f0 * 2f64.powf(octaves)
                };
                // Butterworth magnitude at the warped frequency ratio.
                let omega = (PI * probe / fs).tan() / (PI * f0 / fs).tan();
                let want = if kind == FilterKind::HighPass {
                    -10.0 * (1.0 + omega.powi(-2 * order as i32)).log10()
                } else {
                    -10.0 * (1.0 + omega.powi(2 * order as i32)).log10()
                };
                let got = measure_eq_db(&cfg, fs, probe);
                println!(
                    "{kind:?} {slope} dB/oct, {octaves} octave(s) out: measured {got:.3} dB, \
                     Butterworth {want:.3} dB"
                );
                assert!(
                    (got - want).abs() < 0.05,
                    "{kind:?} {slope} dB/oct at {probe} Hz: measured {got:.4} dB, \
                     order-{order} Butterworth is {want:.4} dB"
                );
            }
            // Every cascade is exactly -3.0103 dB at its own corner.
            let corner = measure_eq_db(&cfg, fs, f0);
            assert!(
                (corner + 3.010_3).abs() < 0.02,
                "{kind:?} {slope} dB/oct is {corner:.4} dB at the corner"
            );
        }
    }
}

/* ── 4. the curve the UI draws ───────────────────────────────────────────── */

/// SPEC §12: "the composite is computed from the actual biquad coefficients -
/// do not approximate it with a parametric sketch, because a curve that
/// disagrees with what you hear is worse than no curve." Measured through the
/// audio path, band by band and as a stack.
#[test]
fn composite_curve_matches_the_audio_path() {
    let fs = 48_000.0;
    let cfg = EqConfig {
        enabled: true,
        bands: vec![
            EqBand {
                kind: FilterKind::HighPass,
                slope_db_oct: 24,
                ..EqBand::new(1, FilterKind::HighPass, 60.0, 0.0, 2.0)
            },
            EqBand::new(2, FilterKind::LowShelf, 150.0, 4.0, 0.4),
            EqBand::new(3, FilterKind::Bell, 900.0, -7.0, 3.0),
            EqBand::new(4, FilterKind::Bell, 3_000.0, 5.0, 0.8),
            EqBand::new(5, FilterKind::HighShelf, 9_000.0, -6.0, 1.5),
            EqBand {
                kind: FilterKind::LowPass,
                slope_db_oct: 48,
                ..EqBand::new(6, FilterKind::LowPass, 16_000.0, 0.0, FRAC_1_SQRT_2 as f32)
            },
        ],
    };
    let probes: Vec<f32> = vec![
        30.0, 60.0, 120.0, 250.0, 500.0, 900.0, 2_000.0, 3_000.0, 6_000.0, 9_000.0, 14_000.0,
        18_000.0,
    ];
    let mut curve = Vec::new();
    curve_db(&cfg, fs, &probes, &mut curve);
    let mut worst = 0.0f64;
    for (i, &f) in probes.iter().enumerate() {
        let measured = measure_eq_db(&cfg, fs, f as f64);
        let drawn = curve[i] as f64;
        let e = (measured - drawn).abs();
        println!("{f:>7} Hz: drawn {drawn:8.3} dB, measured {measured:8.3} dB");
        assert!(
            e < 0.05,
            "at {f} Hz the curve says {drawn:.3} dB and the audio does {measured:.3} dB"
        );
        worst = worst.max(e);
    }
    println!("worst curve-vs-audio disagreement: {worst:.4} dB");
}

/// The front end recomputes the same curve in TypeScript at pointer rate
/// (`src/lib/eq.ts`, SPEC §3.1: "the EQ curve is evaluated in the front end
/// from the same coefficients"). Two implementations, one contract: this pins
/// the engine's cascade expansion to the one `bandStages()` draws, transcribed
/// here. It caught a divergence of up to 35 dB - the front end scaled the first
/// Butterworth section by `q / (1/sqrt 2)` while the engine ignored `q`
/// entirely above 12 dB/oct, so a 24 dB/oct high-pass dragged to Q = 4 drew a
/// +12.5 dB corner resonance that the audio path did not have.
#[test]
fn engine_cascade_matches_the_front_end_expansion() {
    let fs = 48_000.0;
    // src/lib/eq.ts: butterworthQs(stages)[k]
    let butterworth = |stages: usize, k: usize| -> f64 {
        1.0 / (2.0 * (((2 * k + 1) as f64 * PI) / (4.0 * stages as f64)).cos())
    };
    let mut worst = 0.0f64;
    for &kind in &ALL_KINDS {
        for &q in &[0.1f64, 0.5, FRAC_1_SQRT_2, 1.0, 2.0, 4.0, 10.0, 40.0] {
            for &slope in &[12u8, 24, 48] {
                let sections = if kind.has_slope() {
                    match slope {
                        0..=17 => 1,
                        18..=35 => 2,
                        _ => 4,
                    }
                } else {
                    1
                };
                for &f0 in &[40.0f64, 400.0, 4_000.0, 18_000.0] {
                    let gain = if kind.has_gain() { 5.5 } else { 0.0 };
                    // What the front end would draw.
                    let mut ui = vec![0.0f64; 40];
                    for k in 0..sections {
                        // bandStages(): stage 0 carries the band's own Q.
                        let sq = if !kind.has_slope() {
                            q
                        } else if sections == 1 {
                            butterworth(1, 0) * q / FRAC_1_SQRT_2
                        } else if k == 0 {
                            butterworth(sections, 0) * q / FRAC_1_SQRT_2
                        } else {
                            butterworth(sections, k)
                        };
                        let c =
                            Coeffs::design(kind, fs, f0, sq, if sections > 1 { 0.0 } else { gain });
                        for (i, slot) in ui.iter_mut().enumerate() {
                            *slot += c.magnitude_db(fs, 20.0 * 1.2f64.powi(i as i32));
                        }
                    }
                    // What the engine runs.
                    let mut engine = vec![0.0f64; 40];
                    for k in 0..sections {
                        let c = Coeffs::design_cascade_section(kind, fs, f0, q, gain, sections, k);
                        for (i, slot) in engine.iter_mut().enumerate() {
                            *slot += c.magnitude_db(fs, 20.0 * 1.2f64.powi(i as i32));
                        }
                    }
                    for (a, b) in ui.iter().zip(engine.iter()) {
                        if a.is_finite() && b.is_finite() {
                            worst = worst.max((a - b).abs());
                        }
                    }
                }
            }
        }
    }
    println!("worst |front-end curve - engine cascade| = {worst:.3e} dB");
    assert!(
        worst < 1e-9,
        "the drawn curve and the audio path differ by {worst} dB"
    );
}

/* ── 5. up against Nyquist ───────────────────────────────────────────────── */

/// Bilinear warping is exact at `f0` (that is what the pre-warp buys) and
/// compresses everything above it. Both halves of that statement are asserted,
/// with the size of the compression printed, because near Nyquist it is large
/// enough to surprise someone comparing against an analogue curve.
#[test]
fn near_nyquist_warping_is_exact_at_the_corner() {
    for &fs in &[44_100.0f64, 48_000.0] {
        for &f0 in &[8_000.0f64, 10_000.0, 16_000.0, 20_000.0] {
            let c = Coeffs::design(FilterKind::LowPass, fs, f0, FRAC_1_SQRT_2, 0.0);
            // The corner is where the design put it, to the last bit.
            assert!(
                (c.magnitude_db(fs, f0) + 3.010_299_956_639_812).abs() < 1e-9,
                "fs={fs} f0={f0}: corner landed at {} dB",
                c.magnitude_db(fs, f0)
            );
            // An octave up, warping steepens the roll-off relative to analogue.
            // (Only meaningful while an octave up is still inside the band.)
            let probe = f0 * 2.0;
            if probe < fs * 0.45 {
                let digital = c.magnitude_db(fs, probe);
                let analogue =
                    20.0 * analog_mag(FilterKind::LowPass, probe / f0, FRAC_1_SQRT_2, 0.0).log10();
                println!(
                    "fs={fs} LP {f0} Hz at {probe} Hz: digital {digital:.3} dB, \
                     analogue prototype {analogue:.3} dB, warping {:.3} dB",
                    digital - analogue
                );
                assert!(
                    digital <= analogue + 1e-9,
                    "warping should never under-attenuate"
                );
            }
        }
    }
    // Exact zeros: LP at Nyquist, HP at DC. Both floor the read-out.
    let fs = 48_000.0;
    let lp = Coeffs::design(FilterKind::LowPass, fs, 18_000.0, 0.7, 0.0);
    assert!(lp.magnitude_db(fs, fs / 2.0) <= -119.0);
    let hp = Coeffs::design(FilterKind::HighPass, fs, 30.0, 0.7, 0.0);
    assert!(hp.magnitude_db(fs, 0.0) <= -119.0);

    // A band pushed at Nyquist stays finite and stable rather than exploding.
    for &kind in &ALL_KINDS {
        for &q in &[0.1, 40.0] {
            let c = Coeffs::design(kind, fs, fs * 0.5, q, 24.0);
            let mut bq = Biquad::new(c);
            let mut last = 0.0;
            for i in 0..20_000 {
                last = bq.process(((i % 11) as f64 - 5.0) / 5.0);
            }
            assert!(last.is_finite(), "{kind:?} q={q} blew up at Nyquist");
        }
    }
}

/// The same filter at every rate Onyx supports has to describe the same curve
/// in Hz, not in normalised frequency: the classic bug is a design that is only
/// right at 48 kHz. Well below Nyquist the rates must agree to a hundredth of a
/// dB; higher up they may differ only by the bilinear warping, whose size is
/// printed and bounded by the analogue prototype at each rate.
#[test]
fn designs_track_the_sample_rate() {
    let rates = [44_100.0f64, 48_000.0, 88_200.0, 96_000.0, 192_000.0];
    let mut worst_low = 0.0f64;
    let mut worst_warp = 0.0f64;
    for &kind in &ALL_KINDS {
        for &f0 in &[50.0f64, 1_000.0, 10_000.0] {
            let g = if kind.has_gain() { 8.0 } else { 0.0 };
            let reference = Coeffs::design(kind, 48_000.0, f0, 1.3, g);
            for &fs in &rates {
                let c = Coeffs::design(kind, fs, f0, 1.3, g);
                // Each rate is its own analogue prototype, exactly.
                for probe in [30.0f64, 200.0, 2_000.0, 5_000.0, 15_000.0] {
                    let want = expected_db(kind, fs, f0, probe, 1.3, g);
                    if want > -80.0 {
                        assert!(
                            (c.magnitude_db(fs, probe) - want).abs() < 1e-6,
                            "{kind:?} f0={f0} fs={fs} at {probe} Hz"
                        );
                    }
                }
                // And well below Nyquist the rates agree with each other: a
                // 1 kHz filter probed at 2 kHz is the same curve at 44.1 kHz as
                // at 192 kHz, because `f0` is in Hz and the warping there is
                // parts in ten thousand.
                if f0 <= 1_000.0 {
                    for probe in [30.0f64, 200.0, 2_000.0] {
                        let a = reference.magnitude_db(48_000.0, probe);
                        let b = c.magnitude_db(fs, probe);
                        // Only where the curve is actually drawn: 60 dB down a
                        // stopband, a part-per-thousand warping difference in
                        // the pole position is worth 0.07 dB and means nothing.
                        if a > -30.0 {
                            worst_low = worst_low.max((a - b).abs());
                            assert!(
                                (a - b).abs() < 0.1,
                                "{kind:?} f0={f0} at {probe} Hz: {a:.4} dB at 48 kHz vs \
                                 {b:.4} dB at {fs}"
                            );
                        }
                    }
                }
                let a = reference.magnitude_db(48_000.0, 15_000.0);
                let b = c.magnitude_db(fs, 15_000.0);
                if a > -30.0 {
                    worst_warp = worst_warp.max((a - b).abs());
                }
            }
        }
    }
    println!("cross-rate difference at/below 2 kHz: {worst_low:.4} dB (bilinear warping only)");
    println!("cross-rate difference at 15 kHz:   {worst_warp:.4} dB (bilinear warping only)");
    // 15 kHz is 0.68 of Nyquist at 44.1 kHz and 0.16 at 192 kHz, so the same
    // filter genuinely is a different curve there. That is the bilinear
    // transform, not a defect - but it is worth a number, because an engineer
    // A/B-ing a 44.1 and a 96 kHz master through the same EQ will see it.
    assert!(
        worst_warp < 6.0,
        "cross-rate warping at 15 kHz reached {worst_warp} dB"
    );
}
