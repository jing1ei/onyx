/**
 * EQ maths and mapping helpers — SPEC.md §12.
 *
 * The composite curve is computed here from the *actual* biquad coefficients
 * (RBJ audio-EQ cookbook, the same formulas the engine uses), evaluated on the
 * unit circle. It is deliberately not a parametric sketch: a curve that
 * disagrees with what you hear is worse than no curve. Doing it locally also
 * keeps dragging a node at pointer rate free of an IPC round trip.
 *
 * That makes this file one half of a two-language contract with
 * `onyx-core::dsp::eq::curve_db`, and the two have drifted before. It is now
 * checked, not asserted: `crates/onyx-core/tests/eq_curve_contract.rs` pins the
 * engine's response for 462 filter configurations and
 * `scripts/check-eq-curve.mjs` (`npm run check:eq`, and part of `npm run
 * build`) runs *this* module against that fixture. Anything below 1e-3 dB
 * passes; the two currently agree to 5.6e-5 dB, which is the resolution of the
 * `Float32Array` the curve is accumulated into.
 */

import {
  GAINLESS_KINDS,
  SPECTRUM_BANDS,
  SPECTRUM_F_MAX,
  SPECTRUM_F_MIN,
  type EqBand,
  type EqConfig,
  type FilterKind,
} from "./types";

const F_MIN = 20;
const F_MAX = 20_000;
/** Gain axis, ±dB. Bands beyond this stay draggable; the axis does not rescale. */
export const GAIN_RANGE = 18;

export const Q_MIN = 0.1;
export const Q_MAX = 40;
export const GAIN_MIN = -30;
export const GAIN_MAX = 30;

/**
 * The floor the engine's `Coeffs::design` clamps Q to, which is *not* `Q_MIN`.
 *
 * `Q_MIN` is the band model's range (SPEC §12) and is what a knob may be
 * dragged to; `DESIGN_Q_MIN` is what a single biquad section is allowed to be
 * designed at. They differ because a cascade scales the first section's Q below
 * the band's own (see `bandStages`), and clamping that scaled value at 0.1 —
 * which this file used to do — drew a 48 dB/oct high-pass at Q = 0.1 with
 * 2.84 dB more level around the corner than the audio path produced.
 * `onyx-core::dsp::biquad::Coeffs::design` clamps at 0.05, so this does too.
 */
const DESIGN_Q_MIN = 0.05;
/** Rust's `f64::MIN_POSITIVE` — the smallest normal double. */
const MIN_POSITIVE_F64 = 2.2250738585072014e-308;

const LOG_MIN = Math.log10(F_MIN);
const LOG_SPAN = Math.log10(F_MAX) - LOG_MIN;

export const clamp = (v: number, lo: number, hi: number): number =>
  v < lo ? lo : v > hi ? hi : v;

/** frequency → 0..1 across the log axis */
export const freqToPos = (hz: number): number =>
  (Math.log10(clamp(hz, F_MIN, F_MAX)) - LOG_MIN) / LOG_SPAN;

/** 0..1 across the log axis → frequency */
export const posToFreq = (pos: number): number => Math.pow(10, LOG_MIN + clamp(pos, 0, 1) * LOG_SPAN);

/** Hold a frequency inside the drawn axis. The engine clamps too; this keeps
 *  the node under the cursor instead of letting it walk off the plot. */
export const clampFreq = (hz: number): number => clamp(hz, F_MIN, F_MAX);

/**
 * Where each of the engine's log-spaced analyser bands sits on that axis, 0..1.
 * The EQ backdrop draws the engine's 96 analyser bands on this axis, and used
 * to recompute the mapping per band, per frame. One table, built once.
 */
export const SPECTRUM_BAND_POS: Float32Array = (() => {
  const pos = new Float32Array(SPECTRUM_BANDS);
  const ratio = SPECTRUM_F_MAX / SPECTRUM_F_MIN;
  for (let i = 0; i < SPECTRUM_BANDS; i += 1) {
    pos[i] = freqToPos(SPECTRUM_F_MIN * Math.pow(ratio, i / (SPECTRUM_BANDS - 1)));
  }
  return pos;
})();

/* ── biquad coefficients ─────────────────────────────────────────────────── */

interface Biquad {
  b0: number;
  b1: number;
  b2: number;
  a1: number;
  a2: number;
}

/**
 * Butterworth stage Qs for a cascade of `n` biquads (order 2n).
 * 12 dB/oct = 1 stage, 24 = 2, 48 = 4.
 */
function butterworthQs(stages: number): number[] {
  const qs: number[] = [];
  for (let k = 0; k < stages; k += 1) {
    qs.push(1 / (2 * Math.cos(((2 * k + 1) * Math.PI) / (4 * stages))));
  }
  return qs;
}

function rbj(kind: FilterKind, freqHz: number, gainDb: number, q: number, fs: number): Biquad {
  const f0 = clamp(freqHz, 1, fs * 0.49);
  const w0 = (2 * Math.PI * f0) / fs;
  const cos = Math.cos(w0);
  const sin = Math.sin(w0);
  const qq = clamp(q, DESIGN_Q_MIN, Q_MAX);
  const alpha = sin / (2 * qq);
  const A = Math.pow(10, gainDb / 40);
  const sqA = Math.sqrt(A);

  let b0: number;
  let b1: number;
  let b2: number;
  let a0: number;
  let a1: number;
  let a2: number;

  switch (kind) {
    case "bell":
      b0 = 1 + alpha * A;
      b1 = -2 * cos;
      b2 = 1 - alpha * A;
      a0 = 1 + alpha / A;
      a1 = -2 * cos;
      a2 = 1 - alpha / A;
      break;
    case "lowShelf":
      b0 = A * (A + 1 - (A - 1) * cos + 2 * sqA * alpha);
      b1 = 2 * A * (A - 1 - (A + 1) * cos);
      b2 = A * (A + 1 - (A - 1) * cos - 2 * sqA * alpha);
      a0 = A + 1 + (A - 1) * cos + 2 * sqA * alpha;
      a1 = -2 * (A - 1 + (A + 1) * cos);
      a2 = A + 1 + (A - 1) * cos - 2 * sqA * alpha;
      break;
    case "highShelf":
      b0 = A * (A + 1 + (A - 1) * cos + 2 * sqA * alpha);
      b1 = -2 * A * (A - 1 + (A + 1) * cos);
      b2 = A * (A + 1 + (A - 1) * cos - 2 * sqA * alpha);
      a0 = A + 1 - (A - 1) * cos + 2 * sqA * alpha;
      a1 = 2 * (A - 1 - (A + 1) * cos);
      a2 = A + 1 - (A - 1) * cos - 2 * sqA * alpha;
      break;
    case "highPass":
      b0 = (1 + cos) / 2;
      b1 = -(1 + cos);
      b2 = (1 + cos) / 2;
      a0 = 1 + alpha;
      a1 = -2 * cos;
      a2 = 1 - alpha;
      break;
    case "lowPass":
      b0 = (1 - cos) / 2;
      b1 = 1 - cos;
      b2 = (1 - cos) / 2;
      a0 = 1 + alpha;
      a1 = -2 * cos;
      a2 = 1 - alpha;
      break;
    case "notch":
      b0 = 1;
      b1 = -2 * cos;
      b2 = 1;
      a0 = 1 + alpha;
      a1 = -2 * cos;
      a2 = 1 - alpha;
      break;
    case "bandPass":
      // constant 0 dB peak gain
      b0 = alpha;
      b1 = 0;
      b2 = -alpha;
      a0 = 1 + alpha;
      a1 = -2 * cos;
      a2 = 1 - alpha;
      break;
  }

  return { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0 };
}

/**
 * The biquad cascade a band expands to. HP/LP become `slope / 12` Butterworth
 * stages; the band's own Q scales the first stage so raising Q adds the
 * expected corner resonance instead of being silently ignored.
 *
 * This is `Coeffs::design_cascade_section` in the engine, and the band's own Q
 * is clamped to the *band model's* range (`Q_MIN`, what `EqBand::sanitised`
 * enforces before the engine ever designs anything), while the scaled
 * per-section Q is clamped to `DESIGN_Q_MIN` inside `rbj`, exactly as
 * `Coeffs::design` does.
 */
function bandStages(band: EqBand, fs: number): Biquad[] {
  if (band.kind === "highPass" || band.kind === "lowPass") {
    const stages = Math.max(1, Math.round((band.slopeDbOct || 12) / 12));
    const qs = butterworthQs(stages);
    const resonance = clamp(band.q, Q_MIN, Q_MAX) / Math.SQRT1_2;
    return qs.map((q, i) => rbj(band.kind, band.freqHz, 0, i === 0 ? q * resonance : q, fs));
  }
  const gain = GAINLESS_KINDS.has(band.kind) ? 0 : band.gainDb;
  return [rbj(band.kind, band.freqHz, gain, band.q, fs)];
}

/**
 * |H(f)| in dB for one biquad — the same closed form, with the same guards, as
 * `onyx-core::dsp::biquad::Coeffs::magnitude_db`. The floor matters: this used
 * to clamp at −120 dB *per section*, so a 48 dB/oct pair in its stopband drew
 * −480 dB where the engine reports −560 dB. Nobody can see either number, but
 * two implementations of one contract have to be one contract.
 */
function biquadDb(bq: Biquad, w: number): number {
  const cw = Math.cos(w);
  const sw = Math.sin(w);
  const c2w = Math.cos(2 * w);
  const s2w = Math.sin(2 * w);
  const nr = bq.b0 + bq.b1 * cw + bq.b2 * c2w;
  const ni = -(bq.b1 * sw + bq.b2 * s2w);
  const dr = 1 + bq.a1 * cw + bq.a2 * c2w;
  const di = -(bq.a1 * sw + bq.a2 * s2w);
  const num = Math.sqrt(nr * nr + ni * ni);
  const den = Math.sqrt(dr * dr + di * di);
  // `MIN_POSITIVE_F64` is Rust's `f64::MIN_POSITIVE`, the smallest normal.
  if (den <= MIN_POSITIVE_F64 || num <= 1e-12) return -120;
  return 20 * Math.log10(num / den);
}

/** Magnitude response of one band, in dB, over the given frequencies. */
export function bandResponse(band: EqBand, freqs: Float64Array, fs: number, out: Float32Array): void {
  const stages = bandStages(band, fs);
  for (let i = 0; i < freqs.length; i += 1) {
    const w = (2 * Math.PI * freqs[i]) / fs;
    let db = 0;
    for (const s of stages) db += biquadDb(s, w);
    out[i] = db;
  }
}

/** Composite (sum of enabled bands) magnitude response, in dB. */
export function compositeResponse(
  config: EqConfig,
  freqs: Float64Array,
  fs: number,
  out: Float32Array,
): void {
  out.fill(0);
  if (!config.enabled) return;
  const scratch = new Float32Array(freqs.length);
  for (const band of config.bands) {
    if (!band.enabled) continue;
    bandResponse(band, freqs, fs, scratch);
    for (let i = 0; i < out.length; i += 1) out[i] += scratch[i];
  }
}

/* ── bands ───────────────────────────────────────────────────────────────── */

let bandSeq = 1;

/** Ids only have to be unique within the config the front end owns. */
function nextBandId(config: EqConfig | null): number {
  const highest = config ? config.bands.reduce((m, b) => Math.max(m, b.id), 0) : 0;
  bandSeq = Math.max(bandSeq, highest + 1);
  return bandSeq++;
}

export function makeBand(
  config: EqConfig | null,
  kind: FilterKind,
  freqHz: number,
  gainDb: number,
  q = 1,
): EqBand {
  return {
    id: nextBandId(config),
    enabled: true,
    kind,
    freqHz: Math.round(clamp(freqHz, F_MIN, F_MAX)),
    gainDb: GAINLESS_KINDS.has(kind) ? 0 : clamp(gainDb, GAIN_MIN, GAIN_MAX),
    q: clamp(q, Q_MIN, Q_MAX),
    slopeDbOct: 12,
  };
}

/* ── note names — this is a music tool ───────────────────────────────────── */

const NOTE_NAMES = ["C", "C\u266F", "D", "D\u266F", "E", "F", "F\u266F", "G", "G\u266F", "A", "A\u266F", "B"];

/** `A4` for 440 Hz. Null below ~16 Hz / above ~13 kHz where it stops meaning much. */
function noteName(hz: number): string | null {
  if (!Number.isFinite(hz) || hz < 16 || hz > 13000) return null;
  const midi = Math.round(69 + 12 * Math.log2(hz / 440));
  const name = NOTE_NAMES[((midi % 12) + 12) % 12];
  const octave = Math.floor(midi / 12) - 1;
  return `${name}${octave}`;
}

/** `440 Hz · A4` — the read-out used on nodes and the hover label. */
export function formatFreqWithNote(hz: number): string {
  const base = hz >= 1000 ? `${(hz / 1000).toFixed(hz >= 10000 ? 1 : 2)} kHz` : `${Math.round(hz)} Hz`;
  const note = noteName(hz);
  return note ? `${base} \u00B7 ${note}` : base;
}

/* ── band-solo audition sweep ────────────────────────────────────────────── */

const SOLO_Q_MIN = 2;
const SOLO_Q_MAX = 24;

/** Y position 0 (top) .. 1 (bottom) → Q. Higher up the graph = narrower. */
export function soloQForY(norm: number): number {
  const t = clamp(1 - norm, 0, 1);
  return SOLO_Q_MIN * Math.pow(SOLO_Q_MAX / SOLO_Q_MIN, t);
}
