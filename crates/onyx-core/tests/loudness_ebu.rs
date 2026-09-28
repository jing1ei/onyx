//! Loudness validation against external ground truth.
//!
//! References:
//!   * ITU-R BS.1770-4 (10/2015), Annex 1: K-weighting, Table 3 channel
//!     weights, equations (3)-(7) for the 400 ms / 75 % overlap gating, and the
//!     calibration statement "if a 0 dB FS, 1 kHz (997 Hz to be exact) sine
//!     wave is applied to the left, centre, or right channel input, the
//!     indicated loudness will equal -3.01 LKFS".
//!   * EBU Tech 3341 (2023), Table 1, minimum requirement test signals 1-14.
//!   * EBU Tech 3342 (2023), Table 1, minimum requirement test signals 1-4, and
//!     the MATLAB listing in section 5.
//!
//! Tests 7, 8 (Tech 3341) and 5, 6 (Tech 3342) use "authentic programme"
//! material that is only distributed as proprietary WAV files by the EBU; they
//! cannot be synthesised and are therefore *not* covered here. Tech 3341
//! test 6 is a 5.0 signal and Onyx has no surround path at all - see
//! `surround_material_is_not_measured_per_bs1770` below.

use onyx_core::dsp::biquad::Biquad;
use onyx_core::dsp::loudness::{k_weighting_coeffs, LoudnessMeter};

/// EBU Tech 3341 Table 1 tolerance for the loudness read-outs.
const TOL_LU: f32 = 0.1;
/// EBU Tech 3342 Table 1 tolerance for LRA.
const TOL_LRA: f32 = 1.0;

const RATES: [f64; 3] = [44_100.0, 48_000.0, 96_000.0];

/// A 1 kHz sine of the given per-channel peak level in dBFS, in phase on both
/// legs, appended to `out`. The phase continues across segment joins, which is
/// what "followed immediately by" in the tables means.
fn push_tone(out: &mut Vec<f32>, level_dbfs: f64, secs: f64, sr: f64) {
    let amp = 10f64.powf(level_dbfs / 20.0);
    let n = (sr * secs).round() as usize;
    let start = out.len() / 2;
    for i in 0..n {
        let s = (amp * (std::f64::consts::TAU * 1_000.0 * (start + i) as f64 / sr).sin()) as f32;
        out.push(s);
        out.push(s);
    }
}

fn push_silence(out: &mut Vec<f32>, secs: f64, sr: f64) {
    out.resize(out.len() + 2 * (sr * secs).round() as usize, 0.0);
}

fn measure(sr: f64, segments: &[(f64, f64)]) -> (f32, f32, f32, f32) {
    let mut buf = Vec::new();
    for &(level, secs) in segments {
        push_tone(&mut buf, level, secs, sr);
    }
    let mut m = LoudnessMeter::new(sr);
    m.process(&buf);
    (m.momentary(), m.short_term(), m.integrated(), m.lra())
}

// ---------------------------------------------------------------------------
// BS.1770-4 Annex 1 calibration and channel weighting
// ---------------------------------------------------------------------------

/// BS.1770-4 Annex 1: "If a 0 dB FS, 1 kHz (997 Hz to be exact) sine wave is
/// applied to the left, centre, or right channel input, the indicated loudness
/// will equal -3.01 LKFS." That single number calibrates the -0.691 offset,
/// the K-weighting gain at 997 Hz and the channel weights all at once.
#[test]
fn bs1770_calibration_997_hz_on_one_channel_reads_minus_3_01() {
    for sr in RATES {
        for (name, left, right) in [("left", true, false), ("right", false, true)] {
            let n = (sr * 10.0) as usize;
            let mut buf = Vec::with_capacity(n * 2);
            for i in 0..n {
                let s = (std::f64::consts::TAU * 997.0 * i as f64 / sr).sin() as f32;
                buf.push(if left { s } else { 0.0 });
                buf.push(if right { s } else { 0.0 });
            }
            let mut m = LoudnessMeter::new(sr);
            m.process(&buf);
            let got = m.integrated();
            println!("BS.1770-4 calibration, {name} leg @{sr:.0}: {got:+.4} LKFS (expect -3.01)");
            assert!(
                (got + 3.01).abs() <= TOL_LU,
                "{name} @{sr}: {got} LKFS, expected -3.01 +/- {TOL_LU}"
            );
        }
    }
}

/// Table 3: G_L = G_R = 1.0. Two identical legs must therefore read exactly
/// 10*log10(2) = 3.0103 LU above one leg.
#[test]
fn channel_weights_for_left_and_right_are_unity() {
    let sr = 48_000.0;
    let n = (sr * 10.0) as usize;
    let mut stereo = Vec::with_capacity(n * 2);
    let mut one_leg = Vec::with_capacity(n * 2);
    for i in 0..n {
        let s = (0.5 * (std::f64::consts::TAU * 997.0 * i as f64 / sr).sin()) as f32;
        stereo.push(s);
        stereo.push(s);
        one_leg.push(s);
        one_leg.push(0.0);
    }
    let mut a = LoudnessMeter::new(sr);
    a.process(&stereo);
    let mut b = LoudnessMeter::new(sr);
    b.process(&one_leg);
    let delta = a.integrated() - b.integrated();
    println!("G_L = G_R = 1.0 check: stereo - one leg = {delta:+.4} LU (expect 3.0103)");
    assert!((delta - 3.0103).abs() < 0.01, "delta {delta} LU");
}

/// BS.1770-4 Table 3 also gives G_Ls = G_Rs = 1.41 and excludes the LFE.
/// Onyx has no surround path: `decode::fold_interleaved` keeps the first two
/// channels of a wider file and discards the rest, so a 5.0 or 5.1 programme is
/// measured on L and R alone. EBU Tech 3341 test 6 (5.0 sine, expected
/// -23.0 LUFS) therefore cannot be satisfied, and this test states by how much.
#[test]
fn surround_material_is_not_measured_per_bs1770() {
    let sr = 48_000.0;
    // Tech 3341 test 6: -28 dBFS in L and R, -24 in C, -30 in Ls and Rs.
    let g = |db: f64| 10f64.powf(db / 20.0);
    let (l, c, ls) = (g(-28.0), g(-24.0), g(-30.0));
    // Reference value with the BS.1770 weights, relative to the same tone at
    // 0 dBFS on one channel (-3.01 LKFS):
    let full = 2.0 * l * l + c * c + 2.0 * 1.41 * ls * ls;
    let expected_surround = -3.01 + 10.0 * (full / (1.0 / 2.0) * 0.5).log10();
    // What Onyx actually measures: L and R only.
    let stereo_only = -3.01 + 10.0 * (2.0 * l * l / (1.0 / 2.0) * 0.5).log10();
    let n = (sr * 10.0) as usize;
    let mut buf = Vec::with_capacity(n * 2);
    for i in 0..n {
        let s = (l * (std::f64::consts::TAU * 1_000.0 * i as f64 / sr).sin()) as f32;
        buf.push(s);
        buf.push(s);
    }
    let mut m = LoudnessMeter::new(sr);
    m.process(&buf);
    let got = m.integrated();
    println!(
        "EBU Tech 3341 test 6 (5.0): BS.1770 answer {expected_surround:+.2} LUFS, \
         Onyx measures L/R only and reads {got:+.2} LUFS \
         (predicted {stereo_only:+.2}), i.e. {:+.2} LU low",
        got as f64 - expected_surround
    );
    assert!((got as f64 - stereo_only).abs() < 0.1);
    // Documented shortfall: about 5 LU. This assertion exists so that if a
    // surround path is ever added, this test fails and gets revisited.
    assert!(
        (got as f64) < expected_surround - 4.0,
        "surround handling appears to have changed"
    );
}

// ---------------------------------------------------------------------------
// EBU Tech 3341 Table 1, tests 1-5
// ---------------------------------------------------------------------------

#[test]
fn ebu_tech_3341_tests_1_and_2_steady_tones() {
    for sr in RATES {
        for (test, level) in [(1u32, -23.0f64), (2, -33.0)] {
            let (m, s, i, _) = measure(sr, &[(level, 20.0)]);
            println!("3341-{test} @{sr:.0}: M {m:+.4} S {s:+.4} I {i:+.4} (expect {level:+.1})");
            for (label, got) in [("M", m), ("S", s), ("I", i)] {
                assert!(
                    (got - level as f32).abs() <= TOL_LU,
                    "3341-{test} @{sr}: {label} = {got}, expected {level} +/- {TOL_LU}"
                );
            }
        }
    }
}

#[test]
fn ebu_tech_3341_test_3_absolute_gate() {
    for sr in RATES {
        let (_, _, i, _) = measure(sr, &[(-36.0, 10.0), (-23.0, 60.0), (-36.0, 10.0)]);
        println!("3341-3 @{sr:.0}: I {i:+.4} (expect -23.0)");
        assert!((i + 23.0).abs() <= TOL_LU, "3341-3 @{sr}: {i}");
    }
}

/// Test 4 adds -72 dBFS head and tail, which sit below the -70 LUFS absolute
/// gate and must be discarded outright.
#[test]
fn ebu_tech_3341_test_4_minus_70_lufs_absolute_gate() {
    for sr in RATES {
        let (_, _, i, _) = measure(
            sr,
            &[
                (-72.0, 10.0),
                (-36.0, 10.0),
                (-23.0, 60.0),
                (-36.0, 10.0),
                (-72.0, 10.0),
            ],
        );
        println!("3341-4 @{sr:.0}: I {i:+.4} (expect -23.0)");
        assert!((i + 23.0).abs() <= TOL_LU, "3341-4 @{sr}: {i}");
    }
}

/// Test 5 is the relative-gate probe: 20 s at -26, 20.1 s at -20, 20 s at -26.
/// The deliberately non-integer 20.1 s also exercises the 100 ms gating hop.
#[test]
fn ebu_tech_3341_test_5_minus_10_lu_relative_gate() {
    for sr in RATES {
        let (_, _, i, _) = measure(sr, &[(-26.0, 20.0), (-20.0, 20.1), (-26.0, 20.0)]);
        println!("3341-5 @{sr:.0}: I {i:+.4} (expect -23.0)");
        assert!((i + 23.0).abs() <= TOL_LU, "3341-5 @{sr}: {i}");
    }
}

// ---------------------------------------------------------------------------
// EBU Tech 3341 Table 1, tests 9-14: the M and S ballistics
// ---------------------------------------------------------------------------

/// Test 9: (1.34 s at -20 dBFS; 1.66 s at -30 dBFS) x 5.
/// S = -23.0 +/- 0.1 LUFS, constant after 3 s.
#[test]
fn ebu_tech_3341_test_9_short_term_is_constant() {
    let sr = 48_000.0;
    let mut buf = Vec::new();
    for _ in 0..5 {
        push_tone(&mut buf, -20.0, 1.34, sr);
        push_tone(&mut buf, -30.0, 1.66, sr);
    }
    let (worst, at) = sweep(&buf, sr, 3.0, 0.02, |m| m.short_term());
    println!("3341-9: worst S over t > 3 s = {worst:+.4} LUFS at {at:.2} s (expect -23.0)");
    assert!((worst + 23.0).abs() <= TOL_LU, "S = {worst} at {at} s");
}

/// Test 12: (0.18 s at -20 dBFS; 0.22 s at -30 dBFS) x 25.
/// M = -23.0 +/- 0.1 LUFS, constant after 1 s.
#[test]
fn ebu_tech_3341_test_12_momentary_is_constant() {
    let sr = 48_000.0;
    let mut buf = Vec::new();
    for _ in 0..25 {
        push_tone(&mut buf, -20.0, 0.18, sr);
        push_tone(&mut buf, -30.0, 0.22, sr);
    }
    let (worst, at) = sweep(&buf, sr, 1.0, 0.005, |m| m.momentary());
    println!("3341-12: worst M over t > 1 s = {worst:+.4} LUFS at {at:.2} s (expect -23.0)");
    assert!((worst + 23.0).abs() <= TOL_LU, "M = {worst} at {at} s");
}

/// Feed `buf` in small steps and return the reading furthest from -23 LUFS
/// after `skip_secs`, with the time at which it occurred.
fn sweep(
    buf: &[f32],
    sr: f64,
    skip_secs: f64,
    step_secs: f64,
    read: fn(&LoudnessMeter) -> f32,
) -> (f32, f64) {
    let step = (sr * step_secs) as usize * 2;
    let mut m = LoudnessMeter::new(sr);
    let mut worst = -23.0f32;
    let mut at = 0.0;
    let mut pos = 0;
    while pos < buf.len() {
        let end = (pos + step).min(buf.len());
        m.process(&buf[pos..end]);
        pos = end;
        let t = pos as f64 / 2.0 / sr;
        if t >= skip_secs {
            let v = read(&m);
            if (v + 23.0).abs() > (worst + 23.0).abs() {
                worst = v;
                at = t;
            }
        }
    }
    (worst, at)
}

/// Tests 10 and 13, the file-based max-M and max-S cases: 20 segments of
/// `(i * d of silence; tone; 1 s of silence)`, each measured on its own.
/// Max S (test 10, d = 0.15 s, 3 s tone) and max M (test 13, d = 20 ms,
/// 400 ms tone) must both read -23.0 +/- 0.1 LUFS for every i.
#[test]
fn ebu_tech_3341_tests_10_and_13_max_momentary_and_short_term() {
    let sr = 48_000.0;
    for (test, lead, tone_secs, momentary) in
        [(10u32, 0.15f64, 3.0f64, false), (13, 0.020, 0.400, true)]
    {
        let mut worst = -23.0f32;
        let mut worst_i = 0;
        for i in 0..20 {
            let mut buf = Vec::new();
            push_silence(&mut buf, lead * i as f64, sr);
            push_tone(&mut buf, -23.0, tone_secs, sr);
            push_silence(&mut buf, 1.0, sr);
            let mut m = LoudnessMeter::new(sr);
            let step = (sr * 0.005) as usize * 2;
            let mut peak = f32::NEG_INFINITY;
            let mut pos = 0;
            while pos < buf.len() {
                let end = (pos + step).min(buf.len());
                m.process(&buf[pos..end]);
                pos = end;
                let v = if momentary {
                    m.momentary()
                } else {
                    m.short_term()
                };
                peak = peak.max(v);
            }
            if (peak + 23.0).abs() > (worst + 23.0).abs() {
                worst = peak;
                worst_i = i;
            }
        }
        println!("3341-{test}: worst max over 20 segments = {worst:+.4} LUFS (i = {worst_i})");
        assert!(
            (worst + 23.0).abs() <= TOL_LU,
            "3341-{test}: segment {worst_i} peaked at {worst} LUFS"
        );
    }
}

/// Tests 11 and 14, the live-meter versions: one continuous signal whose 20
/// tones step from -38 to -19 dBFS, each of which must be read back exactly.
#[test]
fn ebu_tech_3341_tests_11_and_14_successive_maxima() {
    let sr = 48_000.0;
    for (test, lead, tone_secs, momentary) in
        [(11u32, 0.15f64, 3.0f64, false), (14, 0.020, 0.400, true)]
    {
        let mut worst = 0.0f32;
        let mut worst_i = 0;
        let mut m = LoudnessMeter::new(sr);
        for i in 0..20 {
            let level = -38.0 + i as f64;
            let mut buf = Vec::new();
            push_silence(&mut buf, lead * i as f64, sr);
            push_tone(&mut buf, level, tone_secs, sr);
            push_silence(&mut buf, tone_secs - lead * i as f64, sr);
            let step = (sr * 0.005) as usize * 2;
            let mut peak = f32::NEG_INFINITY;
            let mut pos = 0;
            while pos < buf.len() {
                let end = (pos + step).min(buf.len());
                m.process(&buf[pos..end]);
                pos = end;
                let v = if momentary {
                    m.momentary()
                } else {
                    m.short_term()
                };
                peak = peak.max(v);
            }
            let err = peak - level as f32;
            if err.abs() > worst.abs() {
                worst = err;
                worst_i = i;
            }
        }
        println!(
            "3341-{test}: worst error over the 20 successive maxima = {worst:+.4} LU \
             (tone {}, {:.0} dBFS)",
            worst_i,
            -38.0 + worst_i as f64
        );
        assert!(
            worst.abs() <= TOL_LU,
            "3341-{test}: tone {worst_i} was off by {worst} LU"
        );
    }
}

// ---------------------------------------------------------------------------
// EBU Tech 3342 Table 1, tests 1-4
// ---------------------------------------------------------------------------

#[test]
fn ebu_tech_3342_tests_1_to_4_loudness_range() {
    type Case = (u32, &'static [(f64, f64)], f32);
    let cases: [Case; 4] = [
        (1, &[(-20.0, 20.0), (-30.0, 20.0)], 10.0),
        (2, &[(-20.0, 20.0), (-15.0, 20.0)], 5.0),
        (3, &[(-40.0, 20.0), (-20.0, 20.0)], 20.0),
        (
            4,
            &[
                (-50.0, 20.0),
                (-35.0, 20.0),
                (-20.0, 20.0),
                (-35.0, 20.0),
                (-50.0, 20.0),
            ],
            15.0,
        ),
    ];
    for sr in RATES {
        for &(test, segments, expected) in &cases {
            let (_, _, _, lra) = measure(sr, segments);
            println!("3342-{test} @{sr:.0}: LRA {lra:.4} LU (expect {expected} +/- 1)");
            assert!(
                (lra - expected).abs() <= TOL_LRA,
                "3342-{test} @{sr}: LRA {lra}, expected {expected} +/- {TOL_LRA}"
            );
        }
    }
    // Tech 3342 section 4: "the expected response is unchanged if the test
    // signal is repeated one or more times in its full length".
    let mut doubled = Vec::new();
    for _ in 0..3 {
        push_tone(&mut doubled, -20.0, 20.0, 48_000.0);
        push_tone(&mut doubled, -30.0, 20.0, 48_000.0);
    }
    let mut m = LoudnessMeter::new(48_000.0);
    m.process(&doubled);
    println!("3342-1 repeated 3x: LRA {:.4} LU", m.lra());
    assert!((m.lra() - 10.0).abs() <= TOL_LRA);
}

/// Tech 3342 section 3.1 requires the short-term loudness feeding LRA to be
/// sampled at >= 10 Hz. A 12 s programme therefore has to yield a usable
/// distribution; at the 1 Hz rate this implementation used to run at, only 10
/// values existed and the 10th/95th percentiles quantised to whole steps.
#[test]
fn ebu_tech_3342_short_term_is_sampled_at_10_hz() {
    let sr = 48_000.0;
    // A 12 s linear fade from -18 to -30 LUFS. The 3 s short-term window
    // trails, so the values run from about -19.5 down to -28.5 LUFS and LRA is
    // close to 0.85 * (28.5 - 19.5).
    let mut buf = Vec::new();
    let n = (sr * 12.0) as usize;
    for i in 0..n {
        let db = -18.0 - 12.0 * i as f64 / n as f64;
        let s = (10f64.powf(db / 20.0) * (std::f64::consts::TAU * 1_000.0 * i as f64 / sr).sin())
            as f32;
        buf.push(s);
        buf.push(s);
    }
    push_silence(&mut buf, 1.5, sr); // Tech 3342 section 5 note
    let mut m = LoudnessMeter::new(sr);
    m.process(&buf);
    let lra = m.lra() as f64;
    let reference = reference_lra(&buf, sr);
    println!("12 s fade: LRA {lra:.4} LU, independent 10 Hz reference {reference:.4} LU");
    assert!(
        (lra - reference).abs() < 0.35,
        "LRA {lra} against a reference of {reference}"
    );
}

// ---------------------------------------------------------------------------
// Independent reference implementations, written straight from the equations
// ---------------------------------------------------------------------------

/// K-weight and square, returning the per-sample channel-summed power.
fn k_weighted_power(buf: &[f32], sr: f64) -> Vec<f64> {
    let (shelf, hp) = k_weighting_coeffs(sr);
    let mut f = [
        (Biquad::new(shelf), Biquad::new(hp)),
        (Biquad::new(shelf), Biquad::new(hp)),
    ];
    (0..buf.len() / 2)
        .map(|i| {
            let l = f[0].1.process(f[0].0.process(buf[i * 2] as f64));
            let r = f[1].1.process(f[1].0.process(buf[i * 2 + 1] as f64));
            l * l + r * r
        })
        .collect()
}

/// BS.1770-4 Annex 1, equations (3)-(7), written out literally: 400 ms blocks
/// at a 100 ms step, an absolute gate at -70 LKFS and a relative gate 10 LU
/// below the absolutely-gated mean.
fn reference_integrated(buf: &[f32], sr: f64) -> f64 {
    let p = k_weighted_power(buf, sr);
    let block = (sr * 0.4).round() as usize;
    let step = (sr * 0.1).round() as usize;
    let mut zj = Vec::new();
    let mut start = 0;
    while start + block <= p.len() {
        zj.push(p[start..start + block].iter().sum::<f64>() / block as f64);
        start += step;
    }
    let lufs = |z: f64| -0.691 + 10.0 * z.log10();
    let above_abs: Vec<f64> = zj.iter().copied().filter(|&z| lufs(z) > -70.0).collect();
    if above_abs.is_empty() {
        return f64::NEG_INFINITY;
    }
    let gamma_r = lufs(above_abs.iter().sum::<f64>() / above_abs.len() as f64) - 10.0;
    let kept: Vec<f64> = above_abs
        .iter()
        .copied()
        .filter(|&z| lufs(z) > gamma_r)
        .collect();
    if kept.is_empty() {
        return f64::NEG_INFINITY;
    }
    lufs(kept.iter().sum::<f64>() / kept.len() as f64)
}

/// EBU Tech 3342 section 5, transcribed from the MATLAB listing: 3 s windows at
/// 10 Hz, an absolute gate at -70 LUFS, a relative gate 20 LU down and the
/// 10th/95th percentiles by `round((n-1) * p)`.
fn reference_lra(buf: &[f32], sr: f64) -> f64 {
    let p = k_weighted_power(buf, sr);
    let block = (sr * 3.0).round() as usize;
    let step = (sr * 0.1).round() as usize;
    let lufs = |z: f64| -0.691 + 10.0 * z.log10();
    let mut stl = Vec::new();
    let mut start = 0;
    while start + block <= p.len() {
        stl.push(lufs(
            p[start..start + block].iter().sum::<f64>() / block as f64,
        ));
        start += step;
    }
    let abs_gated: Vec<f64> = stl.iter().copied().filter(|&l| l >= -70.0).collect();
    if abs_gated.len() < 2 {
        return 0.0;
    }
    let power: f64 =
        abs_gated.iter().map(|l| 10f64.powf(l / 10.0)).sum::<f64>() / abs_gated.len() as f64;
    let integrated = 10.0 * power.log10();
    let mut kept: Vec<f64> = abs_gated
        .into_iter()
        .filter(|&l| l >= integrated - 20.0)
        .collect();
    if kept.len() < 2 {
        return 0.0;
    }
    kept.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = kept.len();
    let idx = |q: f64| ((n - 1) as f64 * q).round() as usize;
    kept[idx(0.95)] - kept[idx(0.10)]
}

/// The gating machinery, checked block-for-block against equations (3)-(7) on
/// material that is not a steady tone (so any mistake in block placement,
/// overlap or gate ordering shows up).
#[test]
fn gating_matches_the_bs1770_equations_on_dynamic_material() {
    for sr in RATES {
        let mut buf = Vec::new();
        // Level steps that straddle both gates, on a deliberately awkward grid.
        let plan: [(f64, f64); 8] = [
            (-23.0, 4.3),
            (-75.0, 2.1),
            (-14.0, 1.7),
            (-40.0, 3.3),
            (-23.0, 5.9),
            (-60.0, 2.5),
            (-18.0, 4.1),
            (-31.0, 6.7),
        ];
        for &(level, secs) in &plan {
            push_tone(&mut buf, level, secs, sr);
        }
        let mut m = LoudnessMeter::new(sr);
        m.process(&buf);
        let got = m.integrated() as f64;
        let want = reference_integrated(&buf, sr);
        println!("gating @{sr:.0}: meter {got:+.4} LUFS, BS.1770-4 eq (3)-(7) {want:+.4} LUFS");
        assert!(
            (got - want).abs() < 0.05,
            "@{sr}: meter {got} vs reference {want}"
        );
        let got_lra = m.lra() as f64;
        let want_lra = reference_lra(&buf, sr);
        println!("   LRA: meter {got_lra:.4} LU, Tech 3342 s5 reference {want_lra:.4} LU");
        assert!(
            (got_lra - want_lra).abs() < 0.5,
            "@{sr}: LRA {got_lra} vs reference {want_lra}"
        );
    }
}

// ---------------------------------------------------------------------------
// K-weighting across sample rates
// ---------------------------------------------------------------------------

/// The classic bug: 48 kHz coefficients applied verbatim at another rate. This
/// test measures the same programme at every supported rate and additionally
/// quantifies what the bug *would* cost, so the test cannot pass vacuously.
#[test]
fn k_weighting_is_redesigned_for_every_sample_rate() {
    // A pink-ish multi-tone: the K-weighting curve is only flat in the middle,
    // so an incorrectly warped filter shows up as a level error.
    let tones = [40.0f64, 120.0, 400.0, 1_000.0, 3_000.0, 8_000.0, 15_000.0];
    let mut readings = Vec::new();
    for sr in [
        44_100.0f64,
        48_000.0,
        88_200.0,
        96_000.0,
        176_400.0,
        192_000.0,
    ] {
        let n = (sr * 5.0) as usize;
        let mut buf = Vec::with_capacity(n * 2);
        for i in 0..n {
            let mut s = 0.0;
            for (k, f) in tones.iter().enumerate() {
                s += (std::f64::consts::TAU * f * i as f64 / sr + k as f64).sin();
            }
            let v = (s * 0.1) as f32;
            buf.push(v);
            buf.push(v);
        }
        let mut m = LoudnessMeter::new(sr);
        m.process(&buf);
        readings.push((sr, m.integrated()));
    }
    let reference = readings
        .iter()
        .find(|(sr, _)| *sr == 48_000.0)
        .map(|(_, v)| *v)
        .unwrap();
    let mut worst = 0.0f32;
    for (sr, v) in &readings {
        let d = v - reference;
        println!("multi-tone @{sr:.0}: {v:+.4} LUFS ({d:+.4} LU vs 48 kHz)");
        worst = worst.max(d.abs());
    }
    assert!(worst < 0.1, "cross-rate spread {worst} LU");

    // Negative control: what the same programme would read if the 48 kHz
    // coefficients were reused unchanged at 44.1 and 96 kHz.
    for sr in [44_100.0f64, 96_000.0] {
        let (shelf48, hp48) = k_weighting_coeffs(48_000.0);
        let (shelf, hp) = k_weighting_coeffs(sr);
        let mut worst_db: f64 = 0.0;
        for f in [40.0f64, 120.0, 400.0, 1_000.0, 3_000.0, 8_000.0, 15_000.0] {
            let right = shelf.magnitude_db(sr, f) + hp.magnitude_db(sr, f);
            let wrong = shelf48.magnitude_db(sr, f) + hp48.magnitude_db(sr, f);
            worst_db = worst_db.max((right - wrong).abs());
        }
        println!(
            "negative control @{sr:.0}: reusing the 48 kHz table would shift the \
             K-weighting curve by up to {worst_db:.3} dB"
        );
        assert!(
            worst_db > 0.05,
            "the cross-rate test has no teeth at {sr} Hz"
        );
    }
}
