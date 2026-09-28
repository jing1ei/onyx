//! Adversarial validation of the A/B alignment estimator (SPEC §11).
//!
//! The estimator claims sample accuracy and gates on a confidence figure, so
//! these are the cases it actually meets in a mastering session rather than the
//! easy one (identical audio, shifted). Every case states the true offset and
//! asserts the estimate against it; where the honest answer is "refuse", that
//! is asserted too, because a confident wrong answer is worse than no answer.
//!
//! The programme material is synthesised: broadband noise shaped into bars and
//! beats, then put through the transformation under test (gain, EQ, a real
//! compressor, polarity, silence). Nothing here compares the estimator against
//! itself.

use onyx_core::align::{estimate_from_pcm, estimate_offset, AlignEstimate, MIN_CONFIDENCE};
use onyx_core::dsp::biquad::{Biquad, Coeffs};
use onyx_core::pcm::SharedPcm;
use onyx_core::types::FilterKind;

const RATE: u32 = 48_000;

/* ── material ────────────────────────────────────────────────────────────── */

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / 8_388_608.0 - 1.0
    }
    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// Something that behaves like music: broadband content, a bar-length contour
/// and transients on the beat, so both the envelope stage and the full-rate
/// stage have real features to lock onto.
///
/// Tempo, key and contour all derive from `seed`, so two different seeds are
/// two different *pieces*, not the same arrangement with different noise. (An
/// earlier version of this fixture kept the tempo and the bass note fixed;
/// two "unrelated" takes then shared a phase-locked 110 Hz sine and correlated
/// at 0.81, which says nothing about the estimator.)
fn programme(secs: f64, seed: u64) -> Vec<f32> {
    programme_at(secs, seed, RATE)
}

/// As [`programme`], at an arbitrary sample rate, so the estimator can be
/// checked somewhere other than 48 kHz (every one of its window lengths is
/// derived from the rate).
fn programme_at(secs: f64, seed: u64, rate: u32) -> Vec<f32> {
    let n = (secs * rate as f64) as usize;
    let mut rng = Rng::new(seed);
    let mut out = Vec::with_capacity(n);
    let bpm = 84.0 + (seed % 7) as f64 * 11.0;
    let beat = (rate as f64 * 60.0 / bpm) as usize;
    let root = 55.0 * 2f32.powf((seed % 12) as f32 / 12.0);
    let bar_hz = 0.19 + (seed % 5) as f32 * 0.07;
    for i in 0..n {
        let phase = (i % beat) as f32 / beat as f32;
        // Percussive envelope on each beat plus a slow bar contour.
        let hit = (-6.0 * phase).exp();
        let bar = 0.35 + 0.65 * (i as f32 / rate as f32 * bar_hz).sin().abs();
        let tone = (2.0 * std::f32::consts::PI * root * i as f32 / rate as f32).sin() * 0.3;
        out.push((rng.next_f32() * 0.55 * hit + tone * bar) * 0.5 * bar);
    }
    out
}

/// Delay by `shift` samples (positive = later in its own file).
fn delayed(src: &[f32], shift: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; shift];
    out.extend_from_slice(src);
    out
}

fn gain(src: &[f32], db: f32) -> Vec<f32> {
    let g = 10f32.powf(db / 20.0);
    src.iter().map(|s| s * g).collect()
}

fn inverted(src: &[f32]) -> Vec<f32> {
    src.iter().map(|s| -s).collect()
}

/// A different *master*: EQ moves, so the two files no longer share a spectrum.
fn remastered_eq(src: &[f32]) -> Vec<f32> {
    let fs = RATE as f64;
    let mut chain = [
        Biquad::new(Coeffs::design(FilterKind::HighPass, fs, 45.0, 0.707, 0.0)),
        Biquad::new(Coeffs::design(FilterKind::LowShelf, fs, 120.0, 0.7, 4.5)),
        Biquad::new(Coeffs::design(FilterKind::Bell, fs, 450.0, 2.0, -3.5)),
        Biquad::new(Coeffs::design(FilterKind::HighShelf, fs, 6_500.0, 0.7, 5.0)),
    ];
    src.iter()
        .map(|s| {
            let mut y = *s as f64;
            for b in chain.iter_mut() {
                y = b.process(y);
            }
            y as f32
        })
        .collect()
}

/// A real feed-forward compressor: 4:1 above -24 dBFS, 5 ms attack, 120 ms
/// release, then make-up. This changes the *envelope*, which is what the
/// coarse stage correlates on, so it is the transformation most likely to
/// break the estimator.
fn compressed(src: &[f32]) -> Vec<f32> {
    let attack = (-1.0f64 / (0.005 * RATE as f64)).exp();
    let release = (-1.0f64 / (0.120 * RATE as f64)).exp();
    let threshold_db = -24.0f64;
    let ratio = 4.0f64;
    let mut env = 0.0f64;
    let mut out = Vec::with_capacity(src.len());
    for s in src {
        let x = (*s as f64).abs();
        let coeff = if x > env { attack } else { release };
        env = coeff * env + (1.0 - coeff) * x;
        let level_db = 20.0 * env.max(1e-9).log10();
        let over = (level_db - threshold_db).max(0.0);
        let gain_db = -over * (1.0 - 1.0 / ratio) + 6.0;
        out.push((*s as f64 * 10f64.powf(gain_db / 20.0)) as f32);
    }
    out
}

fn to_pcm(mono: &[f32]) -> std::sync::Arc<SharedPcm> {
    let stereo: Vec<f32> = mono.iter().flat_map(|s| [*s, *s]).collect();
    SharedPcm::from_interleaved(2, &stereo)
}

fn estimate(a: &[f32], b: &[f32]) -> AlignEstimate {
    estimate_offset(a, b, RATE).unwrap()
}

/* ── the cases ───────────────────────────────────────────────────────────── */

/// The two files differ only in level. A normalised cross-correlation is
/// scale-invariant by construction, so this must be exact at any gain the
/// f32 buffer can hold.
#[test]
fn a_level_difference_does_not_move_the_answer() {
    let a = programme(12.0, 1);
    for db in [-40.0f32, -12.0, -0.5, 6.0, 18.0] {
        let b = delayed(&gain(&a, db), 4_321);
        let est = estimate(&a, &b);
        assert_eq!(est.offset_frames, 4_321, "{db:+} dB moved the estimate");
        assert!(
            est.confidence > 0.99,
            "{db:+} dB dropped confidence to {}",
            est.confidence
        );
    }
}

/// Two different masters: EQ *and* compression, i.e. the case the feature
/// exists for. Sample accuracy is still required; the confidence is expected
/// to fall, and the figure is printed so it can be judged rather than assumed.
#[test]
fn eq_and_compression_still_align_to_the_sample() {
    let a = programme(20.0, 2);
    let eq_only = estimate(&a, &delayed(&remastered_eq(&a), 1_500));
    println!(
        "EQ only:            offset {} (true 1500), confidence {:.3}",
        eq_only.offset_frames, eq_only.confidence
    );
    assert_eq!(eq_only.offset_frames, 1_500);
    assert!(eq_only.is_confident());

    let comp_only = estimate(&a, &delayed(&compressed(&a), 1_500));
    println!(
        "compression only:   offset {} (true 1500), confidence {:.3}",
        comp_only.offset_frames, comp_only.confidence
    );
    assert_eq!(comp_only.offset_frames, 1_500);
    assert!(comp_only.is_confident());

    let both = estimate(&a, &delayed(&compressed(&remastered_eq(&a)), 1_500));
    println!(
        "EQ + compression:   offset {} (true 1500), confidence {:.3}",
        both.offset_frames, both.confidence
    );
    assert_eq!(both.offset_frames, 1_500);
    assert!(
        both.is_confident(),
        "a remaster dropped confidence to {}",
        both.confidence
    );
    // Confidence has to be *informative*: a genuinely different master must
    // not score as high as a bit-identical copy, or the number means nothing.
    let identical = estimate(&a, &delayed(&a, 1_500));
    println!(
        "identical copy:     confidence {:.4} (remaster {:.4})",
        identical.confidence, both.confidence
    );
    assert!(
        both.confidence < identical.confidence,
        "a remaster scored {:.4}, an identical copy {:.4} - the confidence \
         figure is not discriminating",
        both.confidence,
        identical.confidence
    );
}

/// Polarity inversion has to be reported, not silently absorbed, and the
/// offset must still be exact.
#[test]
fn polarity_inversion_is_flagged_at_every_stage() {
    let a = programme(12.0, 3);
    for &shift in &[0usize, 97, 12_000] {
        let b = delayed(&inverted(&gain(&a, -6.0)), shift);
        let est = estimate(&a, &b);
        assert_eq!(est.offset_frames, shift as i64);
        assert!(est.polarity_inverted, "polarity flip missed at {shift}");
        assert!(est.confidence > 0.99);
    }
    // ... and not reported when it is not there.
    let est = estimate(&a, &delayed(&a, 97));
    assert!(!est.polarity_inverted);
}

/// A long silent (and a long *quiet*) intro is the classic way to make a
/// correlator confident about noise. The estimator picks the loudest window of
/// A for the fine stage precisely to avoid this.
#[test]
fn a_long_quiet_intro_does_not_fool_it() {
    let core = programme(20.0, 4);
    // 8 s of digital black in front of B.
    let silent_intro = delayed(&core, 8 * RATE as usize);
    let est = estimate(&core, &silent_intro);
    println!(
        "8 s silent intro:   offset {} (true {}), confidence {:.3}",
        est.offset_frames,
        8 * RATE,
        est.confidence
    );
    assert_eq!(est.offset_frames, 8 * RATE as i64);
    assert!(est.is_confident());

    // 8 s of very quiet material (-54 dB) in front of B instead of silence:
    // there *is* something to lock onto, and it is the wrong thing.
    let mut quiet_intro = gain(&programme(8.0, 44), -54.0);
    quiet_intro.extend_from_slice(&core);
    let est = estimate(&core, &quiet_intro);
    println!(
        "8 s quiet intro:    offset {} (true {}), confidence {:.3}",
        est.offset_frames,
        8 * RATE,
        est.confidence
    );
    assert_eq!(est.offset_frames, 8 * RATE as i64);
    assert!(est.is_confident());
}

/// Genuinely different music. The only acceptable behaviours are a low
/// confidence or an error; a confident offset here would silently mis-align
/// two unrelated files.
#[test]
fn different_material_is_refused() {
    for (seed_a, seed_b) in [(5u64, 6u64), (7, 8), (9, 10)] {
        let a = programme(15.0, seed_a);
        let b = programme(15.0, seed_b);
        let est = estimate(&a, &b);
        println!(
            "unrelated {seed_a}/{seed_b}: offset {}, confidence {:.4}",
            est.offset_frames, est.confidence
        );
        assert!(
            !est.is_confident(),
            "unrelated material reported confidence {:.3} (gate is {MIN_CONFIDENCE})",
            est.confidence
        );
    }
    // A pure tone against noise: same length, plenty of energy, nothing in
    // common. Also must not be confident.
    let a = programme(15.0, 11);
    let b: Vec<f32> = (0..a.len())
        .map(|i| 0.4 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / RATE as f32).sin())
        .collect();
    let est = estimate(&a, &b);
    println!("tone vs programme:  confidence {:.4}", est.confidence);
    assert!(!est.is_confident());
}

/// Sub-millisecond offsets are the ones that matter for a null test: 1 sample
/// at 48 kHz is 21 microseconds, and the coarse envelope stage cannot see it
/// at all (its hop is 96 samples), so this exercises the fine stage alone.
#[test]
fn sub_millisecond_offsets_are_exact() {
    let a = programme(12.0, 12);
    for shift in [1i64, 2, 5, 11, 47, 48] {
        let est = estimate(&a, &delayed(&a, shift as usize));
        assert_eq!(
            est.offset_frames,
            shift,
            "a {shift}-sample offset ({:.3} ms) read as {}",
            shift as f64 * 1_000.0 / RATE as f64,
            est.offset_frames
        );
        assert!(est.confidence > 0.99);
    }
}

/// Both signs, and out to the +/-30 s clamp the engine applies
/// (`MAX_AB_OFFSET_SECS`). The estimator only ever looks at the first
/// `MAX_ANALYSIS_SECS` = 60 s of each deck, so a 30 s offset leaves just 30 s
/// of overlap - the case where a mis-derived search window silently returns
/// zero confidence.
#[test]
fn large_offsets_of_both_signs() {
    let core = programme(75.0, 13);
    for &secs in &[0.5f64, 5.0, 15.0, 25.0, 29.0] {
        let shift = (secs * RATE as f64) as usize;
        // B later than A.
        let b = delayed(&core, shift);
        let est = estimate(&core, &b);
        println!(
            "+{secs:>5.1} s: offset {:>9} (true {:>9}), confidence {:.3}",
            est.offset_frames, shift, est.confidence
        );
        assert_eq!(est.offset_frames, shift as i64, "+{secs} s");
        assert!(est.is_confident(), "+{secs} s scored {}", est.confidence);

        // A later than B: the offset must be exactly the negative.
        let est = estimate(&b, &core);
        println!(
            "-{secs:>5.1} s: offset {:>9} (true {:>9}), confidence {:.3}",
            est.offset_frames,
            -(shift as i64),
            est.confidence
        );
        assert_eq!(est.offset_frames, -(shift as i64), "-{secs} s");
        assert!(est.is_confident(), "-{secs} s scored {}", est.confidence);
    }
}

/// Idempotence, through the buffers the engine actually holds, with the two
/// decks differing the way two masters do. Tapping auto-align repeatedly must
/// return the identical estimate every time - the failure mode is an estimate
/// taken from B's already-offset read position, which doubles on every tap.
#[test]
fn repeated_estimates_are_identical() {
    let core = programme(20.0, 14);
    let a = to_pcm(&core);
    let b = to_pcm(&delayed(&compressed(&remastered_eq(&core)), 3_777));
    let mut applied = 0i64;
    let mut seen = Vec::new();
    for tap in 0..6 {
        let est = estimate_from_pcm(&a, &b, RATE).unwrap();
        assert!(est.is_confident(), "tap {tap} lost confidence");
        applied = est.offset_frames;
        seen.push(est);
    }
    assert_eq!(applied, 3_777);
    assert!(
        seen.windows(2).all(|w| w[0] == w[1]),
        "auto-align drifted: {seen:?}"
    );
}

/// Symmetry: swapping the decks must negate the offset exactly, at every
/// scale. An estimator that is off by one in one direction only will fail
/// here even when both directions individually look plausible.
#[test]
fn swapping_the_decks_negates_the_offset() {
    let core = programme(20.0, 15);
    for &shift in &[3i64, 500, 44_100, 480_000] {
        let b = delayed(&core, shift as usize);
        let forward = estimate(&core, &b);
        let backward = estimate(&b, &core);
        assert_eq!(forward.offset_frames, shift);
        assert_eq!(backward.offset_frames, -shift);
    }
}

/* ── cases added while fixing the periodic-material failure ──────────────── */

/// A **perfect loop**: the same four seconds five times over, offset by 3 000
/// samples. Offset + one loop length nulls exactly as well as the offset
/// itself, so the audio genuinely does not say which one is meant. The only
/// honest answer is a refusal, and this is the case that proves the confidence
/// figure is measuring something: the estimator's *peak* correlation here is
/// 1.0, and it still must not apply it.
#[test]
fn a_perfect_loop_is_genuinely_ambiguous_and_is_refused() {
    let loop_len = 4 * RATE as usize;
    let cell = programme(4.0, 21);
    let mut core = Vec::new();
    for _ in 0..5 {
        core.extend_from_slice(&cell);
    }
    let est = estimate(&core, &delayed(&core, 3_000));
    println!(
        "perfect 4 s loop:   offset {} (3000 and {} are equally true), confidence {:.4}",
        est.offset_frames,
        3_000 + loop_len,
        est.confidence
    );
    // The offset it names must at least be *a* correct one, modulo the loop.
    let residue = est.offset_frames.rem_euclid(loop_len as i64);
    assert!(
        (residue - 3_000).abs() <= 1 || (residue - 3_000).abs() >= loop_len as i64 - 1,
        "offset {} is not 3000 modulo the 4 s loop",
        est.offset_frames
    );
    assert!(
        !est.is_confident(),
        "a perfectly looped file cannot be aligned unambiguously, yet it \
         reported confidence {:.3}",
        est.confidence
    );
}

/// Two versions at **different tempo** (here a tape-speed change of +2 %).
/// SPEC §11 is explicit that alignment is offset-only and does not
/// time-stretch, so there is no correct constant offset: any lag that lines up
/// the start is 2 % wrong a bar later. The required behaviour is a refusal,
/// not a plausible-looking number.
#[test]
fn a_tempo_difference_is_refused_rather_than_half_aligned() {
    let core = programme(20.0, 22);
    let mut stretched = Vec::with_capacity(core.len());
    let ratio = 1.02f64;
    let mut pos = 0.0f64;
    while (pos as usize) + 1 < core.len() {
        let i = pos as usize;
        let frac = (pos - i as f64) as f32;
        stretched.push(core[i] * (1.0 - frac) + core[i + 1] * frac);
        pos += ratio;
    }
    let est = estimate(&core, &delayed(&stretched, 5_000));
    println!(
        "+2 % tempo:         offset {}, confidence {:.4}",
        est.offset_frames, est.confidence
    );
    assert!(
        !est.is_confident(),
        "a 2 % tempo difference has no correct constant offset, yet it \
         reported confidence {:.3} at {} samples",
        est.confidence,
        est.offset_frames
    );
}

/// A **different edit**: B is the same master with four seconds cut out of the
/// middle. The head and the tail then need different offsets, so - exactly as
/// the README promises - the acceptable outcomes are a refusal, or an offset
/// that aligns one of the two sections exactly. What is *not* acceptable is a
/// confident offset that aligns neither.
#[test]
fn a_different_edit_aligns_one_section_or_refuses() {
    let core = programme(24.0, 23);
    let cut_at = 10 * RATE as usize;
    let cut_len = 4 * RATE as usize;
    let mut edited = core[..cut_at].to_vec();
    edited.extend_from_slice(&core[cut_at + cut_len..]);
    let head_offset = 2_000i64;
    let est = estimate(&core, &delayed(&edited, head_offset as usize));
    // The head sits at +2 000; everything after the cut sits 4 s earlier.
    let tail_offset = head_offset - cut_len as i64;
    println!(
        "4 s edit removed:   offset {} (head {head_offset}, tail {tail_offset}), confidence {:.4}",
        est.offset_frames, est.confidence
    );
    if est.is_confident() {
        assert!(
            est.offset_frames == head_offset || est.offset_frames == tail_offset,
            "confident ({:.3}) at {}, which aligns neither the head ({head_offset}) \
             nor the tail ({tail_offset})",
            est.confidence,
            est.offset_frames
        );
    }
}

/// Every window length in the estimator is derived from the sample rate, and
/// the band-limited coarse stage picks its decimation from it too. 44.1 kHz
/// gives a non-integer decimation ratio, which is the case most likely to be
/// off by one.
#[test]
fn it_works_at_44_1_khz_too() {
    const R: u32 = 44_100;
    let core = programme_at(15.0, 24, R);
    for &shift in &[7i64, 1_411, 132_300] {
        let est = estimate_offset(&core, &delayed(&core, shift as usize), R).unwrap();
        println!(
            "44.1 kHz +{shift:>7}: offset {:>7}, confidence {:.4}",
            est.offset_frames, est.confidence
        );
        assert_eq!(est.offset_frames, shift);
        assert!(est.confidence > 0.99);
    }
}

/// A sweep rather than a handful of hand-picked numbers: twelve pieces, each
/// with its own tempo and key, each remastered (EQ + compression) and offset
/// by an arbitrary amount. Sample-exact every time, or the estimator is
/// over-fitted to the cases above.
#[test]
fn a_sweep_of_pieces_and_offsets_is_sample_exact() {
    let mut worst = 1.0f32;
    for seed in 30u64..42 {
        let core = programme(15.0, seed);
        let shift = (seed as i64 * 977) % 96_000 + 13;
        let b = delayed(&compressed(&remastered_eq(&core)), shift as usize);
        let est = estimate(&core, &b);
        assert_eq!(
            est.offset_frames, shift,
            "seed {seed}: {} instead of {shift} (confidence {:.3})",
            est.offset_frames, est.confidence
        );
        assert!(
            est.is_confident(),
            "seed {seed} scored {:.3}",
            est.confidence
        );
        worst = worst.min(est.confidence);
    }
    println!("sweep of 12 remasters: all sample-exact, worst confidence {worst:.3}");
}

/// The wide validation behind the cases above: sixty pieces × four master
/// treatments × both deck orders, with polarity flipped on every third piece
/// and a pseudo-random offset each time. It asserts the two things that
/// matter (never confidently wrong, never refusing a correct answer) over 480
/// cases plus 60 unrelated pairs, which is what stops the estimator being
/// tuned to the handful of offsets hard-coded above.
///
/// Ignored by default because it is ~540 estimates: run it with
/// `cargo test -p onyx-core --release --test align_adversarial -- --ignored --nocapture`.
#[test]
#[ignore = "wide sweep: ~540 estimates, run explicitly in --release"]
fn a_sixty_piece_sweep_is_never_confidently_wrong() {
    /// As `remastered_eq`, with the moves varying per piece.
    fn remastered_eq_varied(src: &[f32], seed: u64) -> Vec<f32> {
        let fs = RATE as f64;
        let g = 3.0 + (seed % 5) as f64;
        let mut chain = [
            Biquad::new(Coeffs::design(
                FilterKind::HighPass,
                fs,
                30.0 + g * 4.0,
                0.707,
                0.0,
            )),
            Biquad::new(Coeffs::design(FilterKind::LowShelf, fs, 120.0, 0.7, g)),
            Biquad::new(Coeffs::design(FilterKind::Bell, fs, 450.0, 2.0, -g)),
            Biquad::new(Coeffs::design(FilterKind::HighShelf, fs, 6_500.0, 0.7, g)),
        ];
        src.iter()
            .map(|s| {
                let mut y = *s as f64;
                for b in chain.iter_mut() {
                    y = b.process(y);
                }
                y as f32
            })
            .collect()
    }

    let (mut cases, mut false_accepts, mut false_refusals) = (0u32, 0u32, 0u32);
    let mut worst_correct = 1.0f32;
    for seed in 100u64..160 {
        let core = programme(15.0, seed);
        let shift = (Rng::new(seed ^ 0xABCD).next_u64() % 200_000) as i64 + 1;
        let invert_this_one = seed % 3 == 0;
        for variant in 0..4 {
            let treated = match variant {
                0 => core.clone(),
                1 => remastered_eq_varied(&core, seed),
                2 => compressed(&core),
                _ => compressed(&remastered_eq_varied(&core, seed)),
            };
            let treated = if invert_this_one {
                inverted(&treated)
            } else {
                treated
            };
            let b = delayed(&treated, shift as usize);
            for swapped in [false, true] {
                let (x, y, truth) = if swapped {
                    (&b, &core, -shift)
                } else {
                    (&core, &b, shift)
                };
                let est = estimate(x, y);
                cases += 1;
                let wrong = est.offset_frames != truth;
                if est.is_confident() && wrong {
                    false_accepts += 1;
                    println!(
                        "FALSE ACCEPT seed {seed} variant {variant} swapped {swapped}: \
                         {} instead of {truth}, confidence {:.3}",
                        est.offset_frames, est.confidence
                    );
                }
                if !est.is_confident() && !wrong {
                    false_refusals += 1;
                    println!(
                        "FALSE REFUSAL seed {seed} variant {variant} swapped {swapped}: \
                         correct offset {truth} scored only {:.3}",
                        est.confidence
                    );
                }
                if !wrong {
                    worst_correct = worst_correct.min(est.confidence);
                    assert_eq!(
                        est.polarity_inverted, invert_this_one,
                        "seed {seed} variant {variant}: polarity misreported"
                    );
                }
            }
        }
    }
    println!(
        "sweep: {cases} matching cases, {false_accepts} false accepts, {false_refusals} \
         false refusals, lowest confidence on a correct answer {worst_correct:.3}"
    );
    assert_eq!(false_accepts, 0);
    assert_eq!(false_refusals, 0);

    let mut highest_unrelated = 0.0f32;
    for seed in 200u64..260 {
        let est = estimate(&programme(15.0, seed), &programme(15.0, seed + 977));
        highest_unrelated = highest_unrelated.max(est.confidence);
        assert!(
            !est.is_confident(),
            "unrelated pieces {seed}/{} scored {:.3}",
            seed + 977,
            est.confidence
        );
    }
    println!("sweep: 60 unrelated pairs, highest confidence {highest_unrelated:.3}");
}
