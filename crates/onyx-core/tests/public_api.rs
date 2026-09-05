//! Compile-time pin for the public surface of `onyx-core`.
//!
//! Contract: `SPEC.md` §1 for the core surface, plus `SPEC.md` §6 (monitor
//! matrix), §11 (A/B offset, polarity, auto-align) and §12 (dynamic EQ,
//! audition sweep, spectrum gating) where the v2 sections override it.
//!
//! The Tauri layer is written by a different author against exactly this list,
//! so a rename or a signature change here is a breaking change even though the
//! crate still compiles on its own. Nothing in this file is *called* against a
//! real device - the point is that it must keep type-checking, and that the
//! wire shapes the front end parses are asserted rather than assumed.

#![allow(dead_code, clippy::type_complexity)]

use std::path::Path;
use std::sync::Arc;

use onyx_core::align::{
    estimate_from_pcm, estimate_offset, AlignEstimate, ENVELOPE_HOP_SECS, FINE_SEARCH_SECS,
    FINE_WINDOW_SECS, MAX_ANALYSIS_SECS, MIN_ALIGN_SECS, MIN_CONFIDENCE,
};
use onyx_core::deck::{DeckSlot, DeckState};
use onyx_core::decode::{
    codecs, is_supported_path, open, open_with, probe, probe_with, DecodeHandle, DecodeOptions,
    DecodeStatus, DEFAULT_DECK_BUDGET_BYTES, SUPPORTED_EXTENSIONS,
};
use onyx_core::engine::{
    list_output_devices, AudioEngine, EngineConfig, RtShared, SourceRequest, FALLBACK_RATE,
    MAX_AB_OFFSET_SECS,
};
use onyx_core::midi::{BankInfo, BankSource, MidiOptions, BUNDLED_BANK_NAME};
use onyx_core::pcm::SharedPcm;
use onyx_core::waveform::{Waveform, WaveformData};
use onyx_core::{
    db_to_lin, latency_ms, lin_to_db, BufferRange, Deck, DeviceInfo, EngineSource, EqBand,
    EqConfig, FilterKind, HostInfo, LoudnessAnalysis, MeterSnapshot, MonitorMode, TrackInfo,
    TransportState, LUFS_SILENCE, MAX_BANDS, MAX_EQ_FREQ, MAX_EQ_GAIN_DB, MAX_EQ_Q,
    MAX_EQ_SECTIONS, MIN_DB, MIN_EQ_FREQ, MIN_EQ_Q, MONITOR_IDENTITY,
};

/// Free functions and constants, pinned as typed values.
fn _free_functions() {
    let _: fn(&Path, u32, usize) -> onyx_core::Result<DecodeHandle> = open;
    let _: fn(&Path, u32, usize, &DecodeOptions) -> onyx_core::Result<DecodeHandle> = open_with;
    let _: fn(&Path) -> onyx_core::Result<TrackInfo> = probe;
    let _: fn(&Path, &DecodeOptions) -> onyx_core::Result<TrackInfo> = probe_with;
    let _: fn(&Path) -> bool = is_supported_path;
    // Symphonia is not a dependency of this test crate, so the registry type
    // is pinned by inference rather than by name.
    let _registry = codecs();
    let _: fn() -> Vec<DeviceInfo> = list_output_devices;
    let _: fn(u32, u32) -> f32 = latency_ms;
    let _: fn(f32) -> f32 = db_to_lin;
    let _: fn(f32) -> f32 = lin_to_db;
    let _: fn(EngineConfig) -> onyx_core::Result<Arc<AudioEngine>> = AudioEngine::new;

    let _: usize = DEFAULT_DECK_BUDGET_BYTES;
    let _: &[&str] = SUPPORTED_EXTENSIONS;
    let _: u32 = FALLBACK_RATE;
    let _: f64 = MAX_AB_OFFSET_SECS;
    let _: f32 = MIN_DB;
    let _: f32 = LUFS_SILENCE;

    // SPEC §12 EQ limits.
    let _: usize = MAX_BANDS;
    let _: usize = MAX_EQ_SECTIONS;
    let _: f32 = MIN_EQ_FREQ;
    let _: f32 = MAX_EQ_FREQ;
    let _: f32 = MAX_EQ_GAIN_DB;
    let _: f32 = MIN_EQ_Q;
    let _: f32 = MAX_EQ_Q;

    // SPEC §6 monitor matrix.
    let _: [f32; 4] = MONITOR_IDENTITY;
}

/// `EngineConfig` is constructed by the shell, so its fields are public API.
fn _engine_config() {
    let cfg = EngineConfig {
        host_id: Some("coreaudio".into()),
        device_name: Some("hw:0".into()),
        follow_source_rate: true,
        fallback_rate: FALLBACK_RATE,
        buffer_frames: Some(256),
    };
    let _ = EngineConfig::default();
    let _: Option<String> = cfg.host_id;
    let _: Option<String> = cfg.device_name;
    let _: bool = cfg.follow_source_rate;
    let _: u32 = cfg.fallback_rate;
    let _: Option<u32> = cfg.buffer_frames;
}

/// SPEC §16: the engine-source surface the settings panel drives.
fn _engine_source_surface(e: &AudioEngine) {
    let _: Vec<HostInfo> = onyx_core::engine::list_hosts();
    let _: Vec<DeviceInfo> = onyx_core::engine::list_output_devices();
    let _: Vec<DeviceInfo> = onyx_core::engine::list_output_devices_for_host(Some("alsa"));
    let _: u32 = onyx_core::engine::DEFAULT_BUFFER_FRAMES;

    let _: Vec<HostInfo> = e.list_hosts();
    let _: Vec<DeviceInfo> = e.list_devices_for_host(None);
    let _: EngineConfig = e.config();
    let _: Option<EngineSource> = e.current_source();
    let _: Option<f32> = e.latency_ms();

    let request = SourceRequest {
        host_id: Some("wasapi".into()),
        device_name: Some("Studio Monitors".into()),
        use_system_default_device: false,
        sample_rate: Some(96_000),
        follow_source_rate: Some(true),
        buffer_frames: Some(128),
    };
    let _ = SourceRequest::default();
    let _: Result<EngineSource, onyx_core::Error> = e.set_source(&request);
    let _: Result<EngineSource, onyx_core::Error> = e.set_host(Some("alsa".into()));
    let _: Result<EngineSource, onyx_core::Error> = e.set_buffer_frames(512);
}

/// SPEC §18: what the app layer needs to drive MIDI rendering and to key
/// the loudness cache on the bank as well as the file.
fn _midi_surface(info: &TrackInfo, bank: &BankInfo) {
    let _: &str = BUNDLED_BANK_NAME;
    let opts = MidiOptions {
        soundfont: Some(std::path::PathBuf::from("/tmp/bank.sf2")),
    };
    let _ = MidiOptions::default();
    let _: Option<std::path::PathBuf> = opts.soundfont.clone();
    let _ = DecodeOptions { midi: opts };

    let _: &str = &bank.name;
    let _: &BankSource = &bank.source;
    let _: Option<&str> = bank.fallback_reason.as_deref();
    // The cache key ingredient: `path|size|mtime|<identity>`.
    let _: &str = bank.identity();
    let _ = matches!(bank.source, BankSource::Bundled | BankSource::User(_));

    // Reported on every track, `None` for everything that is not synthesised.
    let _: Option<&str> = info.synth_bank.as_deref();
    let _: Option<&str> = info.render_key.as_deref();
}

/// Every engine method the spec promises, with its exact signature.
fn _engine_surface(e: &AudioEngine, pcm: Arc<SharedPcm>) {
    // devices / rate
    let _: u32 = e.request_rate(44_100).unwrap();
    let _: u32 = e.set_device(None).unwrap();
    let _: Vec<DeviceInfo> = e.list_devices();
    let _: Option<String> = e.current_device();
    let _: u32 = e.engine_rate();
    e.set_follow_source_rate(true);
    let _: bool = e.follow_source_rate();

    // decks
    e.load_deck(Deck::A, pcm, 0.0f32);
    e.clear_deck(Deck::B);
    e.set_trim_db(Deck::A, -3.0f32);

    // transport
    e.play();
    e.pause();
    let _: bool = e.toggle();
    e.stop();
    e.seek_secs(1.0f64);
    e.seek_frames(48_000u64);

    // output
    e.set_volume(0.8f32);
    let _: f32 = e.volume();
    e.set_muted(true);
    let _: bool = e.muted();

    // loop
    e.set_loop_enabled(true);
    let _: bool = e.loop_enabled();
    e.set_loop_region(Some((0.0f64, 1.0f64)));
    let _: Option<(f64, f64)> = e.loop_region();

    // A/B
    e.set_ab_enabled(true);
    let _: bool = e.ab_enabled();
    e.select_deck(Deck::B);
    let _: Deck = e.active_deck();
    e.set_crossfade_ms(8.0f32);
    let _: f32 = e.crossfade_ms();

    // EQ (SPEC §12). One authoritative setter that echoes back what is
    // really running, plus the audition bandpass and the analyser gate.
    let _: EqConfig = e.set_eq(EqConfig::default());
    let _: EqConfig = e.eq_config();
    e.set_eq_audition(Some(2_000.0f32), 12.0f32);
    e.set_eq_audition(None, 12.0f32);
    let _: Option<(f32, f32)> = e.eq_audition();
    let _: Vec<f32> = e.eq_curve(&[100.0f32, 1_000.0]);
    e.set_spectrum_enabled(false);
    let _: bool = e.spectrum_enabled();

    // Monitor matrix (SPEC §6).
    e.set_monitor_mode(MonitorMode::Side);
    let _: MonitorMode = e.monitor_mode();

    // A/B alignment + polarity (SPEC §11).
    e.set_ab_offset_frames(-480i64);
    let _: i64 = e.ab_offset_frames();
    e.set_deck_invert(Deck::B, true);
    let _: bool = e.deck_inverted(Deck::B);

    // metering + shared state
    let _: MeterSnapshot = e.meters();
    e.reset_meters();
    let _: &Arc<RtShared> = e.shared();
}

fn _shared_surface(s: &RtShared) {
    let _: f64 = s.position_secs();
    let _: u64 = s.position_frames();
    let _: bool = s.is_playing();
    let _: bool = s.is_buffering();
    let _: u32 = s.underruns();
    let _: Deck = s.active_deck();
    let _: u32 = s.engine_rate();
    let _: bool = s.take_ended();
}

/// The alignment estimator (SPEC §11). The shell turns an [`AlignEstimate`]
/// plus an "applied" flag into the `AlignResult` the front end sees, so the
/// field names and the confidence gate are load-bearing.
fn _align_surface(a: &Arc<SharedPcm>, b: &Arc<SharedPcm>, e: &AlignEstimate) {
    let _: fn(&Arc<SharedPcm>, &Arc<SharedPcm>, u32) -> onyx_core::Result<AlignEstimate> =
        estimate_from_pcm;
    let _: fn(&[f32], &[f32], u32) -> onyx_core::Result<AlignEstimate> = estimate_offset;
    let _: onyx_core::Result<AlignEstimate> = estimate_from_pcm(a, b, 48_000);

    let _: i64 = e.offset_frames;
    let _: f32 = e.confidence;
    let _: bool = e.polarity_inverted;
    let _: bool = e.is_confident();

    let _: f64 = MIN_ALIGN_SECS;
    let _: f64 = MAX_ANALYSIS_SECS;
    let _: f64 = ENVELOPE_HOP_SECS;
    let _: f64 = FINE_SEARCH_SECS;
    let _: f64 = FINE_WINDOW_SECS;
    let _: f32 = MIN_CONFIDENCE;
}

fn _decode_surface(h: &DecodeHandle, st: &DecodeStatus, wf: &Waveform) {
    let _: &Arc<SharedPcm> = &h.pcm;
    let _: &Arc<Waveform> = &h.waveform;
    let _: &TrackInfo = &h.info;
    let _: &Arc<DecodeStatus> = &h.status;
    let _: u32 = h.stored_rate;
    let _: bool = h.bit_transparent;
    let _: f64 = h.duration_secs();

    let _: Option<LoudnessAnalysis> = st.analysis();
    let _: bool = st.is_finished();
    let _: bool = st.is_truncated();
    let _: bool = st.failed();
    let _: Option<String> = st.error();
    st.cancel();

    let _: WaveformData = wf.data(0);
    let _: f64 = wf.bucket_secs();
    let _: usize = wf.len();
}

fn _deck_surface(slot: &DeckSlot) {
    let _: bool = slot.is_loaded();
    let _: Option<&TrackInfo> = slot.info();
    let _: f64 = slot.duration_secs();
    let _: Option<LoudnessAnalysis> = slot.analysis();
    let _: DeckState = slot.state();
}

/// `EqBand` / `EqConfig` are constructed field-by-field by the shell from the
/// front end's JSON, so every field and helper is API.
fn _eq_types() {
    let band = EqBand {
        id: 7,
        enabled: true,
        kind: FilterKind::HighPass,
        freq_hz: 40.0,
        gain_db: 0.0,
        q: 0.707,
        slope_db_oct: 24,
    };
    let _: u32 = band.id;
    let _: bool = band.enabled;
    let _: FilterKind = band.kind;
    let _: f32 = band.freq_hz;
    let _: f32 = band.gain_db;
    let _: f32 = band.q;
    let _: u8 = band.slope_db_oct;

    let _: EqBand = EqBand::default();
    let _: EqBand = EqBand::bell(1, 1_000.0, 3.0, 1.0);
    let _: EqBand = EqBand::new(2, FilterKind::HighShelf, 8_000.0, -4.5, 0.7);
    let _: EqBand = band.sanitised(48_000.0);
    let _: usize = band.sections();
    let _: bool = FilterKind::HighPass.has_slope();
    let _: bool = FilterKind::Bell.has_gain();

    let cfg = EqConfig {
        enabled: true,
        bands: vec![band],
    };
    let _: bool = cfg.enabled;
    let _: Vec<EqBand> = cfg.bands.clone();
    let _: EqConfig = cfg.clone().sanitised(48_000.0);
    let _: bool = cfg.is_transparent();
}

/// The monitor matrix surface (SPEC §6).
fn _monitor_types() {
    let _: [MonitorMode; 7] = MonitorMode::ALL;
    let _: [f32; 4] = MonitorMode::Mono.matrix();
    let _: (f32, f32) = MonitorMode::Mono.fold(1.0, -1.0);
    let _: u8 = MonitorMode::Side.as_u8();
    let _: MonitorMode = MonitorMode::from_u8(3);
    let _: MonitorMode = MonitorMode::default();
}

/// These types cross the IPC boundary, so they must stay (de)serialisable and
/// must survive a round trip without changing shape.
#[test]
fn ipc_types_round_trip_through_serde() {
    fn round_trip<T>(v: &T)
    where
        T: serde::Serialize + serde::de::DeserializeOwned,
    {
        let json = serde_json::to_string(v).expect("serialise");
        let back: T = serde_json::from_str(&json).expect("deserialise");
        // No PartialEq on most of these (they hold f32s), so compare the wire
        // form instead - that is what the front end actually consumes.
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
    }

    round_trip(&Deck::A);
    round_trip(&Deck::B);
    round_trip(&EqConfig::default());
    round_trip(&EqConfig {
        enabled: true,
        bands: vec![
            EqBand::bell(1, 1_000.0, 3.0, 1.0),
            EqBand::new(2, FilterKind::HighShelf, 8_000.0, -4.5, 0.7),
        ],
    });
    for kind in [
        FilterKind::Bell,
        FilterKind::LowShelf,
        FilterKind::HighShelf,
        FilterKind::HighPass,
        FilterKind::LowPass,
        FilterKind::Notch,
        FilterKind::BandPass,
    ] {
        round_trip(&EqBand::new(3, kind, 500.0, 1.5, 2.0));
    }
    for mode in MonitorMode::ALL {
        round_trip(&mode);
    }
    round_trip(&TransportState::default());
    round_trip(&MeterSnapshot::default());
    round_trip(&LoudnessAnalysis::default());
    round_trip(&TrackInfo::default());
    round_trip(&DeviceInfo {
        name: "test".into(),
        is_default: true,
        sample_rates: vec![44_100, 48_000],
        host_id: "coreaudio".into(),
        default_sample_rate: Some(48_000),
        buffer_frames: Some(BufferRange::new(64, 2_048)),
        max_channels: 2,
    });
    // SPEC §16 wire shapes.
    round_trip(&HostInfo {
        id: "wasapi".into(),
        name: "WASAPI".into(),
        is_default: true,
        available: true,
        device_count: 3,
    });
    round_trip(&BufferRange::new(32, 8_192));
    round_trip(&EngineSource {
        host_id: "alsa".into(),
        device_name: Some("hw:0".into()),
        following_system_default: false,
        sample_rate: 96_000,
        buffer_frames: Some(256),
        latency_ms: Some(latency_ms(256, 96_000)),
        follow_source_rate: true,
    });
}

/// `WaveformData` is outbound-only (Serialize, no Deserialize), so pin its
/// camelCase key set instead of round-tripping it.
#[test]
fn waveform_payload_keys_are_camel_case() {
    let wf = Waveform::new(48_000, 48_000);
    let json = serde_json::to_value(wf.data(0)).unwrap();
    let obj = json.as_object().expect("object");
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["bucketSecs", "count", "expected", "max", "min", "rms"]
    );
}

/// `Deck` has to keep its two-value shape and its wire representation, because
/// the front end sends `"a"` / `"b"` strings.
#[test]
fn deck_wire_format_is_lowercase() {
    assert_eq!(serde_json::to_string(&Deck::A).unwrap(), "\"a\"");
    assert_eq!(serde_json::to_string(&Deck::B).unwrap(), "\"b\"");
    assert_eq!(serde_json::from_str::<Deck>("\"b\"").unwrap(), Deck::B);
    assert_eq!(Deck::default(), Deck::A);
    assert_eq!(Deck::A.index(), 0);
    assert_eq!(Deck::B.index(), 1);
    assert_eq!(Deck::A.other(), Deck::B);
    assert_eq!(Deck::B.other(), Deck::A);
    assert_eq!(Deck::from_index(0), Deck::A);
    assert_eq!(Deck::from_index(1), Deck::B);
}

/// SPEC §6: the seven modes, their exact wire strings, and the absence of a
/// redundant `Mid` (mid *is* `Mono`).
#[test]
fn monitor_mode_wire_format_and_variant_set() {
    let wire: Vec<String> = MonitorMode::ALL
        .iter()
        .map(|m| serde_json::to_string(m).unwrap())
        .collect();
    assert_eq!(
        wire,
        [
            "\"stereo\"",
            "\"mono\"",
            "\"left\"",
            "\"right\"",
            "\"swap\"",
            "\"side\"",
            "\"flipRight\"",
        ]
        .map(String::from)
    );
    assert_eq!(MonitorMode::default(), MonitorMode::Stereo);
    // `Mid` must not exist as a separate mode; `Mono` is it.
    assert!(serde_json::from_str::<MonitorMode>("\"mid\"").is_err());
    // An unknown index degrades to the transparent default rather than panicking.
    assert_eq!(MonitorMode::from_u8(200), MonitorMode::Stereo);
    for (i, m) in MonitorMode::ALL.iter().enumerate() {
        assert_eq!(m.as_u8() as usize, i);
        assert_eq!(MonitorMode::from_u8(i as u8), *m);
    }
}

/// SPEC §6: the folds themselves, expressed as matrices, and `Stereo` as a
/// literal identity.
#[test]
fn monitor_mode_matrices_match_the_spec() {
    assert_eq!(MonitorMode::Stereo.matrix(), MONITOR_IDENTITY);
    assert_eq!(MONITOR_IDENTITY, [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(MonitorMode::Mono.matrix(), [0.5, 0.5, 0.5, 0.5]);
    assert_eq!(MonitorMode::Left.matrix(), [1.0, 0.0, 1.0, 0.0]);
    assert_eq!(MonitorMode::Right.matrix(), [0.0, 1.0, 0.0, 1.0]);
    assert_eq!(MonitorMode::Swap.matrix(), [0.0, 1.0, 1.0, 0.0]);
    assert_eq!(MonitorMode::Side.matrix(), [0.5, -0.5, 0.5, -0.5]);
    assert_eq!(MonitorMode::FlipRight.matrix(), [1.0, 0.0, 0.0, -1.0]);

    // `fold` and `matrix` must never disagree - they describe one fold.
    for mode in MonitorMode::ALL {
        let m = mode.matrix();
        let (l, r) = (0.37f32, -0.11f32);
        assert_eq!(
            mode.fold(l, r),
            (m[0] * l + m[1] * r, m[2] * l + m[3] * r),
            "{mode:?}"
        );
    }
    // ... and `Stereo` returns the input untouched, bit for bit.
    for s in [0.0f32, -0.0, 1.0, -1.0, 1e-30, f32::MIN_POSITIVE] {
        let (l, r) = MonitorMode::Stereo.fold(s, -s);
        assert_eq!(l.to_bits(), s.to_bits());
        assert_eq!(r.to_bits(), (-s).to_bits());
    }
}

/// SPEC §12: the EQ is a dynamic 0..=16 band list whose resting state is
/// *no bands at all*, and whose wire format is camelCase.
#[test]
fn eq_config_is_a_dynamic_band_list() {
    assert_eq!(MAX_BANDS, 16);
    assert_eq!(MAX_EQ_SECTIONS, 4, "48 dB/oct = four cascaded biquads");
    let def = EqConfig::default();
    assert!(def.bands.is_empty(), "zero bands is the resting state");
    assert!(!def.enabled, "EQ is off by default");
    assert!(def.is_transparent());

    // Disabled, or enabled with every band bypassed, is transparent.
    assert!(EqConfig {
        enabled: false,
        bands: vec![EqBand::bell(1, 1_000.0, 6.0, 1.0)],
    }
    .is_transparent());
    assert!(EqConfig {
        enabled: true,
        bands: vec![EqBand {
            enabled: false,
            ..EqBand::bell(1, 1_000.0, 6.0, 1.0)
        }],
    }
    .is_transparent());
    assert!(!EqConfig {
        enabled: true,
        bands: vec![EqBand::bell(1, 1_000.0, 6.0, 1.0)],
    }
    .is_transparent());

    // More than MAX_BANDS is truncated, not rejected and not honoured.
    let too_many = EqConfig {
        enabled: true,
        bands: (0..MAX_BANDS as u32 + 5)
            .map(|i| EqBand::bell(i, 1_000.0, 1.0, 1.0))
            .collect(),
    }
    .sanitised(48_000.0);
    assert_eq!(too_many.bands.len(), MAX_BANDS);
}

/// The band payload the front end sends, key for key.
#[test]
fn eq_band_wire_keys_are_camel_case() {
    let json = serde_json::to_value(EqBand::default()).unwrap();
    let obj = json.as_object().expect("object");
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "enabled",
            "freqHz",
            "gainDb",
            "id",
            "kind",
            "q",
            "slopeDbOct"
        ]
    );

    let kinds: Vec<String> = [
        FilterKind::Bell,
        FilterKind::LowShelf,
        FilterKind::HighShelf,
        FilterKind::HighPass,
        FilterKind::LowPass,
        FilterKind::Notch,
        FilterKind::BandPass,
    ]
    .iter()
    .map(|k| serde_json::to_string(k).unwrap())
    .collect();
    assert_eq!(
        kinds,
        [
            "\"bell\"",
            "\"lowShelf\"",
            "\"highShelf\"",
            "\"highPass\"",
            "\"lowPass\"",
            "\"notch\"",
            "\"bandPass\"",
        ]
        .map(String::from)
    );
    assert_eq!(FilterKind::default(), FilterKind::Bell);
    // The v1 name is gone: a stale front end must fail loudly, not silently
    // deserialise into a bell.
    assert!(serde_json::from_str::<FilterKind>("\"peaking\"").is_err());

    // `id` and `slopeDbOct` have serde defaults so a config written by an
    // older build still loads.
    let band: EqBand =
        serde_json::from_str(r#"{"enabled":true,"kind":"bell","freqHz":100,"gainDb":0,"q":1}"#)
            .expect("legacy band without id/slope");
    assert_eq!(band.id, 0);
    assert_eq!(band.slope_db_oct, 12);
}

/// Slope handling: only HP/LP have one, it snaps to 12/24/48, and it maps to
/// the number of cascaded sections the realtime side pre-allocates for.
#[test]
fn filter_kind_capabilities_and_slope_sections() {
    for kind in [FilterKind::HighPass, FilterKind::LowPass] {
        assert!(kind.has_slope(), "{kind:?}");
        assert!(!kind.has_gain(), "{kind:?}");
    }
    for kind in [
        FilterKind::Bell,
        FilterKind::LowShelf,
        FilterKind::HighShelf,
    ] {
        assert!(!kind.has_slope(), "{kind:?}");
        assert!(kind.has_gain(), "{kind:?}");
    }
    for kind in [FilterKind::Notch, FilterKind::BandPass] {
        assert!(!kind.has_slope(), "{kind:?}");
        assert!(!kind.has_gain(), "{kind:?}");
    }

    for (asked, want_slope, want_sections) in [
        (0u8, 12u8, 1usize),
        (12, 12, 1),
        (24, 24, 2),
        (48, 48, 4),
        (255, 48, 4),
    ] {
        let band = EqBand {
            kind: FilterKind::HighPass,
            slope_db_oct: asked,
            ..EqBand::default()
        }
        .sanitised(48_000.0);
        assert_eq!(band.slope_db_oct, want_slope, "asked {asked}");
        assert_eq!(band.sections(), want_sections, "asked {asked}");
        assert!(band.sections() <= MAX_EQ_SECTIONS);
    }
    // Non-slope shapes are always a single section whatever the field says.
    assert_eq!(
        EqBand {
            kind: FilterKind::Bell,
            slope_db_oct: 48,
            ..EqBand::default()
        }
        .sections(),
        1
    );
}

/// A hostile or wildly-dragged payload must be clamped into the documented
/// ranges, and NaN must not survive into a filter design.
#[test]
fn eq_band_sanitisation_clamps_and_defuses_nan() {
    let wild = EqBand {
        id: 4,
        enabled: true,
        kind: FilterKind::Bell,
        freq_hz: 1.0e9,
        gain_db: 400.0,
        q: 1.0e6,
        slope_db_oct: 12,
    }
    .sanitised(48_000.0);
    assert!(wild.freq_hz <= MAX_EQ_FREQ && wild.freq_hz >= MIN_EQ_FREQ);
    assert_eq!(wild.gain_db, MAX_EQ_GAIN_DB);
    assert_eq!(wild.q, MAX_EQ_Q);

    let low = EqBand {
        freq_hz: 0.0,
        gain_db: -400.0,
        q: 0.0,
        ..EqBand::default()
    }
    .sanitised(48_000.0);
    assert_eq!(low.freq_hz, MIN_EQ_FREQ);
    assert_eq!(low.gain_db, -MAX_EQ_GAIN_DB);
    assert_eq!(low.q, MIN_EQ_Q);

    let nan = EqBand {
        freq_hz: f32::NAN,
        gain_db: f32::INFINITY,
        q: f32::NEG_INFINITY,
        ..EqBand::default()
    }
    .sanitised(48_000.0);
    assert!(nan.freq_hz.is_finite() && nan.gain_db.is_finite() && nan.q.is_finite());

    // Frequency is bounded below Nyquist, so an 8 kHz session cannot ask for a
    // 20 kHz shelf that would design an unstable filter.
    let narrow = EqBand::bell(0, 20_000.0, 6.0, 1.0).sanitised(8_000.0);
    assert!(
        narrow.freq_hz < 8_000.0 * 0.5,
        "{} should be under nyquist",
        narrow.freq_hz
    );
    assert!(narrow.freq_hz >= MIN_EQ_FREQ);
}

/// `is_supported_path` must agree with the advertised extension list, in both
/// cases, because the drag-and-drop filter in the shell uses the list and the
/// open path uses the function.
#[test]
fn supported_extensions_and_the_predicate_agree() {
    assert!(!SUPPORTED_EXTENSIONS.is_empty());
    for ext in SUPPORTED_EXTENSIONS {
        let lower = std::path::PathBuf::from(format!("/tmp/track.{ext}"));
        let upper = std::path::PathBuf::from(format!("/tmp/track.{}", ext.to_uppercase()));
        assert!(is_supported_path(&lower), "{ext} should be supported");
        assert!(
            is_supported_path(&upper),
            "{ext} should be supported case-insensitively"
        );
    }
    assert!(!is_supported_path(Path::new("/tmp/notes.txt")));
    assert!(!is_supported_path(Path::new("/tmp/no-extension")));
}

/// dB <-> linear must be exact inverses in the useful range, and must floor
/// rather than return -inf / 0 divergently.
#[test]
fn db_helpers_are_inverses_and_floor_cleanly() {
    for db in [-96.0f32, -24.0, -6.0, 0.0, 6.0, 24.0] {
        let back = lin_to_db(db_to_lin(db));
        assert!((back - db).abs() < 1e-3, "{db} dB round-tripped to {back}");
    }
    assert_eq!(db_to_lin(0.0), 1.0);
    assert!((db_to_lin(-6.0206) - 0.5).abs() < 1e-4);
    assert_eq!(db_to_lin(MIN_DB), 0.0);
    assert_eq!(lin_to_db(0.0), MIN_DB);

    // A NaN must never reach a gain stage or a meter read-out (SPEC §9).
    assert_eq!(db_to_lin(f32::NAN), 0.0);
    assert_eq!(db_to_lin(f32::NEG_INFINITY), 0.0);
    assert_eq!(lin_to_db(f32::NAN), MIN_DB);
    assert_eq!(lin_to_db(f32::INFINITY), MIN_DB);
}

/// SPEC §11: the confidence gate is part of the contract, not an internal
/// detail - the shell reports `applied: false` off the back of it.
#[test]
fn align_confidence_gate_is_pinned() {
    assert_eq!(MIN_CONFIDENCE, 0.3);
    let good = AlignEstimate {
        offset_frames: 480,
        confidence: 0.9,
        polarity_inverted: false,
    };
    let bad = AlignEstimate {
        confidence: MIN_CONFIDENCE - 0.01,
        ..good
    };
    assert!(good.is_confident());
    assert!(!bad.is_confident());
    assert!(AlignEstimate {
        confidence: MIN_CONFIDENCE,
        ..good
    }
    .is_confident());
}

/// SPEC §11: too-short material is an `Err` with a message, never a bogus
/// offset and never a panic.
#[test]
fn align_refuses_short_material_without_panicking() {
    for (a, b) in [
        (vec![], vec![]),
        (vec![0.0f32; 1], vec![0.0f32; 1]),
        (vec![0.1f32; 48_000 * 6], vec![0.0f32; 1]),
    ] {
        let err = estimate_offset(&a, &b, 48_000).expect_err("short input must be an error");
        let msg = err.to_string();
        assert!(msg.contains("align"), "unhelpful message: {msg}");
    }
    // A zero sample rate is nonsense, not a division by zero.
    assert!(estimate_offset(&[0.0; 16], &[0.0; 16], 0).is_err());
}

/// `TransportState` carries the monitor mode to the UI (SPEC §6).
#[test]
fn transport_state_exposes_the_monitor_mode() {
    let mut t = TransportState::default();
    assert_eq!(t.monitor_mode, MonitorMode::Stereo);
    t.monitor_mode = MonitorMode::Side;
    let json = serde_json::to_value(&t).unwrap();
    assert_eq!(
        json.get("monitorMode").and_then(|v| v.as_str()),
        Some("side")
    );
}
