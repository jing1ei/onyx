import { t } from "../lib/i18n";
/**
 * The metering cluster that sits to the right of the waveform lanes.
 *
 * It replaces the old full-height right-hand meter bridge: same numbers a
 * working engineer actually glances at — momentary / short-term / integrated
 * LUFS, true peak, LRA, the L/R level meters, correlation — with the section
 * heads, folds and standalone analyser gone. It is sized by the waveform
 * region, so it has to survive both A/B layouts: one lane (~150 px tall) and
 * two stacked lanes (~220 px). Only the level meter grows; everything else is
 * fixed, small and legible.
 *
 * Below 880 px of window the same DOM is laid out as a strip underneath the
 * lanes instead of a column beside them (`app.css`, "Narrow windows"), down to
 * the 420 × 560 minimum window: the numbers keep their size, the loudness scale
 * and the L/R meter take the width, and `LevelMeter` turns itself on its side.
 *
 * Every value is written from the frame loop straight into the DOM, and every
 * one of them is masked while a blind test runs: two masters 0.4 LUFS apart are
 * told apart by watching a number instead of listening (SPEC §7).
 */

import { useRef } from "react";
import * as api from "../lib/api";
import { beginPaint, useSurface } from "../lib/canvas";
import { setAttr, setText } from "../lib/dom";
import { useFrameEffect } from "../lib/frame";
import { formatDb, formatLu, formatLufs, formatSampleRate } from "../lib/format";
import { useStore } from "../lib/store";
import { paint } from "../lib/theme";
import Correlation from "./Correlation";
import LevelMeter from "./LevelMeter";

/** `···` stands in for every numeric read-out while a blind test is running. */
const HIDDEN = "\u00B7\u00B7\u00B7";

/* ── the loudness scale ────────────────────────────────────────────────────
   One strip carrying all three EBU R128 windows against a −14 LUFS reference:
   momentary as the fill, short-term as a steel tick, integrated as a diamond.
   It is the one graphic here worth its height — the numbers above say where
   the loudness *is*, this says where it is going and how far off target. */

const LU_MIN = -40;
const LU_MAX = 0;
const LU_TARGET = -14;
const LU_BAR_H = 18;

const luNorm = (lufs: number): number =>
  Math.max(0, Math.min(1, (lufs - LU_MIN) / (LU_MAX - LU_MIN)));

function LoudnessBar() {
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const surface = useSurface(canvasRef, { measure: wrapRef, height: LU_BAR_H });

  useFrameEffect((frame) => {
    const m = frame?.meters;
    const { w } = surface.current;
    const ctx = beginPaint(canvasRef.current, surface.current);
    if (!ctx) return;
    const p = paint();

    const barY = 1;
    const barH = 9;

    ctx.fillStyle = p.color("--lu-track");
    ctx.fillRect(0, barY, w, barH);

    const mom = m?.lufsMomentary ?? -70;
    if (mom > LU_MIN) {
      const g = ctx.createLinearGradient(0, 0, w, 0);
      g.addColorStop(0, p.color("--lu-safe"));
      g.addColorStop(luNorm(-18), p.color("--lu-warn"));
      g.addColorStop(luNorm(-9), p.color("--lu-hot"));
      g.addColorStop(1, p.color("--lu-clip"));
      ctx.fillStyle = g;
      ctx.fillRect(0, barY, luNorm(mom) * w, barH);
    }

    // target reference, labelled: an unlabelled hairline is just a scratch
    const tx = Math.round(luNorm(LU_TARGET) * w) + 0.5;
    ctx.strokeStyle = p.color("--lu-target");
    ctx.setLineDash([2, 2]);
    ctx.beginPath();
    ctx.moveTo(tx, barY - 1);
    ctx.lineTo(tx, barY + barH + 1);
    ctx.stroke();
    ctx.setLineDash([]);
    ctx.font = p.font(7.5);
    ctx.fillStyle = p.color("--lu-target-label");
    ctx.textAlign = "center";
    ctx.textBaseline = "top";
    ctx.fillText("\u221214", tx, barY + barH + 2);

    const st = m?.lufsShort ?? -70;
    if (st > LU_MIN) {
      const x = Math.round(luNorm(st) * w) + 0.5;
      ctx.fillStyle = p.color("--lu-short");
      ctx.fillRect(x - 1, barY - 1, 2, barH + 2);
    }

    const it = m?.lufsIntegrated ?? -70;
    if (it > LU_MIN) {
      const x = luNorm(it) * w;
      ctx.fillStyle = p.color("--lu-int");
      ctx.beginPath();
      ctx.moveTo(x, barY + barH / 2 - 4);
      ctx.lineTo(x + 3.2, barY + barH / 2);
      ctx.lineTo(x, barY + barH / 2 + 4);
      ctx.lineTo(x - 3.2, barY + barH / 2);
      ctx.closePath();
      ctx.fill();
    }
  });

  return (
    <div className="wm-lu" ref={wrapRef}>
      <canvas ref={canvasRef} />
    </div>
  );
}

export default function MeterCluster() {
  const pushToast = useStore((s) => s.pushToast);
  const blindActive = useStore((s) => s.snapshot?.blind.active ?? false);

  const intEl = useRef<HTMLSpanElement | null>(null);
  const momEl = useRef<HTMLSpanElement | null>(null);
  const shortEl = useRef<HTMLSpanElement | null>(null);
  const tpEl = useRef<HTMLSpanElement | null>(null);
  const lraEl = useRef<HTMLSpanElement | null>(null);
  /** the true-peak cell doubles as the clip indicator and the meter reset */
  const tpCellEl = useRef<HTMLButtonElement | null>(null);
  const tpKeyEl = useRef<HTMLSpanElement | null>(null);
  const rateEl = useRef<HTMLSpanElement | null>(null);
  const transEl = useRef<HTMLSpanElement | null>(null);
  useFrameEffect((frame) => {
    const m = frame?.meters;
    const t = frame?.transport;
    /* Read straight off the render's own state: `useFrameEffect` re-points the
       subscription at every render, so the painter sees the current value
       without a hand-written mirror ref (SPEC §7 — a leaked read-out is the
       failure this guards). */
    const hide = blindActive;

    setText(intEl.current, hide ? HIDDEN : formatLufs(m?.lufsIntegrated ?? -70));
    setText(momEl.current, hide ? HIDDEN : formatLufs(m?.lufsMomentary ?? -70));
    setText(shortEl.current, hide ? HIDDEN : formatLufs(m?.lufsShort ?? -70));
    setText(lraEl.current, hide ? HIDDEN : formatLu(m?.lra ?? 0));

    const tp = Math.max(m?.truePeakDb[0] ?? -144, m?.truePeakDb[1] ?? -144);
    setText(tpEl.current, hide ? HIDDEN : formatDb(tp));
    setAttr(tpEl.current, "data-tone", hide ? "" : tp > -0.1 ? "clip" : tp > -1 ? "hot" : "");

    // The clip counter has no pill of its own: the true-peak cell *is* the
    // clip light, and clicking it is what resets the holds. One control, and
    // it sits on the number it is about.
    const clipped = !hide && (m?.clipCount ?? 0) > 0;
    setAttr(tpCellEl.current, "data-on", String(clipped));
    setText(tpKeyEl.current, clipped ? "CLIP" : "TP");

    // Engine rate and bit-transparency follow the *audible* deck, so with
    // follow-source-rate on, or a trim on one deck, these flipped as the
    // subject switched slots and spelled out the mapping (SPEC §7).
    setText(rateEl.current, hide ? HIDDEN : formatSampleRate(t?.engineSampleRate ?? 0));
    setText(
      transEl.current,
      hide ? HIDDEN : t?.bitTransparent ? "bit-transparent" : "processed",
    );
    setAttr(transEl.current, "data-ok", String(!hide && t?.bitTransparent === true));
  });

  return (
    <aside className="wave-meters" data-blind={blindActive}>
      <div className="wm-stack">
        <div className="wm-head">
          <span className="wm-int num" ref={intEl}>
            {t("\u2212\u221E")}
          </span>
          <span className="wm-int-u">{t("LUFS")}<em>{t("I")}</em>
          </span>
        </div>

        <div className="wm-grid">
          <div className="wm-cell">
            <span className="wm-k">{t("M")}</span>
            <span className="wm-v num" ref={momEl}>
              {t("\u2014")}
            </span>
          </div>
          <div className="wm-cell">
            <span className="wm-k">{t("S")}</span>
            <span className="wm-v num" ref={shortEl}>
              {t("\u2014")}
            </span>
          </div>
          <button
            className="wm-cell wm-tp"
            ref={tpCellEl}
            data-on="false"
            onClick={() =>
              void api
                .resetMeters()
                .catch((err) =>
                  pushToast("error", `Could not reset meters: ${api.errorMessage(err)}`),
                )
            }
            title={t("True peak, dBTP \u2014 the label reads CLIP once a sample has clipped.\nClick to reset the peak holds and the clip counter.")}
          >
            <span className="wm-k" ref={tpKeyEl}>{t("TP")}</span>
            <span className="wm-v num" ref={tpEl}>
              {t("\u2014")}
            </span>
          </button>
          <div className="wm-cell">
            <span className="wm-k" title={t("Loudness range, LU")}>{t("LRA")}</span>
            <span className="wm-v num" ref={lraEl}>
              {t("\u2014")}
            </span>
          </div>
        </div>

        {/* The masks carry the slot's own layout class as well as `masked-panel`:
            in the narrow window this cluster is a flex strip (`app.css`, "Narrow
            windows"), and a mask with no slot class collapsed to zero width —
            which is a leak in itself, since the strip then looked different
            during a blind test than outside one. */}
        {t(blindActive ? (
          <div className="masked-panel wm-lu" style={{ height: LU_BAR_H }} />
        ) : (
          <LoudnessBar />
        ))}

        {t(blindActive ? (
          <div className="masked-panel wm-corr" style={{ height: 14 }} />
        ) : (
          <Correlation />
        ))}

        {t(blindActive && (
          <p className="wm-note">{t("Read-outs are hidden while a blind test runs: 0.4 LUFS is visible long before it is audible.")}</p>
        ))}

        <div className="wm-foot">
          <span className="num" ref={rateEl}>
            {t("\u2014")}
          </span>
          <span className="wm-trans" ref={transEl} data-ok="false">
            {t("\u2014")}
          </span>
        </div>
      </div>

      {t(blindActive ? <div className="masked-panel wm-level" /> : <LevelMeter />)}
    </aside>
  );
}
