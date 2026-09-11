import { t } from "../lib/i18n";
/**
 * Interactive spectrum EQ — SPEC.md §12.
 *
 * Zero to sixteen bands, direct manipulation, and a Cmd/Ctrl band-solo sweep.
 *
 * # It lives in its own window
 *
 * This component is the whole content of `eq.html`, a second `WebviewWindow`
 * created by Rust (`src-tauri/src/eqwindow.rs`, `src/lib/eqwindow.ts`) — drag
 * it to a second monitor, work the curve, close it without touching playback.
 * Two consequences run through the code below:
 *
 *  - **the engine is the only authority.** This window is the sole editor of
 *    the band list (SPEC §12: the front end owns it and sends the whole config
 *    through `set_eq`), and everything else — the main window's badges, its EQ
 *    button — reads the engine's answer rather than a copy. The one value the
 *    other window can also change is `enabled` (Shift+E), so that flag is
 *    adopted even mid-drag; band geometry is not, or a snapshot landing during
 *    a gesture would fight the pointer;
 *  - **teardown is not guaranteed.** A closing webview does not reliably run
 *    `useEffect` cleanups, so the analyser and the audition bandpass are also
 *    switched off from Rust when the window is destroyed. What this side still
 *    owes is the *last* `set_eq`: it is flushed the moment a gesture ends
 *    rather than on the next animation frame, so closing the window in the same
 *    breath as letting go of a node cannot lose the move.
 *
 * Performance shape, deliberately:
 *  - the band list lives in a mutable ref, not in React state, so dragging a
 *    node never re-renders the tree; the canvas repaints from the shared 60 Hz
 *    rAF loop and the numeric read-outs are written straight into the DOM;
 *  - React state only changes when the *structure* changes (a band is added,
 *    removed, retyped or bypassed), i.e. at human rate;
 *  - `set_eq` writes are coalesced to one per animation frame, and always carry
 *    the complete config — there are no per-band commands.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import * as api from "../lib/api";
import { auditionRef, setAudition, stopAudition } from "../lib/audition";
import { beginPaint, useSurface } from "../lib/canvas";
import { setAttr, setText } from "../lib/dom";
import { closeEqWindow, setEqWindowPinned } from "../lib/eqwindow";
import { useFrameEffect } from "../lib/frame";
import { acquireSpectrum } from "../lib/spectrum";
import { formatSignedDb } from "../lib/format";
import {
  bandResponse,
  clamp,
  clampFreq,
  compositeResponse,
  formatFreqWithNote,
  freqToPos,
  GAIN_MAX,
  GAIN_MIN,
  GAIN_RANGE,
  makeBand,
  posToFreq,
  Q_MAX,
  Q_MIN,
  soloQForY,
  SPECTRUM_BAND_POS,
} from "../lib/eq";
import { useStore } from "../lib/store";
import { paint } from "../lib/theme";
import {
  FILTER_KINDS,
  FILTER_LABEL,
  FILTER_SHORT,
  GAINLESS_KINDS,
  MAX_EQ_BANDS,
  SLOPE_CHOICES,
  SPECTRUM_BANDS,
  type EqBand,
  type EqConfig,
  type FilterKind,
} from "../lib/types";
import { IconClose } from "./Icons";

const CURVE_POINTS = 320;
const GRID_F = [20, 30, 50, 100, 200, 300, 500, 1000, 2000, 3000, 5000, 10000, 20000];
const LABEL_F = [20, 50, 100, 500, 1000, 5000, 10000, 20000];
const GRID_DB = [-18, -12, -6, 0, 6, 12, 18];
const PAD_TOP = 8;
const PAD_BOTTOM = 15;
const HIT_RADIUS = 13;
const CLICK_SLOP = 4;

const CURVE_FREQS = new Float64Array(CURVE_POINTS);
for (let i = 0; i < CURVE_POINTS; i += 1) CURVE_FREQS[i] = posToFreq(i / (CURVE_POINTS - 1));

const DB_FLOOR = -84;
const DB_CEIL = -6;

type GestureKind = "node" | "sweep" | "empty";

interface Gesture {
  kind: GestureKind;
  pointerId: number;
  bandId: number;
  startX: number;
  startY: number;
  startFreq: number;
  startGain: number;
  altAtStart: boolean;
  moved: boolean;
}

interface Menu {
  x: number;
  y: number;
  bandId: number;
}

/* ── one coalesced writer for `set_eq` ───────────────────────────────────── */

function useEqSender(onReject: () => void): {
  send: (cfg: EqConfig) => void;
  /** write now, not on the next frame — see the note about teardown above */
  flushNow: () => void;
} {
  const pending = useRef<EqConfig | null>(null);
  const scheduled = useRef(0);
  const pushToast = useStore((s) => s.pushToast);

  const flush = useCallback(() => {
    scheduled.current = 0;
    const cfg = pending.current;
    pending.current = null;
    if (!cfg) return;
    void api.setEq(cfg).catch((err) => {
      // the engine refused: say so and let the next snapshot overwrite the
      // optimistic curve rather than leaving the display lying
      pushToast("error", `EQ rejected: ${api.errorMessage(err)}`);
      onReject();
    });
  }, [onReject, pushToast]);

  useEffect(() => {
    return () => {
      if (scheduled.current) cancelAnimationFrame(scheduled.current);
      scheduled.current = 0;
      // Closing the panel in the same frame as the last drag movement used to
      // drop that write on the floor: the node stopped where the user let go,
      // the engine stayed a frame behind, and reopening the panel adopted the
      // engine's older curve. Send it instead of cancelling it.
      flush();
    };
  }, [flush]);

  const send = useCallback(
    (cfg: EqConfig) => {
      pending.current = cfg;
      if (!scheduled.current) scheduled.current = requestAnimationFrame(flush);
    },
    [flush],
  );

  const flushNow = useCallback(() => {
    if (scheduled.current) cancelAnimationFrame(scheduled.current);
    scheduled.current = 0;
    flush();
  }, [flush]);

  return { send, flushNow };
}

export default function EqPanel() {
  const snapshot = useStore((s) => s.snapshot);
  const pinned = useStore((s) => s.eqWindow.pinned);
  const pushToast = useStore((s) => s.pushToast);
  const blindActive = useStore((s) => s.snapshot?.blind.active ?? false);

  const wrapRef = useRef<HTMLDivElement | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const rowsRef = useRef<HTMLDivElement | null>(null);
  const readoutRef = useRef<HTMLSpanElement | null>(null);

  /** live, mutable config; the painter and the pointer handlers own this */
  const cfgRef = useRef<EqConfig>(snapshot?.eq ?? { enabled: true, bands: [] });
  /** structural mirror for React — updated at human rate only */
  const [cfg, setCfgState] = useState<EqConfig>(cfgRef.current);
  const [menu, setMenu] = useState<Menu | null>(null);
  const gesture = useRef<Gesture | null>(null);
  const hoverBand = useRef<number | null>(null);
  const cursor = useRef<{ x: number; y: number } | null>(null);
  const forceAdopt = useRef(false);
  const version = useRef(0);

  const surface = useSurface(canvasRef, { measure: wrapRef });

  const { send: sendEq, flushNow } = useEqSender(
    useCallback(() => {
      forceAdopt.current = true;
    }, []),
  );

  /** structural change → React state; continuous change → ref + canvas only */
  const commit = useCallback(
    (next: EqConfig, structural: boolean) => {
      cfgRef.current = next;
      version.current += 1;
      if (structural) setCfgState(next);
      sendEq(next);
    },
    [sendEq],
  );

  const patchBand = useCallback(
    (id: number, patch: Partial<EqBand>, structural: boolean) => {
      const cur = cfgRef.current;
      const bands = cur.bands.map((b) => (b.id === id ? { ...b, ...patch } : b));
      commit({ ...cur, bands }, structural);
    },
    [commit],
  );

  /* adopt engine state when we are not mid-gesture (or after a rejection) */
  useEffect(() => {
    const eq = snapshot?.eq;
    if (!eq) return;
    if (gesture.current && !forceAdopt.current) {
      // One exception, and it exists because there are two windows: Shift+E in
      // the main window bypasses the EQ, and dropping that here would mean the
      // next write from this drag quietly re-engaged it. Take the flag, leave
      // the bands the pointer is holding alone.
      if (eq.enabled !== cfgRef.current.enabled) {
        cfgRef.current = { ...cfgRef.current, enabled: eq.enabled };
        version.current += 1;
        setCfgState(cfgRef.current);
      }
      return;
    }
    const same = JSON.stringify(eq) === JSON.stringify(cfgRef.current);
    if (same && !forceAdopt.current) return;
    forceAdopt.current = false;
    cfgRef.current = eq;
    version.current += 1;
    setCfgState(eq);
  }, [snapshot?.eq]);

  /* This backdrop is the *only* view of the analyser in the whole app, and it
     now lives in a window of its own, so the FFT runs exactly while that window
     is open — SPEC §12. Rust switches it off again if the window is destroyed
     without this cleanup running. And not during a blind test: a live spectrum
     of the audible slot is the same leak as a live meter (SPEC §7), so it is
     masked here and the analyser is released for the duration of the test. */
  useEffect(() => {
    if (blindActive) return;
    return acquireSpectrum();
  }, [blindActive]);

  /* a sweep must never outlive the panel that started it */
  useEffect(() => stopAudition, []);

  /* ── painting ──────────────────────────────────────────────────────────── */

  const smooth = useRef(new Float32Array(SPECTRUM_BANDS).fill(DB_FLOOR));
  const peaks = useRef(new Float32Array(SPECTRUM_BANDS).fill(DB_FLOOR));
  const composite = useRef(new Float32Array(CURVE_POINTS));
  const perBand = useRef(new Map<number, Float32Array>());
  const curveVersion = useRef(-1);
  const curveRate = useRef(0);

  useFrameEffect((frame, _time, dt) => {
    const { w, h } = surface.current;
    const ctx = beginPaint(canvasRef.current, surface.current);
    if (!ctx) return;
    /* The EQ lives in its own webview (SPEC §12) and re-themes with the main
       window, so the palette is resolved per frame here too — `paint()` is a
       cache lookup until the theme actually changes. */
    const p = paint();

    const plotTop = PAD_TOP;
    const plotBottom = h - PAD_BOTTOM;
    const plotH = Math.max(1, plotBottom - plotTop);
    const midY = plotTop + plotH / 2;
    const gainToY = (db: number): number => midY - (db / GAIN_RANGE) * (plotH / 2);
    const freqToX = (f: number): number => freqToPos(f) * w;

    const config = cfgRef.current;
    const rate = frame?.transport.engineSampleRate || 48000;

    /* recompute the response only when something actually changed */
    if (curveVersion.current !== version.current || curveRate.current !== rate) {
      curveVersion.current = version.current;
      curveRate.current = rate;
      compositeResponse(config, CURVE_FREQS, rate, composite.current);
      const map = perBand.current;
      map.clear();
      for (const band of config.bands) {
        const buf = new Float32Array(CURVE_POINTS);
        bandResponse(band, CURVE_FREQS, rate, buf);
        map.set(band.id, buf);
      }
    }

    /* ── grid ── */
    ctx.font = p.font(9);
    ctx.textBaseline = "alphabetic";
    for (const f of GRID_F) {
      const x = Math.round(freqToX(f)) + 0.5;
      const labelled = LABEL_F.includes(f);
      ctx.strokeStyle = p.color(labelled ? "--eq-grid-major" : "--eq-grid-minor");
      ctx.beginPath();
      ctx.moveTo(x, plotTop);
      ctx.lineTo(x, plotBottom);
      ctx.stroke();
      if (labelled) {
        ctx.fillStyle = p.color("--eq-freq-label");
        ctx.textAlign = f === 20 ? "left" : f === 20000 ? "right" : "center";
        const label = f >= 1000 ? `${f / 1000}k` : `${f}`;
        ctx.fillText(label, f === 20 ? 3 : f === 20000 ? w - 3 : x, h - 4);
      }
    }
    for (const db of GRID_DB) {
      const y = Math.round(gainToY(db)) + 0.5;
      ctx.strokeStyle = p.color(db === 0 ? "--eq-grid-zero" : "--eq-grid-minor");
      ctx.beginPath();
      ctx.moveTo(0, y);
      ctx.lineTo(w, y);
      ctx.stroke();
      if (db !== 0) {
        ctx.fillStyle = p.color("--eq-db-label");
        ctx.textAlign = "left";
        ctx.fillText(`${db > 0 ? "+" : "\u2212"}${Math.abs(db)}`, 4, y - 3);
      }
    }

    /* ── analyser backdrop ── */
    const spec = frame?.meters.spectrum;
    const sm = smooth.current;
    const pk = peaks.current;
    const fall = Math.min(1, dt / 900);
    const specY = (db: number): number =>
      plotBottom - ((clamp(db, DB_FLOOR, DB_CEIL) - DB_FLOOR) / (DB_CEIL - DB_FLOOR)) * plotH;

    let anyEnergy = false;
    // `blindActive` straight off this render: `useFrameEffect` re-points the
    // subscription at every render's painter, so the frame loop reads the live
    // value without the hand-written `blindRef` mirror this used to carry.
    if (blindActive) {
      // flattened, not merely un-drawn: no residue to decay back from when
      // the test ends, and nothing left in the buffers to read a slot off
      sm.fill(DB_FLOOR);
      pk.fill(DB_FLOOR);
    } else {
      for (let i = 0; i < SPECTRUM_BANDS; i += 1) {
        const v = spec && i < spec.length ? spec[i] : DB_FLOOR;
        const target = clamp(v, DB_FLOOR, DB_CEIL);
        sm[i] = target > sm[i] ? target : sm[i] + (target - sm[i]) * Math.min(1, dt / 110);
        pk[i] = target > pk[i] ? target : pk[i] - 34 * fall;
        if (sm[i] > DB_FLOOR + 1) anyEnergy = true;
      }
    }

    if (anyEnergy) {
      ctx.beginPath();
      ctx.moveTo(0, plotBottom);
      for (let i = 0; i < SPECTRUM_BANDS; i += 1) {
        ctx.lineTo(SPECTRUM_BAND_POS[i] * w, specY(sm[i]));
      }
      ctx.lineTo(w, plotBottom);
      ctx.closePath();
      /* Deliberately weaker than the per-band curves (0.16) which are in turn
         weaker than the composite (0.95). The analyser is the room the curve
         stands in, not a second curve: at the alphas this used to use it read
         as a rival line crossing the composite. */
      const g = ctx.createLinearGradient(0, plotTop, 0, plotBottom);
      g.addColorStop(0, p.color("--eq-spec-top"));
      g.addColorStop(1, p.color("--eq-spec-bottom"));
      ctx.fillStyle = g;
      ctx.fill();

      // a dim edge on the live curve: without it the fill alone reads as haze
      ctx.beginPath();
      for (let i = 0; i < SPECTRUM_BANDS; i += 1) {
        const x = SPECTRUM_BAND_POS[i] * w;
        const y = specY(sm[i]);
        if (i === 0) ctx.moveTo(x, y);
        else ctx.lineTo(x, y);
      }
      ctx.strokeStyle = p.color("--eq-spec-edge");
      ctx.lineWidth = 1;
      ctx.stroke();

      // decaying peak hold, drawn as a hairline so it stays a backdrop
      ctx.beginPath();
      for (let i = 0; i < SPECTRUM_BANDS; i += 1) {
        const x = SPECTRUM_BAND_POS[i] * w;
        const y = specY(pk[i]);
        if (i === 0) ctx.moveTo(x, y);
        else ctx.lineTo(x, y);
      }
      ctx.strokeStyle = p.color("--eq-spec-peak");
      ctx.lineWidth = 1;
      ctx.stroke();
    }

    /* ── per-band curves ── */
    const enabled = config.enabled;
    if (enabled) {
      ctx.lineWidth = 1;
      for (const band of config.bands) {
        const buf = perBand.current.get(band.id);
        if (!buf || !band.enabled) continue;
        ctx.beginPath();
        for (let i = 0; i < CURVE_POINTS; i += 1) {
          const x = (i / (CURVE_POINTS - 1)) * w;
          const y = gainToY(clamp(buf[i], -GAIN_RANGE * 2, GAIN_RANGE * 2));
          if (i === 0) ctx.moveTo(x, y);
          else ctx.lineTo(x, y);
        }
        const hot = hoverBand.current === band.id || gesture.current?.bandId === band.id;
        ctx.strokeStyle = p.color(hot ? "--eq-band-hot" : "--eq-band");
        ctx.stroke();
      }
    }

    /* ── composite ── */
    const comp = composite.current;
    ctx.beginPath();
    for (let i = 0; i < CURVE_POINTS; i += 1) {
      const x = (i / (CURVE_POINTS - 1)) * w;
      const y = gainToY(clamp(comp[i], -GAIN_RANGE * 2, GAIN_RANGE * 2));
      if (i === 0) ctx.moveTo(x, y);
      else ctx.lineTo(x, y);
    }
    const strokeStyle = p.color(enabled ? "--eq-curve" : "--eq-curve-off");
    ctx.strokeStyle = strokeStyle;
    ctx.lineWidth = 1.7;
    ctx.lineJoin = "round";
    ctx.stroke();

    ctx.lineTo(w, gainToY(0));
    ctx.lineTo(0, gainToY(0));
    ctx.closePath();
    const fill = ctx.createLinearGradient(0, plotTop, 0, plotBottom);
    fill.addColorStop(0, p.color(enabled ? "--eq-fill-top" : "--eq-fill-off-top"));
    fill.addColorStop(1, p.color(enabled ? "--eq-fill-bottom" : "--eq-fill-off-bottom"));
    ctx.fillStyle = fill;
    ctx.fill();

    /* ── nodes ── */
    for (const band of config.bands) {
      const x = clamp(freqToX(band.freqHz), 8, w - 8);
      const y = gainToY(GAINLESS_KINDS.has(band.kind) ? 0 : clamp(band.gainDb, -GAIN_RANGE, GAIN_RANGE));
      const hot = hoverBand.current === band.id || gesture.current?.bandId === band.id;
      const r = hot ? 7.5 : 6;

      if (hot) {
        const glow = ctx.createRadialGradient(x, y, 0, x, y, 22);
        glow.addColorStop(0, p.color("--eq-node-glow"));
        glow.addColorStop(1, p.fade("--eq-node-glow", 0));
        ctx.fillStyle = glow;
        ctx.beginPath();
        ctx.arc(x, y, 22, 0, Math.PI * 2);
        ctx.fill();
      }

      ctx.beginPath();
      ctx.arc(x, y, r, 0, Math.PI * 2);
      ctx.fillStyle = p.color(band.enabled ? "--eq-node-fill" : "--eq-node-fill-off");
      ctx.fill();
      ctx.lineWidth = 1.4;
      ctx.strokeStyle = p.color(band.enabled ? "--eq-node-ring" : "--eq-node-ring-off");
      ctx.stroke();

      ctx.beginPath();
      ctx.arc(x, y, 2.1, 0, Math.PI * 2);
      ctx.fillStyle = band.enabled ? p.color("--eq-node-ring") : p.fade("--eq-node-ring-off", 0.86);
      ctx.fill();

      if (hot) {
        const label = `${formatFreqWithNote(band.freqHz)}${
          GAINLESS_KINDS.has(band.kind) ? "" : `  ${formatSignedDb(band.gainDb)} dB`
        }  Q ${band.q.toFixed(2)}`;
        ctx.font = p.font(10);
        const tw = ctx.measureText(label).width;
        const bw = tw + 14;
        const bx = clamp(x - bw / 2, 3, w - bw - 3);
        const by = clamp(y - 30, 3, h - 24);
        ctx.fillStyle = p.fade("--tip-bg", 1.022);
        ctx.fillRect(bx, by, bw, 19);
        ctx.strokeStyle = p.color("--eq-tip-line");
        ctx.lineWidth = 1;
        ctx.strokeRect(bx + 0.5, by + 0.5, bw - 1, 18);
        ctx.fillStyle = p.color("--tip-text");
        ctx.textAlign = "left";
        ctx.textBaseline = "middle";
        ctx.fillText(label, bx + 7, by + 10);
        ctx.textBaseline = "alphabetic";
      }
    }

    /* ── band-solo sweep overlay ── */
    // This window's own ref leads the engine by up to a frame, which is what
    // makes the sweep feel attached to the pointer; the engine's value is the
    // fallback so the overlay cannot be left dark by an audition this webview
    // did not start (or lit by one it has already forgotten about).
    const aud = auditionRef.current ?? frame?.audition ?? null;
    // the sweep outline is CSS on the wrapper; audition state lives in a ref,
    // so React never re-renders to set it and it has to be written here
    setAttr(wrapRef.current, "data-sweeping", aud ? "true" : "false");
    if (aud) {
      const cx = freqToX(aud.freqHz);
      const halfOct = 1.2 / Math.max(0.5, aud.q);
      const x0 = freqToX(clampFreq(aud.freqHz * Math.pow(2, -halfOct)));
      const x1 = freqToX(clampFreq(aud.freqHz * Math.pow(2, halfOct)));
      const band = ctx.createLinearGradient(x0, 0, x1, 0);
      band.addColorStop(0, p.fade("--solo-band", 0));
      band.addColorStop(0.5, p.color("--solo-band"));
      band.addColorStop(1, p.fade("--solo-band", 0));
      ctx.fillStyle = band;
      ctx.fillRect(x0, plotTop, Math.max(2, x1 - x0), plotH);
      ctx.strokeStyle = p.color("--solo-line");
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(Math.round(cx) + 0.5, plotTop);
      ctx.lineTo(Math.round(cx) + 0.5, plotBottom);
      ctx.stroke();

      const label = `BAND SOLO  ${formatFreqWithNote(aud.freqHz)}  Q ${aud.q.toFixed(1)}`;
      ctx.font = p.font(10);
      const tw = ctx.measureText(label).width;
      const bx = clamp(cx - (tw + 16) / 2, 3, w - tw - 19);
      ctx.fillStyle = p.color("--solo-chip");
      ctx.fillRect(bx, plotTop + 2, tw + 16, 18);
      ctx.fillStyle = p.color("--solo-chip-ink");
      ctx.textAlign = "left";
      ctx.textBaseline = "middle";
      ctx.fillText(label, bx + 8, plotTop + 11);
      ctx.textBaseline = "alphabetic";
    }

    /* ── the numeric read-outs, written straight into the DOM ── */
    const rows = rowsRef.current;
    if (rows) {
      for (const band of config.bands) {
        const row = rows.querySelector<HTMLElement>(`[data-band="${band.id}"]`);
        if (!row) continue;
        setText(row.querySelector<HTMLElement>(".f"), formatFreqWithNote(band.freqHz));
        setText(
          row.querySelector<HTMLElement>(".g"),
          GAINLESS_KINDS.has(band.kind) ? "\u2014" : `${formatSignedDb(band.gainDb)} dB`,
        );
        setText(row.querySelector<HTMLElement>(".q"), `Q ${band.q.toFixed(2)}`);
      }
    }

    const hovered = config.bands.find((b) => b.id === hoverBand.current);
    const cur = cursor.current;
    setText(
      readoutRef.current,
      aud
        ? `SOLO ${formatFreqWithNote(aud.freqHz)} \u00B7 Q ${aud.q.toFixed(1)}`
        : hovered
          ? `${FILTER_LABEL[hovered.kind]} \u00B7 ${formatFreqWithNote(hovered.freqHz)}`
          : cur
            ? formatFreqWithNote(posToFreq(cur.x / Math.max(1, w)))
            : `${config.bands.length} / ${MAX_EQ_BANDS} bands`,
    );
  });

  /* ── hit testing + pointer gestures ────────────────────────────────────── */

  const geometry = useCallback(() => {
    const { w, h } = surface.current;
    const plotTop = PAD_TOP;
    const plotBottom = h - PAD_BOTTOM;
    const plotH = Math.max(1, plotBottom - plotTop);
    const midY = plotTop + plotH / 2;
    return {
      w,
      h,
      plotTop,
      plotBottom,
      plotH,
      midY,
      gainToY: (db: number) => midY - (db / GAIN_RANGE) * (plotH / 2),
      yToGain: (y: number) => ((midY - y) / (plotH / 2)) * GAIN_RANGE,
      freqToX: (f: number) => freqToPos(f) * w,
      xToFreq: (x: number) => posToFreq(x / Math.max(1, w)),
    };
  }, [surface]);

  const localPoint = useCallback((e: { clientX: number; clientY: number }) => {
    const el = wrapRef.current;
    if (!el) return { x: 0, y: 0 };
    const rect = el.getBoundingClientRect();
    return { x: e.clientX - rect.left, y: e.clientY - rect.top };
  }, []);

  const hitTest = useCallback(
    (x: number, y: number): EqBand | null => {
      const g = geometry();
      let best: EqBand | null = null;
      let bestD = HIT_RADIUS * HIT_RADIUS;
      for (const band of cfgRef.current.bands) {
        const bx = clamp(g.freqToX(band.freqHz), 8, g.w - 8);
        const by = g.gainToY(
          GAINLESS_KINDS.has(band.kind) ? 0 : clamp(band.gainDb, -GAIN_RANGE, GAIN_RANGE),
        );
        const d = (bx - x) * (bx - x) + (by - y) * (by - y);
        if (d <= bestD) {
          bestD = d;
          best = band;
        }
      }
      return best;
    },
    [geometry],
  );

  const beginSweep = useCallback(
    (x: number, y: number) => {
      const g = geometry();
      const freq = clampFreq(g.xToFreq(x));
      const q = soloQForY((y - g.plotTop) / g.plotH);
      setAudition({ freqHz: freq, q });
    },
    [geometry],
  );

  const onPointerDown = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      if (e.button === 2) return; // context menu handles its own thing
      // A second pointer (a stray touch, a pen) landing mid-gesture used to
      // overwrite the first one's record: a sweep replaced by a node drag left
      // the audition with no gesture left to end it.
      if (gesture.current) return;
      const { x, y } = localPoint(e);
      setMenu(null);
      const el = e.currentTarget;
      el.setPointerCapture(e.pointerId);

      if (e.metaKey || e.ctrlKey) {
        gesture.current = {
          kind: "sweep",
          pointerId: e.pointerId,
          bandId: -1,
          startX: x,
          startY: y,
          startFreq: 0,
          startGain: 0,
          altAtStart: false,
          moved: false,
        };
        beginSweep(x, y);
        return;
      }

      const band = hitTest(x, y);
      if (band) {
        gesture.current = {
          kind: "node",
          pointerId: e.pointerId,
          bandId: band.id,
          startX: x,
          startY: y,
          startFreq: band.freqHz,
          startGain: band.gainDb,
          altAtStart: e.altKey,
          moved: false,
        };
        return;
      }

      gesture.current = {
        kind: "empty",
        pointerId: e.pointerId,
        bandId: -1,
        startX: x,
        startY: y,
        startFreq: 0,
        startGain: 0,
        altAtStart: e.altKey,
        moved: false,
      };
    },
    [beginSweep, hitTest, localPoint],
  );

  const onPointerMove = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      const { x, y } = localPoint(e);
      cursor.current = { x, y };
      const g = gesture.current;

      if (!g) {
        const band = hitTest(x, y);
        hoverBand.current = band?.id ?? null;
        return;
      }

      if (Math.abs(x - g.startX) > CLICK_SLOP || Math.abs(y - g.startY) > CLICK_SLOP) g.moved = true;

      if (g.kind === "sweep") {
        beginSweep(x, y);
        return;
      }
      if (g.kind !== "node") return;

      const geo = geometry();
      const band = cfgRef.current.bands.find((b) => b.id === g.bandId);
      if (!band) return;

      // Shift constrains to gain, Alt to frequency (SPEC §12)
      const freq = e.shiftKey ? g.startFreq : clampFreq(geo.xToFreq(x));
      const gain =
        e.altKey || GAINLESS_KINDS.has(band.kind)
          ? g.startGain
          : clamp(geo.yToGain(y), GAIN_MIN, GAIN_MAX);

      patchBand(
        band.id,
        { freqHz: Math.round(freq * 10) / 10, gainDb: Math.round(gain * 10) / 10 },
        false,
      );
    },
    [beginSweep, geometry, hitTest, localPoint, patchBand],
  );

  const endGesture = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      const g = gesture.current;
      gesture.current = null;
      if (e.currentTarget.hasPointerCapture(e.pointerId)) {
        e.currentTarget.releasePointerCapture(e.pointerId);
      }
      if (!g) return;

      if (g.kind === "sweep") {
        stopAudition();
        return;
      }

      // The window can be closed in the same breath as this pointer-up; a
      // config still sitting in the rAF queue would never be sent.
      flushNow();

      const { x, y } = localPoint(e);

      if (g.kind === "node") {
        const band = cfgRef.current.bands.find((b) => b.id === g.bandId);
        if (!band) return;
        // Alt + click (no drag) bypasses the band
        if (!g.moved && g.altAtStart) {
          patchBand(band.id, { enabled: !band.enabled }, true);
          return;
        }
        // commit the structural mirror so the band list catches up
        setCfgState(cfgRef.current);
        return;
      }

      // empty space: a real click creates a bell; a drag is a deliberate no-op
      if (g.moved) return;
      const cur = cfgRef.current;
      if (cur.bands.length >= MAX_EQ_BANDS) {
        pushToast("warn", `The EQ is limited to ${MAX_EQ_BANDS} bands`);
        return;
      }
      const geo = geometry();
      const freq = clampFreq(geo.xToFreq(x));
      const gain = clamp(geo.yToGain(y), -GAIN_RANGE, GAIN_RANGE);
      const band = makeBand(cur, "bell", freq, Math.round(gain * 10) / 10, 1);
      hoverBand.current = band.id;
      commit({ ...cur, bands: [...cur.bands, band] }, true);
    },
    [commit, flushNow, geometry, localPoint, patchBand, pushToast],
  );

  /* wheel over a node → Q. Non-passive so the drawer never scrolls instead. */
  useEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent): void => {
      const rect = el.getBoundingClientRect();
      const band = hitTest(e.clientX - rect.left, e.clientY - rect.top);
      if (!band) return;
      e.preventDefault();
      const step = e.shiftKey ? 0.0006 : 0.0022;
      const q = clamp(band.q * Math.exp(-e.deltaY * step), Q_MIN, Q_MAX);
      patchBand(band.id, { q: Math.round(q * 100) / 100 }, false);
      setCfgState(cfgRef.current);
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, [hitTest, patchBand]);

  /* ── band list actions ─────────────────────────────────────────────────── */

  const removeBand = useCallback(
    (id: number) => {
      const cur = cfgRef.current;
      commit({ ...cur, bands: cur.bands.filter((b) => b.id !== id) }, true);
      if (hoverBand.current === id) hoverBand.current = null;
    },
    [commit],
  );

  const soloBand = useCallback((band: EqBand | null) => {
    if (!band) {
      stopAudition();
      return;
    }
    setAudition({ freqHz: band.freqHz, q: Math.max(2, band.q) });
  }, []);

  /* close the type menu on any outside interaction */
  useEffect(() => {
    if (!menu) return;
    const close = (): void => setMenu(null);
    window.addEventListener("pointerdown", close, { capture: true });
    window.addEventListener("blur", close);
    return () => {
      window.removeEventListener("pointerdown", close, { capture: true });
      window.removeEventListener("blur", close);
    };
  }, [menu]);

  /* Safety net for gestures the element never sees the end of.
     A Cmd-drag is the worst case: Cmd-Tab away mid-sweep and the pointerup is
     delivered to another application, so without this the audition bandpass
     stays engaged and the engineer is left monitoring a filtered signal with no
     visible cause. Blur covers that, and clearing any gesture (not just the
     sweep) stops a node drag resuming without the button held when focus
     returns. */
  useEffect(() => {
    const end = (): void => {
      const g = gesture.current;
      gesture.current = null;
      if (auditionRef.current != null) stopAudition();
      // the ref moved while dragging; let the band list catch up
      if (g?.kind === "node") setCfgState(cfgRef.current);
    };
    window.addEventListener("pointerup", end);
    window.addEventListener("pointercancel", end);
    window.addEventListener("blur", end);
    return () => {
      window.removeEventListener("pointerup", end);
      window.removeEventListener("pointercancel", end);
      window.removeEventListener("blur", end);
    };
  }, []);

  const menuBand = useMemo(
    () => (menu ? cfg.bands.find((b) => b.id === menu.bandId) ?? null : null),
    [cfg.bands, menu],
  );

  const setEnabled = useCallback(
    (on: boolean) => {
      commit({ ...cfgRef.current, enabled: on }, true);
    },
    [commit],
  );

  return (
    <div className="eq-panel">
      <div className="eq-inner">
        <div className="eq-left">
          <div className="eq-head">
            <span className="label">{t("Equaliser")}</span>
            <button
              className="tr-toggle"
              data-on={cfg.enabled}
              onClick={() => setEnabled(!cfg.enabled)}
              title={t("EQ bypass (\u21E7E)")}
            >
              {t(cfg.enabled ? "Engaged" : "Bypassed")}
            </button>
            <span className="eq-readout num" ref={readoutRef} />
            <span className="spacer" />
            {/* Two spans, not one: the header runs out of room long before the
                window does, and the solo sweep is the half worth keeping. */}
            <span className="eq-hint eq-head-hint">
              <span className="verbose">
                {t("drag = freq / gain \u00B7 wheel = Q \u00B7 dbl-click = delete ")}
              </span>
              <b>{t("\u2318/Ctrl-drag = solo sweep")}</b>
            </span>
            <button
              className="ghost-btn"
              disabled={cfg.bands.length === 0}
              onClick={() => commit({ ...cfgRef.current, bands: [] }, true)}
              title={t("Remove every band")}
            >{t("Clear")}</button>
            {/* Always-on-top is the plugin-editor default and a preference, not
                a law: on a second monitor it buys nothing, and Rust remembers
                the answer in settings.json. */}
            <button
              className="ghost-btn"
              data-on={pinned}
              onClick={() => setEqWindowPinned(!pinned)}
              title={t(pinned
                  ? "Floating above other windows \u00B7 click to let it go behind"
                  : "Behind other windows \u00B7 click to float it on top")}
              aria-pressed={pinned}
            >
              {t(pinned ? "Float" : "Behind")}
            </button>
            <button className="close-btn" onClick={closeEqWindow} title={t("Close (E or Esc)")}>
              <IconClose />
            </button>
          </div>

          <div
            className="eq-canvas-wrap"
            ref={wrapRef}
            onPointerDown={onPointerDown}
            onPointerMove={onPointerMove}
            onPointerUp={endGesture}
            onPointerCancel={endGesture}
            onPointerLeave={() => {
              if (!gesture.current) {
                hoverBand.current = null;
                cursor.current = null;
              }
            }}
            onDoubleClick={(e) => {
              const { x, y } = localPoint(e);
              const band = hitTest(x, y);
              if (band) removeBand(band.id);
            }}
            onContextMenu={(e) => {
              e.preventDefault();
              const { x, y } = localPoint(e);
              const band = hitTest(x, y);
              if (band) setMenu({ x, y, bandId: band.id });
            }}
          >
            <canvas ref={canvasRef} />
            {t(menu && menuBand && (
              <div
                className="eq-menu"
                style={{ left: Math.min(menu.x, (surface.current.w || 0) - 168), top: menu.y }}
                onPointerDown={(e) => e.stopPropagation()}
              >
                <div className="eq-menu-head label">{t("Filter type")}</div>
                {t(FILTER_KINDS.map((kind) => (
                  <button
                    key={kind}
                    className="eq-menu-item"
                    data-on={menuBand.kind === kind}
                    onClick={() => {
                      patchBand(menuBand.id, kindPatch(kind, menuBand), true);
                      setMenu(null);
                    }}
                  >
                    {t(FILTER_LABEL[kind])}
                  </button>
                )))}
                {t((menuBand.kind === "highPass" || menuBand.kind === "lowPass") && (
                  <>
                    <div className="eq-menu-head label">{t("Slope")}</div>
                    <div className="eq-menu-slopes">
                      {t(SLOPE_CHOICES.map((s) => (
                        <button
                          key={s}
                          className="num"
                          data-on={menuBand.slopeDbOct === s}
                          onClick={() => {
                            patchBand(menuBand.id, { slopeDbOct: s }, true);
                            setMenu(null);
                          }}
                        >
                          {t(s)}
                        </button>
                      )))}
                    </div>
                  </>
                ))}
                <button
                  className="eq-menu-item"
                  data-danger="true"
                  onClick={() => {
                    removeBand(menuBand.id);
                    setMenu(null);
                  }}
                >{t("Delete band")}</button>
              </div>
            ))}
          </div>
        </div>

        <div className="eq-right">
          <div className="eq-right-head">
            <span className="label">{t("Bands")}</span>
            <span className="spacer" />
            <span className="num eq-count">
              {t(cfg.bands.length)} / {t(MAX_EQ_BANDS)}
            </span>
          </div>

          <div className="eq-rows" ref={rowsRef}>
            {t(cfg.bands.length === 0 && (
              <div className="eq-empty">{t("No bands. Click anywhere on the curve to add one, or hold")}{t("\u2318")}{t("/Ctrl and drag to sweep a solo bandpass across the spectrum.")}</div>
            ))}
            {t(cfg.bands.map((band) => (
              <div
                key={band.id}
                className="eq-row"
                data-band={band.id}
                data-off={!band.enabled}
                onPointerEnter={() => {
                  hoverBand.current = band.id;
                }}
                onPointerLeave={() => {
                  if (hoverBand.current === band.id) hoverBand.current = null;
                }}
              >
                <button
                  className="eq-row-kind"
                  onClick={() =>
                    patchBand(band.id, kindPatch(nextKind(band.kind), band), true)
                  }
                  title={t("Cycle the filter type (or right-click the node)")}
                >
                  {t(FILTER_SHORT[band.kind])}
                </button>
                <span className="f num" />
                <span className="g num" />
                <span className="q num" />
                <button
                  className="eq-row-btn solo"
                  title={t("Hold to audition this band's frequency region")}
                  onPointerDown={(e) => {
                    e.currentTarget.setPointerCapture(e.pointerId);
                    soloBand(band);
                  }}
                  onPointerUp={() => soloBand(null)}
                  onPointerCancel={() => soloBand(null)}
                >
                  {t("\u25CE")}
                </button>
                <button
                  className="eq-row-btn"
                  data-on={band.enabled}
                  title={t("Bypass this band (Alt-click its node)")}
                  onClick={() => patchBand(band.id, { enabled: !band.enabled }, true)}
                >
                  {t("\u00F8")}
                </button>
                <button
                  className="eq-row-btn danger"
                  title={t("Delete this band (double-click its node)")}
                  onClick={() => removeBand(band.id)}
                >
                  {t("\u00D7")}
                </button>
              </div>
            )))}
          </div>

          {t(blindActive && (
            <div className="eq-hint" style={{ color: "var(--m-warn)" }}>{t("The analyser is masked while a blind test is running: a live spectrum of the audible slot names it as plainly as a meter would.")}</div>
          ))}

          <div className="eq-hint eq-foot-hint">{t("Click the curve to add a bell, right-click a node for its filter type. Drag a node for frequency and gain, wheel over it for Q, double-click to remove it. Hold")}{t("\u2318")}{t("/Ctrl and drag to sweep a solo bandpass. The curve is drawn from the same biquad coefficients the engine runs, not a sketch. Meters stay on the true programme.")}</div>
        </div>
      </div>
    </div>
  );
}

function nextKind(kind: FilterKind): FilterKind {
  return FILTER_KINDS[(FILTER_KINDS.indexOf(kind) + 1) % FILTER_KINDS.length];
}

/** Retyping a band has to keep it sane: gainless kinds forget their gain. */
function kindPatch(kind: FilterKind, band: EqBand): Partial<EqBand> {
  if (GAINLESS_KINDS.has(kind)) {
    return { kind, gainDb: 0, q: clamp(band.q, 0.3, Q_MAX), slopeDbOct: band.slopeDbOct || 12 };
  }
  return { kind, q: clamp(band.q, Q_MIN, Q_MAX) };
}
