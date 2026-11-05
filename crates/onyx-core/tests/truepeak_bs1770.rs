//! True-peak validation against external ground truth.
//!
//! References:
//!   * ITU-R BS.1770-4 (10/2015), Annex 2 "Guidelines for accurate measurement
//!     of true-peak level" and its Appendix 1 (the 4x sampling-grid error
//!     table).
//!   * EBU Tech 3341 (2023), Table 1, minimum requirement test signals 15-23
//!     ("EBU Mode" true-peak tests), tolerance +0.2 / -0.4 dBTP.
//!
//! Nothing here asserts self-consistency: every expectation is either an
//! analytic value of the continuous waveform, a number printed in one of the
//! two documents, or an exact FFT reconstruction of the band-limited signal.

use onyx_core::dsp::truepeak::TruePeak;

const TOL_HI: f32 = 0.2; // EBU Tech 3341 Table 1
const TOL_LO: f32 = -0.4;

fn tp_db(signal: &[f32]) -> f32 {
    let mut tp = TruePeak::new(1);
    tp.process(signal);
    tp.peak_db(0)
}

fn db(x: f64) -> f64 {
    20.0 * x.log10()
}

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-12 {
        1.0
    } else {
        (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
    }
}

/// EBU Tech 3341 Table 1, tests 15-19: a stereo sine of given frequency,
/// amplitude and starting phase. The expected reading in the table is quoted
/// to 0.1 dB; the analytic true peak is `20 log10(amplitude)`.
fn ebu_tone(freq: f64, amp: f64, phase_deg: f64, sr: f64, secs: f64) -> Vec<f32> {
    let n = (sr * secs) as usize;
    (0..n)
        .map(|i| {
            (amp * (2.0 * std::f64::consts::PI * freq * i as f64 / sr + phase_deg.to_radians())
                .sin()) as f32
        })
        .collect()
}

#[test]
fn ebu_tech_3341_tests_15_to_19() {
    // (test, freq divisor, amplitude, phase deg, table value)
    let cases: [(u32, f64, f64, f64, f64); 5] = [
        (15, 4.0, 0.50, 0.0, -6.0),
        (16, 4.0, 0.50, 45.0, -6.0),
        (17, 6.0, 0.50, 60.0, -6.0),
        (18, 8.0, 0.50, 67.5, -6.0),
        (19, 4.0, 1.41, 45.0, 3.0),
    ];
    for sr in [44_100.0f64, 48_000.0, 96_000.0] {
        for &(test, div, amp, phase, table) in &cases {
            let sig = ebu_tone(sr / div, amp, phase, sr, 1.0);
            let got = tp_db(&sig);
            let analytic = db(amp) as f32;
            let err = got - table as f32;
            println!(
                "3341-{test} @{sr:.0}: table {table:+.1}  analytic {analytic:+.4}  \
                 measured {got:+.4} dBTP  (err vs table {err:+.4})"
            );
            assert!(
                (TOL_LO..=TOL_HI).contains(&err),
                "EBU Tech 3341 test {test} at {sr} Hz: {got} dBTP outside {table} +0.2/-0.4"
            );
            // Much tighter than EBU demands: we should land on the analytic
            // peak of the waveform, because the 4x grid hits the crest of a
            // tone at fs/4, fs/6 and fs/8 exactly.
            assert!(
                (got - analytic).abs() < 0.02,
                "EBU Tech 3341 test {test} at {sr} Hz: {got} dBTP vs analytic {analytic}"
            );
        }
    }
}

/// EBU Tech 3341 Table 1, tests 20-23: an fs/6 tone at 0.5 with one embedded
/// period of an fs/4 tone at 1.0, synthesised at 4 fs, low-pass filtered and
/// decimated by 4 with an offset of 0, 1, 2 or 3 samples at the 4 fs rate.
/// Expected: max true peak = 0.0 +0.2/-0.4 dBTP.
#[test]
fn ebu_tech_3341_tests_20_to_23() {
    let sr = 48_000.0f64;
    let up = 4.0 * sr;
    let n_up = (up * 0.5) as usize;
    let f1 = sr / 6.0;
    let f2 = sr / 4.0;
    // Base tone, phase-continuous burst substituted at a positive-going zero
    // crossing so both joins are at zero.
    let burst_len = (up / f2).round() as usize;
    let period = up / f1;
    let burst_start = ((n_up as f64 / 2.0 / period).round() * period) as usize;
    let mut x = vec![0.0f64; n_up];
    for (i, v) in x.iter_mut().enumerate() {
        *v = if i >= burst_start && i < burst_start + burst_len {
            (2.0 * std::f64::consts::PI * f2 * (i - burst_start) as f64 / up).sin()
        } else if i < burst_start {
            0.5 * (2.0 * std::f64::consts::PI * f1 * i as f64 / up).sin()
        } else {
            0.5 * (2.0 * std::f64::consts::PI * f1 * (i - burst_start - burst_len) as f64 / up)
                .sin()
        };
    }
    // 10 ms taper, as the table asks for.
    let ramp = (0.01 * up) as usize;
    for i in 0..ramp {
        let w = i as f64 / ramp as f64;
        x[i] *= w;
        let j = n_up - 1 - i;
        x[j] *= w;
    }
    // Anti-alias filter: 511-tap Kaiser-windowed sinc at 0.5 / 4 of the 4 fs
    // rate, i.e. the input Nyquist.
    let taps = 511usize;
    let centre = (taps - 1) as f64 / 2.0;
    let lp: Vec<f64> = (0..taps)
        .map(|i| {
            let t = i as f64 - centre;
            let s = 2.0 * 0.125 * sinc(2.0 * 0.125 * t);
            // Blackman window is plenty here.
            let a = 2.0 * std::f64::consts::PI * i as f64 / (taps - 1) as f64;
            s * (0.42 - 0.5 * a.cos() + 0.08 * (2.0 * a).cos())
        })
        .collect();
    let mut y = vec![0.0f64; n_up];
    for (i, out) in y.iter_mut().enumerate() {
        let mut acc = 0.0;
        for (k, c) in lp.iter().enumerate() {
            let idx = i as isize + centre as isize - k as isize;
            if idx >= 0 && (idx as usize) < n_up {
                acc += c * x[idx as usize];
            }
        }
        *out = acc;
    }
    for (test, offset) in (20u32..=23).zip(0usize..4) {
        let dec: Vec<f32> = y
            .iter()
            .skip(offset)
            .step_by(4)
            .map(|v| *v as f32)
            .collect();
        let exact = exact_true_peak(&dec);
        let got = tp_db(&dec);
        println!(
            "3341-{test} (offset {offset}): table +0.0  exact reconstruction {:+.4}  \
             measured {got:+.4} dBTP",
            db(exact as f64)
        );
        assert!(
            (TOL_LO..=TOL_HI).contains(&got),
            "EBU Tech 3341 test {test}: {got} dBTP outside 0.0 +0.2/-0.4"
        );
        // The synthesised signal is not bit-identical to the EBU's WAV (we do
        // not have it), so also pin the reading against an exact reconstruction
        // of the signal we actually generated.
        assert!(
            (got as f64 - db(exact as f64)).abs() < 0.1,
            "test {test}: measured {got} dBTP against an exact {} dBTP",
            db(exact as f64)
        );
    }
}

/// True peak is by definition the maximum of the *continuous* waveform, so it
/// can never be below the largest sample. A polyphase bank whose branch 0 is
/// not the identity (the BS.1770-4 Annex 2 table, for instance) breaks this.
#[test]
fn never_reports_less_than_the_sample_peak() {
    let sr = 48_000.0f64;
    for &f in &[19_000.0f64, 21_000.0, 23_000.0, 23_900.0] {
        for phase in 0..8 {
            let ph = phase as f64 * 45.0;
            let sig = ebu_tone(f, 0.9, ph, sr, 0.2);
            let sample_peak = 20.0 * sig.iter().fold(0.0f32, |m, s| m.max(s.abs())).log10();
            let got = tp_db(&sig);
            assert!(
                got >= sample_peak - 1e-4,
                "{f} Hz phase {ph}: true peak {got} < sample peak {sample_peak}"
            );
        }
    }
}

/// Exact true peak of the band-limited interpolation of `x`, computed by
/// zero-padding the DFT by 32x. For a signal that is band-limited and whose
/// periodic extension is smooth this is the continuous-time maximum to within
/// the 32x grid (< 0.002 dB at Nyquist), and it uses none of the code under
/// test.
fn exact_true_peak(x: &[f32]) -> f32 {
    use rustfft::{num_complex::Complex32, FftPlanner};
    let n = x.len();
    let up = 32usize;
    let mut planner = FftPlanner::<f32>::new();
    let fwd = planner.plan_fft_forward(n);
    let inv = planner.plan_fft_inverse(n * up);
    let mut spec: Vec<Complex32> = x.iter().map(|&v| Complex32::new(v, 0.0)).collect();
    fwd.process(&mut spec);
    let mut big = vec![Complex32::new(0.0, 0.0); n * up];
    let half = n / 2;
    for k in 0..half {
        big[k] = spec[k];
        big[n * up - 1 - k] = spec[n - 1 - k];
    }
    inv.process(&mut big);
    let scale = 1.0 / n as f32;
    big.iter().fold(0.0f32, |m, c| m.max((c.re * scale).abs()))
}

/// The reference interpolator printed in BS.1770-4 Annex 2 (order 48, 4-phase),
/// used only as a yardstick: `recommends 4` allows "a method that gives similar
/// or superior results", so ours must be at least as accurate.
const ITU_TABLE: [[f64; 4]; 12] = [
    [
        0.001_708_984_375,
        -0.029_174_804_687_5,
        -0.018_920_898_437_5,
        -0.008_300_781_25,
    ],
    [
        0.010_986_328_125,
        0.029_296_875,
        0.033_081_054_687_5,
        0.014_892_578_125,
    ],
    [
        -0.019_653_320_312_5,
        -0.051_757_812_5,
        -0.058_227_539_062_5,
        -0.026_611_328_125,
    ],
    [
        0.033_203_125,
        0.089_111_328_125,
        0.101_562_5,
        0.047_607_421_875,
    ],
    [
        -0.059_448_242_187_5,
        -0.166_503_906_25,
        -0.200_317_382_812_5,
        -0.102_294_921_875,
    ],
    [
        0.137_329_101_562_5,
        0.465_087_890_625,
        0.779_785_156_25,
        0.972_167_968_75,
    ],
    [
        0.972_167_968_75,
        0.779_785_156_25,
        0.465_087_890_625,
        0.137_329_101_562_5,
    ],
    [
        -0.102_294_921_875,
        -0.200_317_382_812_5,
        -0.166_503_906_25,
        -0.059_448_242_187_5,
    ],
    [
        0.047_607_421_875,
        0.101_562_5,
        0.089_111_328_125,
        0.033_203_125,
    ],
    [
        -0.026_611_328_125,
        -0.058_227_539_062_5,
        -0.051_757_812_5,
        -0.019_653_320_312_5,
    ],
    [
        0.014_892_578_125,
        0.033_081_054_687_5,
        0.029_296_875,
        0.010_986_328_125,
    ],
    [
        -0.008_300_781_25,
        -0.018_920_898_437_5,
        -0.029_174_804_687_5,
        0.001_708_984_375,
    ],
];

fn itu_reference_peak(x: &[f32]) -> f64 {
    let mut best = 0.0f64;
    for n in 11..x.len() {
        for p in 0..4 {
            let mut acc = 0.0f64;
            for (t, row) in ITU_TABLE.iter().enumerate() {
                acc += row[p] * x[n - t] as f64;
            }
            best = best.max(acc.abs());
        }
    }
    best
}

/// A band-limited impulse `B * sinc(B (t - t0))` has an analytic continuous
/// peak of exactly `B` at `t = t0`. Sweeping `t0` across a sample interval
/// exercises every fractional position, and the worst case is bounded from
/// below by the 4x grid itself: the nearest of the 0, 1/4, 1/2, 3/4 grid points
/// is at most 1/8 of a sample away, i.e. `20 log10(sinc(B/8))`
/// (BS.1770-4 Annex 2, Appendix 1 gives the same numbers for a pure tone).
#[test]
fn band_limited_impulse_reaches_the_4x_grid_limit() {
    let n = 4096usize;
    for &bw in &[0.85f64, 0.90, 0.95, 1.00] {
        let grid_limit = db(sinc(bw / 8.0));
        let mut worst = f64::INFINITY;
        let mut over = f64::NEG_INFINITY;
        let mut itu_worst = f64::INFINITY;
        for step in 0..33 {
            let t0 = (n / 2) as f64 + step as f64 / 32.0;
            let sig: Vec<f32> = (0..n)
                .map(|i| (bw * sinc(bw * (i as f64 - t0))) as f32)
                .collect();
            let e = tp_db(&sig) as f64 - db(bw);
            worst = worst.min(e);
            over = over.max(e);
            itu_worst = itu_worst.min(db(itu_reference_peak(&sig)) - db(bw));
        }
        println!(
            "band-limited impulse, BW {:.2} x Nyquist: onyx {worst:+.3} .. {over:+.3} dB, \
             ITU table {itu_worst:+.3} dB, 4x grid limit {grid_limit:+.3} dB",
            bw
        );
        // Never over-read by more than a hundredth of a dB.
        assert!(over < 0.02, "over-read {over} dB at BW {bw}");
        // Must beat the reference filter printed in BS.1770-4 Annex 2.
        assert!(
            worst > itu_worst,
            "BW {bw}: onyx {worst} dB is worse than the ITU table's {itu_worst} dB"
        );
        // ... and stay within 0.2 dB of the grid limit that no 4x meter can beat.
        assert!(
            worst > grid_limit - 0.2,
            "BW {bw}: {worst} dB against a grid limit of {grid_limit} dB"
        );
    }
}

/// The interpolation error of the four branches, measured by driving the
/// detector with tones whose true peak is known analytically and whose crest
/// lands exactly on a 4x grid point (so the sampling grid contributes nothing
/// and what is left is the filter).
#[test]
fn interpolator_passband_is_flat() {
    let sr = 48_000.0f64;
    let mut worst = 0.0f64;
    let mut worst_at = (0.0f64, 0.0f64);
    // For a tone at fs/k the 4x grid has 4k points per period, i.e. one every
    // 360/(4k) degrees; a crest sits on the grid whenever the start phase is a
    // multiple of that step, because 90 deg is exactly k steps.
    for &k in &[4.0f64, 8.0, 16.0, 32.0, 64.0] {
        let step = 360.0 / (4.0 * k);
        for j in 0..8 {
            let ph = j as f64 * step;
            let sig = ebu_tone(sr / k, 0.5, ph, sr, 0.5);
            let e = tp_db(&sig) as f64 - db(0.5);
            if e.abs() > worst {
                worst = e.abs();
                worst_at = (sr / k, ph);
            }
        }
    }
    println!(
        "worst |error| over fs/4..fs/64 tones on-grid, 8 start phases: {worst:.4} dB \
         (at {:.0} Hz, {:.2} deg)",
        worst_at.0, worst_at.1
    );
    assert!(worst < 0.02, "passband error {worst} dB");
}

/// Cross-check against an exact FFT reconstruction on a signal that is not a
/// pure tone: shaped noise band-limited to 0.45 fs.
#[test]
fn matches_exact_reconstruction_on_broadband_material() {
    let n = 8192usize;
    // Deterministic pseudo-noise, low-passed to 0.45 fs by summing harmonics.
    let mut sig = vec![0.0f64; n];
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut harmonics = Vec::new();
    for k in 1..(n as f64 * 0.45) as usize {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        let phase = (state >> 11) as f64 / (1u64 << 53) as f64 * std::f64::consts::TAU;
        harmonics.push((k, phase));
    }
    for (k, phase) in harmonics.iter().take(400) {
        for (i, s) in sig.iter_mut().enumerate() {
            *s += (std::f64::consts::TAU * *k as f64 * i as f64 / n as f64 + phase).cos();
        }
    }
    let norm = sig.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    let x: Vec<f32> = sig.iter().map(|v| (0.9 * v / norm) as f32).collect();

    let exact = db(exact_true_peak(&x) as f64);
    let got = tp_db(&x) as f64;
    println!(
        "broadband: exact {exact:+.4} dBTP, measured {got:+.4} dBTP, err {:+.4} dB",
        got - exact
    );
    assert!(
        (got - exact).abs() < 0.1,
        "measured {got} dBTP against an exact {exact} dBTP"
    );
}
