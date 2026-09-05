import { useRef } from "react";
import { beginPaint, useSurface } from "../lib/canvas";
import { useFrameEffect } from "../lib/frame";
import { paint, type Paint, type PaintToken } from "../lib/theme";

/** dB → 0..1 along the scale. Piecewise so the top of the scale breathes. */
const ANCHORS: Array<[number, number]> = [
  [-60, 0],
  [-48, 0.1],
  [-36, 0.22],
  [-24, 0.38],
  [-18, 0.48],
  [-12, 0.6],
  [-6, 0.75],
  [-3, 0.85],
  [0, 1],
];
/** Labels are dropped, coarsest last, until they stop colliding. */
const LABEL_TIERS: number[][] = [
  [0, -3, -6, -12, -18, -24, -36, -48, -60],
  [0, -6, -12, -18, -24, -36, -60],
  [0, -6, -12, -24, -60],
  [0, -12, -60],
];

function dbToNorm(db: number): number {
  if (!Number.isFinite(db)) return 0;
  if (db <= -60) return 0;
  if (db >= 0) return 1;
  for (let i = 1; i < ANCHORS.length; i += 1) {
    const [d1, n1] = ANCHORS[i];
    const [d0, n0] = ANCHORS[i - 1];
    if (db <= d1) return n0 + ((db - d0) / (d1 - d0)) * (n1 - n0);
  }
  return 1;
}

/** The peak-hold tick: the loudest thing on the meter, so it is fully saturated. */
function holdToken(db: number): PaintToken {
  if (db >= -0.2) return "--m-clip";
  if (db >= -1) return "--m-hot";
  if (db >= -6) return "--m-warn";
  return "--m-safe";
}

/**
 * SPEC §4's meter scale, held back at the safe end so it does not shout.
 *
 * The four stops are tokens, and each theme sets both the hue and how far it is
 * held back: on paper the same alphas are a pastel, so `--mt-*` in the light
 * block is a good deal denser than in the dark one.
 */
function scaleStops(g: CanvasGradient, p: Paint): CanvasGradient {
  g.addColorStop(0, p.color("--mt-safe"));
  g.addColorStop(dbToNorm(-12), p.color("--mt-safe-lo"));
  g.addColorStop(dbToNorm(-6), p.color("--mt-warn"));
  g.addColorStop(dbToNorm(-1), p.color("--mt-hot"));
  g.addColorStop(1, p.color("--mt-clip"));
  return g;
}

/**
 * Peak / RMS / peak-hold, L and R, in whatever box the layout gives it.
 *
 * Two orientations, chosen from the box's own aspect rather than from a prop:
 * beside the waveform lanes it is a tall pair of columns; in a narrow window
 * the meter cluster becomes a strip under the lanes (`app.css`, "Narrow
 * windows") and the same meter lies down into two rows. A 40 px-tall vertical
 * meter is a decoration — a 40 px-tall horizontal one is still a meter, with
 * the whole dB scale spread over the width the strip does have.
 *
 * Either way the legend thins itself out instead of printing labels on top of
 * each other, and both paths read `surface.current` every frame, so a resize or
 * a devicePixelRatio change flips or rescales it without a remount.
 */
export default function LevelMeter() {
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const surface = useSurface(canvasRef, { measure: wrapRef });

  useFrameEffect((frame) => {
    const { w, h } = surface.current;
    const ctx = beginPaint(canvasRef.current, surface.current);
    if (!ctx) return;
    const p = paint();
    const MONO = p.font(7.5);
    const SANS = p.fontUi(7.5);

    const meters = frame?.meters;
    const peaks = meters?.peakDb ?? [-144, -144];
    const rms = meters?.rmsDb ?? [-144, -144];
    const holds = meters?.peakHoldDb ?? [-144, -144];

    /* ── lying down: two rows, scale across the width ─────────────────── */
    if (w > h * 2.2) {
      const left = 11; // the L / R letters
      const right = w - 7; // room for the "0" at the top of the scale
      const usable = right - left;
      if (usable <= 20) return;
      const legendH = 9;
      const gap = 3;
      const barH = Math.max(4, Math.min(11, (h - legendH - gap) / 2));
      const top = Math.max(0, (h - legendH - gap - barH * 2) / 2);

      const x = (db: number): number => left + dbToNorm(db) * usable;
      const labels =
        LABEL_TIERS.find((tier) => usable / tier.length >= 24) ?? LABEL_TIERS[3];
      const bottom = top + barH * 2 + gap;

      ctx.font = MONO;
      ctx.textAlign = "center";
      ctx.textBaseline = "top";
      for (const [db] of ANCHORS) {
        const xx = Math.round(x(db)) + 0.5;
        ctx.strokeStyle = p.color(db === 0 ? "--mt-grid-zero" : "--mt-grid");
        ctx.lineWidth = 1;
        ctx.beginPath();
        ctx.moveTo(xx, top);
        ctx.lineTo(xx, bottom);
        ctx.stroke();
        if (labels.includes(db)) {
          ctx.fillStyle = p.color(db === 0 ? "--mt-label-zero" : "--mt-label");
          ctx.fillText(db === 0 ? "0" : `${Math.abs(db)}`, xx, bottom + 1);
        }
      }

      for (let ch = 0; ch < 2; ch += 1) {
        const y = top + ch * (barH + gap);

        ctx.fillStyle = p.color("--mt-track");
        ctx.fillRect(left, y, usable, barH);

        const peak = peaks[ch] ?? -144;
        if (peak > -60) {
          ctx.fillStyle = scaleStops(ctx.createLinearGradient(left, 0, right, 0), p);
          ctx.fillRect(left, y, x(peak) - left, barH);
        }

        const r = rms[ch] ?? -144;
        if (r > -60) {
          ctx.fillStyle = p.color("--mt-rms");
          ctx.fillRect(left, y, x(r) - left, barH);
        }

        const hold = holds[ch] ?? -144;
        if (hold > -60) {
          ctx.fillStyle = p.color(holdToken(hold));
          ctx.fillRect(Math.round(x(hold)) - 1, y, 1.5, barH);
        }

        ctx.fillStyle = p.color("--mt-chan");
        ctx.font = SANS;
        ctx.textAlign = "left";
        ctx.textBaseline = "middle";
        ctx.fillText(ch === 0 ? "L" : "R", 1, y + barH / 2 + 0.5);
        ctx.textBaseline = "top";
        ctx.font = MONO;
        ctx.textAlign = "center";
      }
      return;
    }

    /* ── standing up: two columns beside the lanes ────────────────────── */
    const top = 4;
    const bottom = h - 9;
    const usable = bottom - top;
    if (usable <= 8) return;
    const gap = 4;
    // the legend needs ~15 px; the bars take the rest, up to a sane width
    const barW = Math.max(4, Math.min(12, (w - 15 - gap) / 2));
    const barsX = w - barW * 2 - gap;

    const y = (db: number): number => bottom - dbToNorm(db) * usable;

    // legend: every anchor gets a gridline, only as many get a number as fit
    const labels = LABEL_TIERS.find((tier) => usable / tier.length >= 15) ?? LABEL_TIERS[3];
    ctx.font = MONO;
    ctx.textAlign = "right";
    ctx.textBaseline = "middle";
    for (const [db] of ANCHORS) {
      const yy = Math.round(y(db)) + 0.5;
      if (labels.includes(db)) {
        ctx.fillStyle = p.color(db === 0 ? "--mt-label-zero" : "--mt-label");
        ctx.fillText(db === 0 ? "0" : `${Math.abs(db)}`, barsX - 4, yy);
      }
      ctx.strokeStyle = p.color(db === 0 ? "--mt-grid-zero" : "--mt-grid");
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(barsX, yy);
      ctx.lineTo(w, yy);
      ctx.stroke();
    }

    for (let ch = 0; ch < 2; ch += 1) {
      const x = barsX + ch * (barW + gap);

      ctx.fillStyle = p.color("--mt-track");
      ctx.fillRect(x, top, barW, usable);

      const peak = peaks[ch] ?? -144;
      if (peak > -60) {
        const py = y(peak);
        ctx.fillStyle = scaleStops(ctx.createLinearGradient(0, bottom, 0, top), p);
        ctx.fillRect(x, py, barW, bottom - py);
      }

      // RMS body — a denser inner column
      const r = rms[ch] ?? -144;
      if (r > -60) {
        const ry = y(r);
        ctx.fillStyle = p.color("--mt-rms");
        ctx.fillRect(x, ry, barW, bottom - ry);
      }

      // peak-hold tick
      const hold = holds[ch] ?? -144;
      if (hold > -60) {
        ctx.fillStyle = p.color(holdToken(hold));
        ctx.fillRect(x, Math.round(y(hold)) - 1, barW, 1.5);
      }

      ctx.fillStyle = p.color("--mt-chan");
      ctx.textAlign = "center";
      ctx.font = SANS;
      ctx.fillText(ch === 0 ? "L" : "R", x + barW / 2, h - 3);
    }
  });

  return (
    <div className="wm-level" ref={wrapRef}>
      <canvas ref={canvasRef} />
    </div>
  );
}
