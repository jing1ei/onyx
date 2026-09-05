//! The drawn EQ curve is computed twice: here in Rust (`dsp::eq::curve_db`,
//! which the engine also uses) and again in TypeScript at pointer rate
//! (`src/lib/eq.ts::compositeResponse`). SPEC §12 requires the drawn composite
//! to *be* the response the audio path has, so those two implementations are a
//! contract, and nothing in either language can check it.
//!
//! This file is one half of the check. It pins the Rust answer for a spread of
//! shapes, corners, Qs, gains, slopes and sample rates into
//! `tests/fixtures/eq_curve_reference.json`; `scripts/check-eq-curve.mjs`
//! transpiles the *real* `src/lib/eq.ts` and asserts the TypeScript agrees with
//! that same fixture. Change the Rust design and this test fails until the
//! fixture is regenerated; regenerate the fixture and the Node check fails
//! until the TypeScript is brought back in line.
//!
//! Regenerate with:
//!
//! ```text
//! ONYX_UPDATE_EQ_FIXTURE=1 cargo test -p onyx-core --test eq_curve_contract
//! ```

use onyx_core::dsp::eq::curve_db;
use onyx_core::types::{EqBand, EqConfig, FilterKind};
use std::path::PathBuf;

/// Frequencies the curve is sampled at: a 64-point log sweep over the drawn
/// axis, which is what `EqPanel.tsx` builds, plus the two ends exactly.
fn probes() -> Vec<f32> {
    let n = 32;
    let (lo, hi) = (20.0f64, 20_000.0f64);
    (0..n)
        .map(|i| (lo * (hi / lo).powf(i as f64 / (n - 1) as f64)) as f32)
        .collect()
}

fn band(kind: FilterKind, freq: f32, gain: f32, q: f32, slope: u8) -> EqBand {
    EqBand {
        id: 1,
        enabled: true,
        kind,
        freq_hz: freq,
        gain_db: gain,
        q,
        slope_db_oct: slope,
    }
}

/// `(name, sample rate, config)` — every shape across the parameter space the
/// UI can actually reach, then some real multi-band chains.
fn cases() -> Vec<(String, f64, EqConfig)> {
    const KINDS: [(&str, FilterKind); 7] = [
        ("bell", FilterKind::Bell),
        ("lowShelf", FilterKind::LowShelf),
        ("highShelf", FilterKind::HighShelf),
        ("highPass", FilterKind::HighPass),
        ("lowPass", FilterKind::LowPass),
        ("notch", FilterKind::Notch),
        ("bandPass", FilterKind::BandPass),
    ];
    let mut out = Vec::new();
    for &(label, kind) in &KINDS {
        for &fs in &[44_100.0f64, 48_000.0, 96_000.0] {
            for &freq in &[30.0f32, 1_000.0, 19_000.0] {
                for &q in &[0.1f32, 0.707_106_77, 40.0] {
                    for &gain in &[-30.0f32, 6.0, 30.0] {
                        if !kind.has_gain() && gain != -30.0 {
                            // Gain is not a control for this shape: run it once.
                            continue;
                        }
                        let gain = if kind.has_gain() { gain } else { 0.0 };
                        // 24 and 48 dB/oct only mean something for HP/LP; the
                        // other shapes are one section whatever the field says.
                        let slopes: &[u8] = if kind.has_slope() {
                            &[12, 24, 48]
                        } else {
                            &[12]
                        };
                        for &slope in slopes {
                            out.push((
                                format!("{label} fs={fs} f0={freq} q={q} g={gain} slope={slope}"),
                                fs,
                                EqConfig {
                                    enabled: true,
                                    bands: vec![band(kind, freq, gain, q, slope)],
                                },
                            ));
                        }
                    }
                }
            }
        }
    }

    // A mastering chain: everything at once, including a disabled band and a
    // cascaded pair, so the composite sum and the enabled/disabled filtering
    // are pinned too.
    let mut chain = EqConfig {
        enabled: true,
        bands: vec![
            band(FilterKind::HighPass, 28.0, 0.0, 0.9, 24),
            band(FilterKind::LowShelf, 110.0, 2.5, 0.6, 12),
            band(FilterKind::Bell, 320.0, -3.2, 1.8, 12),
            band(FilterKind::Bell, 3_400.0, 1.4, 0.9, 12),
            band(FilterKind::HighShelf, 9_000.0, 2.0, 0.707, 12),
            band(FilterKind::LowPass, 17_500.0, 0.0, 1.6, 48),
            band(FilterKind::Notch, 6_000.0, 0.0, 12.0, 12),
            band(FilterKind::BandPass, 1_000.0, 0.0, 2.0, 12),
        ],
    };
    for (i, b) in chain.bands.iter_mut().enumerate() {
        b.id = i as u32 + 1;
    }
    out.push(("chain: all eight shapes".into(), 48_000.0, chain.clone()));

    chain.bands[2].enabled = false;
    out.push(("chain: one band disabled".into(), 44_100.0, chain.clone()));

    chain.enabled = false;
    out.push(("chain: eq bypassed".into(), 48_000.0, chain));

    out
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/eq_curve_reference.json")
}

/// `f32` at its shortest round-trip decimal, so the fixture stays small and
/// still holds every bit the engine produced.
fn short(v: f32) -> serde_json::Value {
    serde_json::json!(v.to_string().parse::<f64>().expect("finite"))
}

fn build() -> serde_json::Value {
    let probes = probes();
    let mut curve = Vec::new();
    let cases: Vec<serde_json::Value> = cases()
        .into_iter()
        .map(|(name, sample_rate, config)| {
            curve_db(&config, sample_rate, &probes, &mut curve);
            serde_json::json!({
                "name": name,
                "sampleRate": sample_rate,
                "config": config,
                "curveDb": curve.iter().copied().map(short).collect::<Vec<_>>(),
            })
        })
        .collect();
    serde_json::json!({
        "note": "Generated by crates/onyx-core/tests/eq_curve_contract.rs from \
                 onyx_core::dsp::eq::curve_db — the engine's own curve. Checked \
                 against src/lib/eq.ts by scripts/check-eq-curve.mjs.",
        "regenerate": "ONYX_UPDATE_EQ_FIXTURE=1 cargo test -p onyx-core --test eq_curve_contract",
        // The TypeScript accumulates into a Float32Array, so it cannot be
        // expected to match f64 arithmetic exactly; 1e-3 dB is ~4 orders of
        // magnitude below anything visible on a 36 dB tall plot and 6 below
        // anything audible.
        "toleranceDb": 1.0e-3,
        "probeHz": probes.iter().copied().map(short).collect::<Vec<_>>(),
        "cases": cases,
    })
}

/// One line per case: a 460-case matrix pretty-printed the usual way is a
/// megabyte of indented single-digit lines, and nobody diffs it either way.
fn render(doc: &serde_json::Value) -> String {
    let mut out = String::from("{\n");
    for key in ["note", "regenerate", "toleranceDb", "probeHz"] {
        out.push_str(&format!(
            "  {}: {},\n",
            serde_json::to_string(key).expect("a key"),
            serde_json::to_string(&doc[key]).expect("serialisable")
        ));
    }
    out.push_str("  \"cases\": [\n");
    let cases = doc["cases"].as_array().expect("cases");
    for (i, case) in cases.iter().enumerate() {
        out.push_str("    ");
        out.push_str(&serde_json::to_string(case).expect("serialisable"));
        out.push_str(if i + 1 == cases.len() { "\n" } else { ",\n" });
    }
    out.push_str("  ]\n}\n");
    out
}

/// The fixture the front end is checked against must still be what the engine
/// computes.
#[test]
fn the_rust_curve_matches_the_pinned_reference() {
    let built = build();
    let path = fixture_path();

    if std::env::var("ONYX_UPDATE_EQ_FIXTURE").is_ok() {
        std::fs::create_dir_all(path.parent().expect("a parent directory"))
            .expect("could not create the fixture directory");
        let text = render(&built);
        std::fs::write(&path, text).expect("could not write the fixture");
        println!("wrote {}", path.display());
        return;
    }

    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "{} is missing ({e}); regenerate it with \
             ONYX_UPDATE_EQ_FIXTURE=1 cargo test -p onyx-core --test eq_curve_contract",
            path.display()
        )
    });
    let pinned: serde_json::Value = serde_json::from_str(&raw).expect("the fixture must be JSON");

    let built_cases = built["cases"].as_array().expect("cases");
    let pinned_cases = pinned["cases"].as_array().expect("pinned cases");
    assert_eq!(
        built_cases.len(),
        pinned_cases.len(),
        "the case list changed; regenerate the fixture"
    );
    assert_eq!(
        built["probeHz"], pinned["probeHz"],
        "the probe grid changed"
    );

    let mut worst = 0.0f64;
    let mut worst_where = String::new();
    for (built, pinned) in built_cases.iter().zip(pinned_cases.iter()) {
        assert_eq!(built["name"], pinned["name"], "case order changed");
        assert_eq!(
            built["config"], pinned["config"],
            "case `{}` describes a different filter",
            built["name"]
        );
        let a = built["curveDb"].as_array().expect("curve");
        let b = pinned["curveDb"].as_array().expect("pinned curve");
        assert_eq!(a.len(), b.len());
        for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
            let (x, y) = (x.as_f64().unwrap(), y.as_f64().unwrap());
            if !x.is_finite() && !y.is_finite() {
                continue;
            }
            let e = (x - y).abs();
            if e > worst {
                worst = e;
                worst_where = format!("{} at probe {i}", built["name"]);
            }
        }
    }
    println!("worst |engine curve - pinned fixture| = {worst:.3e} dB ({worst_where})");
    assert!(
        worst < 1e-9,
        "the engine's curve has moved away from the pinned reference by {worst} dB \
         ({worst_where}). If that is intended, regenerate the fixture and make \
         src/lib/eq.ts match: ONYX_UPDATE_EQ_FIXTURE=1 cargo test -p onyx-core \
         --test eq_curve_contract && node scripts/check-eq-curve.mjs"
    );

    // A fixture of flat lines would pass everything above and prove nothing.
    let span = built_cases
        .iter()
        .flat_map(|c| c["curveDb"].as_array().unwrap())
        .filter_map(|v| v.as_f64())
        .filter(|v| v.is_finite())
        .fold(0.0f64, |m, v| m.max(v.abs()));
    assert!(span > 30.0, "the reference curves are suspiciously flat");
    assert!(built_cases.len() > 400, "too few cases to be a contract");
}
