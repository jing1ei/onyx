//! Sample-rate-conversion quality, measured rather than assumed.
//!
//! Round one of the audit fixed the resampler's *length* and *alignment* (the
//! rubato tail and group delay). This file measures what it actually does to
//! the signal: THD+N, alias rejection, passband flatness, DC and level, for the
//! conversions Onyx really performs (44.1<->48, 48->96, 96->44.1), plus the
//! hard product requirement that a file already at the device rate is passed
//! through untouched, sample for sample (SPEC §7).
//!
//! References for the numbers being asked for:
//!   * AES17-2020 §4.2 defines THD+N as the ratio of the residual (everything
//!     but the fundamental) to the total, which is how it is computed here.
//!   * There is no ITU/EBU tolerance for a *player's* SRC, so the thresholds
//!     below are stated as engineering requirements rather than compliance
//!     limits, and every measured figure is printed so a regression shows up as
//!     a number and not just as a pass/fail.

use onyx_core::decode::{open, DecodeHandle, DEFAULT_DECK_BUDGET_BYTES};
use rustfft::{num_complex::Complex64, FftPlanner};
use std::f64::consts::PI;
use std::io::Write;
use std::path::{Path, PathBuf};

/* ── fixtures ────────────────────────────────────────────────────────────── */

/// 32-bit float WAV, so the *source* is not what limits the measurement:
/// 16-bit PCM would floor THD+N at about -96 dB and hide everything.
fn write_wav_f32(path: &Path, sample_rate: u32, channels: u16, samples: &[f32]) {
    let data_len = (samples.len() * 4) as u32;
    let mut f = std::fs::File::create(path).unwrap();
    let mut w = |b: &[u8]| f.write_all(b).unwrap();
    w(b"RIFF");
    w(&(36 + data_len).to_le_bytes());
    w(b"WAVEfmt ");
    w(&16u32.to_le_bytes());
    w(&3u16.to_le_bytes()); // WAVE_FORMAT_IEEE_FLOAT
    w(&channels.to_le_bytes());
    w(&sample_rate.to_le_bytes());
    w(&(sample_rate * channels as u32 * 4).to_le_bytes());
    w(&(channels * 4).to_le_bytes());
    w(&32u16.to_le_bytes());
    w(b"data");
    w(&data_len.to_le_bytes());
    for s in samples {
        w(&s.to_le_bytes());
    }
    f.flush().unwrap();
}

fn tmp(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("onyx-src-{}-{name}", std::process::id()));
    p
}

fn wait_for(h: &DecodeHandle) {
    for _ in 0..1_200 {
        if h.status.is_finished() {
            assert!(h.status.error().is_none(), "{:?}", h.status.error());
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("decode did not finish");
}

/// Decode `samples` (interleaved stereo at `src_rate`) at `dst_rate` and return
/// the left channel of the result.
fn convert(name: &str, src_rate: u32, dst_rate: u32, samples: &[f32]) -> Vec<f64> {
    let path = tmp(name);
    write_wav_f32(&path, src_rate, 2, samples);
    let h = open(&path, dst_rate, DEFAULT_DECK_BUDGET_BYTES).unwrap();
    wait_for(&h);
    let n = h.pcm.frames_ready();
    let out: Vec<f64> = (0..n).map(|i| h.pcm.frame_stereo(i)[0] as f64).collect();
    let _ = std::fs::remove_file(&path);
    out
}

fn stereo_sine(freq: f64, amp: f64, rate: u32, secs: f64) -> Vec<f32> {
    let n = (rate as f64 * secs) as usize;
    (0..n)
        .flat_map(|i| {
            let s = (amp * (2.0 * PI * freq * i as f64 / rate as f64).sin()) as f32;
            [s, s]
        })
        .collect()
}

/* ── spectrum helpers ────────────────────────────────────────────────────── */

/// Blackman-Harris window: -92 dB sidelobes. Used only where there is no
/// strong on-bin fundamental to leak.
fn blackman_harris(n: usize) -> Vec<f64> {
    (0..n)
        .map(|i| {
            let x = 2.0 * PI * i as f64 / n as f64;
            0.35875 - 0.48829 * x.cos() + 0.14128 * (2.0 * x).cos() - 0.01168 * (3.0 * x).cos()
        })
        .collect()
}

fn fft_mag(buf: Vec<Complex64>) -> Vec<f64> {
    let n = buf.len();
    let mut buf = buf;
    FftPlanner::<f64>::new()
        .plan_fft_forward(n)
        .process(&mut buf);
    buf[..n / 2].iter().map(|c| c.norm()).collect()
}

fn spectrum_windowed(x: &[f64]) -> Vec<f64> {
    let win = blackman_harris(x.len());
    fft_mag(
        x.iter()
            .zip(win.iter())
            .map(|(v, w)| Complex64::new(v * w, 0.0))
            .collect(),
    )
}

/// Un-windowed spectrum. Only valid when the tone sits exactly on a bin, which
/// is how every measurement here is set up - then the fundamental leaks
/// *nothing* and the residual floor is limited by f32 storage (about -150 dB)
/// rather than by the window (about -95 dB, which is what a Blackman-Harris
/// measurement of a -6 dBFS tone bottoms out at, and is not a property of the
/// resampler at all).
fn spectrum_rect(x: &[f64]) -> Vec<f64> {
    fft_mag(x.iter().map(|v| Complex64::new(*v, 0.0)).collect())
}

/// Analysis length, and the exact-bin tone that goes with a given rate.
const ANALYSIS_N: usize = 1 << 16;

/// The frequency nearest 997 Hz that is an exact bin of an `ANALYSIS_N`-point
/// FFT at `rate` (AES17 §4.2.3 asks for ~997 Hz precisely because it is not
/// harmonically related to anything).
fn on_bin_997(rate: u32) -> (usize, f64) {
    on_bin(rate, 997.0)
}

/// The frequency nearest `nominal` that is an exact bin at `rate`.
fn on_bin(rate: u32, nominal: f64) -> (usize, f64) {
    let bin = (nominal * ANALYSIS_N as f64 / rate as f64).round() as usize;
    (bin, bin as f64 * rate as f64 / ANALYSIS_N as f64)
}

/// THD+N in dB (AES17-2020 §4.2: residual / total). Exact: with the tone on a
/// bin the only fundamental energy is in that one bin, and Parseval makes the
/// residual sum independent of how the distortion products themselves leak.
fn thd_n_db(x: &[f64], bin: usize) -> (f64, f64) {
    let mag = spectrum_rect(x);
    let mut total = 0.0;
    let mut residual = 0.0;
    let mut worst = 0.0f64;
    for (k, m) in mag.iter().enumerate() {
        let p = m * m;
        total += p;
        if k == bin || k < 2 {
            continue;
        }
        residual += p;
        worst = worst.max(*m);
    }
    let peak_db = 20.0 * (worst / mag[bin]).log10();
    (10.0 * (residual / total).log10(), peak_db)
}

/// Steady-state slice: skip the first and last half second.
fn steady(x: &[f64], rate: u32) -> Vec<f64> {
    let skip = (rate as usize) / 2;
    assert!(
        x.len() >= skip + ANALYSIS_N + skip,
        "not enough decoded audio to analyse"
    );
    x[skip..skip + ANALYSIS_N].to_vec()
}

const CONVERSIONS: [(u32, u32); 4] = [
    (44_100, 48_000),
    (48_000, 44_100),
    (48_000, 96_000),
    (96_000, 44_100),
];

/* ── 1. the bit-transparent path ─────────────────────────────────────────── */

/// SPEC §7: deck A follows the source rate, so a file already at the device
/// rate must reach the buffer untouched. Not "close": identical.
#[test]
fn a_source_at_the_device_rate_is_bit_transparent() {
    for rate in [44_100u32, 48_000, 96_000] {
        // Deliberately nasty: full-scale, broadband, both channels different,
        // so any filtering, gain or resampling at all shows up immediately.
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        let mut samples = Vec::with_capacity(rate as usize * 2);
        for _ in 0..rate as usize {
            for _ in 0..2 {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                samples.push(((state >> 40) as f32 / 8_388_608.0) - 1.0);
            }
        }
        let path = tmp(&format!("transparent-{rate}.wav"));
        write_wav_f32(&path, rate, 2, &samples);
        let h = open(&path, rate, DEFAULT_DECK_BUDGET_BYTES).unwrap();
        wait_for(&h);
        assert!(h.bit_transparent, "{rate} Hz was not flagged transparent");
        assert_eq!(h.pcm.frames_ready(), rate as usize);
        for i in 0..rate as usize {
            let got = h.pcm.frame_stereo(i);
            assert_eq!(
                got[0].to_bits(),
                samples[i * 2].to_bits(),
                "frame {i} left changed at {rate} Hz"
            );
            assert_eq!(got[1].to_bits(), samples[i * 2 + 1].to_bits());
        }
        let _ = std::fs::remove_file(&path);
    }
}

/* ── 2. THD+N ────────────────────────────────────────────────────────────── */

/// A 997 Hz tone at -6 dBFS through each real conversion. 997 Hz is the
/// standard non-harmonically-related test frequency (AES17 §4.2.3) so the
/// distortion products do not land on the analysis bins.
#[test]
fn thd_n_of_the_real_conversions() {
    // The measurement itself is validated first: the same analysis applied to
    // an ideal f32 sine has to sit far below anything the resampler does,
    // otherwise the numbers below would be measuring the FFT.
    let (bin, freq) = on_bin_997(48_000);
    let ideal: Vec<f64> = (0..ANALYSIS_N)
        .map(|i| (0.5 * (2.0 * PI * freq * i as f64 / 48_000.0).sin()) as f32 as f64)
        .collect();
    let (floor, _) = thd_n_db(&ideal, bin);
    println!("measurement floor (ideal f32 sine, same analysis): {floor:.1} dB");
    assert!(floor < -140.0, "the analysis floor is only {floor:.1} dB");

    for (src, dst) in CONVERSIONS {
        let amp = 0.5;
        let (bin, freq) = on_bin_997(dst);
        let x = convert(
            &format!("thd-{src}-{dst}.wav"),
            src,
            dst,
            &stereo_sine(freq, amp, src, 6.0),
        );
        let s = steady(&x, dst);
        let (thd, spur) = thd_n_db(&s, bin);
        println!(
            "{src} -> {dst}: THD+N {thd:.1} dB, worst residual bin {spur:.1} dBc \
             ({freq:.3} Hz at -6.0 dBFS)"
        );
        assert!(
            thd < -120.0,
            "{src} -> {dst} THD+N is only {thd:.1} dB (want < -120 dB)"
        );
        assert!(
            spur < -130.0,
            "{src} -> {dst} worst residual bin is {spur:.1} dBc (want < -130)"
        );
    }
}

/* ── 3. alias rejection ──────────────────────────────────────────────────── */

/// Downsampling must band-limit first. A tone above the *destination* Nyquist
/// has nowhere legal to go, so whatever comes out is alias, and its level is
/// the stopband rejection of the conversion filter.
#[test]
fn downsampling_rejects_content_above_the_destination_nyquist() {
    for (src, dst, tone) in [
        (96_000u32, 44_100u32, 30_000.0f64),
        (96_000, 44_100, 24_000.0),
        (48_000, 44_100, 23_000.0),
    ] {
        let amp = 0.5;
        let x = convert(
            &format!("alias-{src}-{dst}-{tone}.wav"),
            src,
            dst,
            &stereo_sine(tone, amp, src, 4.0),
        );
        let s = steady(&x, dst);
        let mag = spectrum_windowed(&s);
        // Calibrate FFT units against a known-amplitude tone at the same length.
        let cal = spectrum_windowed(
            &(0..s.len())
                .map(|i| amp * (2.0 * PI * 1_000.0 * i as f64 / dst as f64).sin())
                .collect::<Vec<f64>>(),
        );
        let unit = cal.iter().fold(0.0f64, |m, v| m.max(*v)) / amp;
        let worst = mag.iter().skip(3).fold(0.0f64, |m, v| m.max(*v));
        let level = 20.0 * (worst / unit).log10();
        println!("{src} -> {dst}, {tone} Hz in: worst output component {level:.1} dBFS");
        assert!(
            level < -100.0,
            "{src} -> {dst} let a {tone} Hz tone through at {level:.1} dBFS"
        );
    }
}

/* ── 4. passband flatness, level and DC ──────────────────────────────────── */

/// The conversion must not tilt the band, shift the level or add DC. The
/// tolerance on level is tight on purpose: a mastering engineer level-matches
/// A against B, and a resampler that costs 0.1 dB would show up as a
/// preference for the un-resampled deck.
#[test]
fn passband_is_flat_and_free_of_dc_and_level_shift() {
    for (src, dst) in CONVERSIONS {
        let usable = (src.min(dst) as f64) * 0.45;
        let mut worst_err = 0.0f64;
        let mut worst_freq = 0.0f64;
        let mut worst_dc = 0.0f64;
        for &nominal in &[20.0f64, 100.0, 997.0, 5_000.0, 10_000.0, 15_000.0, 19_000.0] {
            if nominal > usable {
                continue;
            }
            // Exactly on an analysis bin, so the window holds a whole number of
            // cycles: the RMS of the slice is then exactly A/sqrt(2) and its
            // mean is exactly zero for a correct conversion. Off-bin, a 20 Hz
            // tone's own half-cycle at the end of the window looks like 0.05 dB
            // of level error and 4e-3 of DC, and neither is real.
            let (_, f) = on_bin(dst, nominal);
            let amp = 0.5;
            let x = convert(
                &format!("flat-{src}-{dst}-{nominal}.wav"),
                src,
                dst,
                &stereo_sine(f, amp, src, 3.0),
            );
            let s = steady(&x, dst);
            let rms = (s.iter().map(|v| v * v).sum::<f64>() / s.len() as f64).sqrt();
            let err = 20.0 * (rms * 2f64.sqrt() / amp).log10();
            let dc = s.iter().sum::<f64>() / s.len() as f64;
            worst_dc = worst_dc.max(dc.abs());
            assert!(
                dc.abs() < 1e-6,
                "{src} -> {dst} at {f} Hz introduced {dc:.3e} of DC"
            );
            if err.abs() > worst_err.abs() {
                worst_err = err;
                worst_freq = f;
            }
        }
        println!(
            "{src} -> {dst}: worst DC offset {worst_dc:.2e} ({:.1} dBFS)",
            20.0 * worst_dc.max(1e-30).log10()
        );
        println!(
            "{src} -> {dst}: worst passband level error {worst_err:+.4} dB (at {worst_freq} Hz)"
        );
        assert!(
            worst_err.abs() < 0.01,
            "{src} -> {dst} is {worst_err:+.4} dB off at {worst_freq} Hz"
        );
    }
}

/// Where the conversion filter actually rolls off. rubato is configured with
/// `calculate_cutoff(256, BlackmanHarris2)`, so the passband ends a little
/// below the destination Nyquist; the exact figure matters to anyone who cares
/// whether an SRC eats the top of the band, so it is measured and printed
/// rather than left to the reader to infer from the parameters.
#[test]
fn transition_band_is_measured() {
    for (src, dst) in [(48_000u32, 44_100u32), (96_000, 44_100)] {
        let nyq = dst as f64 / 2.0;
        let mut edge_db = Vec::new();
        for &frac in &[0.80f64, 0.85, 0.90, 0.95, 0.98] {
            let (_, f) = on_bin(dst, nyq * frac);
            let amp = 0.5;
            let x = convert(
                &format!("edge-{src}-{dst}-{frac}.wav"),
                src,
                dst,
                &stereo_sine(f, amp, src, 3.0),
            );
            let s = steady(&x, dst);
            let rms = (s.iter().map(|v| v * v).sum::<f64>() / s.len() as f64).sqrt();
            let db = 20.0 * (rms * 2f64.sqrt() / amp).log10();
            edge_db.push((frac, f, db));
        }
        for (frac, f, db) in &edge_db {
            println!(
                "{src} -> {dst}: {:.0}% of Nyquist ({f:.0} Hz) is {db:+.3} dB",
                frac * 100.0
            );
        }
        // 20 kHz must survive a conversion to 44.1 kHz essentially intact.
        let twenty = edge_db
            .iter()
            .find(|(frac, _, _)| (*frac - 0.90).abs() < 1e-9)
            .unwrap();
        assert!(
            twenty.2 > -0.5,
            "{src} -> {dst} loses {:.2} dB at {:.0} Hz",
            -twenty.2,
            twenty.1
        );
    }
}
