import { useRef } from "react";
import { beginPaint, useSurface } from "../lib/canvas";
import { setText } from "../lib/dom";
import { useFrameEffect } from "../lib/frame";
import { paint } from "../lib/theme";

/**
 * Stereo correlation, −1 (out of phase) … +1 (mono), as one hairline strip.
 * It earns its place next to the waveform only by staying this small: the label
 * and the number are DOM, so the canvas is 10 px of track and nothing else.
 */
export default function Correlation() {
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const surface = useSurface(canvasRef, { measure: wrapRef, height: 10 });
  const valEl = useRef<HTMLSpanElement | null>(null);
  const smooth = useRef(0);

  useFrameEffect((frame, _t, dt) => {
    const target = Math.max(-1, Math.min(1, frame?.meters.correlation ?? 0));
    smooth.current += (target - smooth.current) * Math.min(1, dt / 120);
    const v = smooth.current;

    setText(valEl.current, `${v < 0 ? "\u2212" : "+"}${Math.abs(v).toFixed(2)}`);

    const { w, h } = surface.current;
    const ctx = beginPaint(canvasRef.current, surface.current);
    if (!ctx) return;
    const p = paint();

    const trackY = Math.round((h - 4) / 2);
    ctx.fillStyle = p.color("--corr-track");
    ctx.fillRect(0, trackY, w, 4);

    const mid = w / 2;
    const x = mid + (v * w) / 2;

    const g = ctx.createLinearGradient(0, 0, w, 0);
    g.addColorStop(0, p.color("--corr-neg"));
    g.addColorStop(0.5, p.color("--corr-mid"));
    g.addColorStop(1, p.color("--corr-pos"));
    ctx.fillStyle = g;
    if (x >= mid) ctx.fillRect(mid, trackY, x - mid, 4);
    else ctx.fillRect(x, trackY, mid - x, 4);

    // centre reference, then the needle
    ctx.fillStyle = p.color("--corr-centre");
    ctx.fillRect(Math.round(mid), trackY - 2, 1, 8);
    ctx.fillStyle = p.color(v < 0 ? "--corr-needle-neg" : "--corr-needle-pos");
    ctx.fillRect(Math.round(x) - 1, trackY - 3, 2, 10);
  });

  return (
    <div className="wm-corr">
      <span className="wm-k">Corr</span>
      <div className="wm-corr-track" ref={wrapRef}>
        <canvas ref={canvasRef} />
      </div>
      <span className="wm-v num" ref={valEl}>
        +0.00
      </span>
    </div>
  );
}
