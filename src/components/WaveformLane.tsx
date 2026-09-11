import { t } from "../lib/i18n";
import { useEffect, useMemo, useRef } from "react";
import { alignRef, formatOffsetFrames, formatOffsetMs } from "../lib/align";
import { beginPaint, useSurface } from "../lib/canvas";
import { useFrameEffect } from "../lib/frame";
import { formatLufs, formatSignedDb, formatTimeFine } from "../lib/format";
import type { DeckWaveform } from "../lib/store";
import { paint, type PaintToken } from "../lib/theme";
import type { Deck, DeckState } from "../lib/types";
import { barGeometry, waveRamps } from '../lib/waveStyle';

/** Shared, mutable interaction state. Lives in a ref so pointer motion and the
 *  60 Hz playhead never re-render React. */
export interface LaneInteraction {
  hoverRatio: number | null;
  hoverDeck: Deck | null;
  dragLoop: [number, number] | null;
}

interface Props {
  deck: Deck;
  state: DeckState | null;
  waveform: DeckWaveform;
  /** deck currently audible */
  audible: boolean;
  /** blind mode: never reveal which deck this lane is */
  masked?: boolean;
  abEnabled: boolean;
  /** deck duration / shared timeline duration, 0..1 */
  spanRatio: number;
  /** shared timeline length in seconds (deck A is the reference, SPEC §11) */
  timelineSecs: number;
  /** lane B slides with the A/B offset; lane A never does */
  offsetAware?: boolean;
  /** a track is being dragged over this lane and would land on this deck */
  dropTarget?: boolean;
  interaction: { current: LaneInteraction };
  onPointerDown: (e: React.PointerEvent<HTMLDivElement>, deck: Deck) => void;
  onPointerMove: (e: React.PointerEvent<HTMLDivElement>, deck: Deck) => void;
  onPointerLeave: () => void;
  /* Assignment by drag (SPEC §2.8). Optional: the masked lane of a blind test
     is not a deck as far as the user is allowed to know, so it gets none. */
  onLaneDragOver?: (e: React.DragEvent<HTMLDivElement>, deck: Deck) => void;
  onLaneDragLeave?: (e: React.DragEvent<HTMLDivElement>, deck: Deck) => void;
  onLaneDrop?: (e: React.DragEvent<HTMLDivElement>, deck: Deck) => void;
}

/**
 * One entry per drawn bar, reduced from the engine's buckets. `step` and `barW`
 * are CSS px but were chosen on the device grid, so the bars stay crisp at any
 * devicePixelRatio instead of landing on half pixels and blurring.
 */
interface Columns {
  /** bar pitch (bar + gap) in CSS px */
  step: number;
  /** bar width in CSS px */
  barW: number;
  /** bars the deck's own material spans (the rest of the lane is other decks) */
  span: number;
  /** bars actually filled from decoded buckets; the remainder is skeleton */
  filled: number;
  peak: Float32Array;
  rms: Float32Array;
}

/**
 * Bar pitch, in CSS px, before snapping, and the share of it the bar itself
 * gets. At 5 / 0.6 the lane reads as discrete bars rather than as a hatched
 * envelope: ~3 px of bar and ~2 px of gap at both 1× and 2×. The count follows
 * the lane width — a fixed count stretched into wide windows and turned into a
 * picket fence in narrow ones.
 *
 * Both come from the token layer (`--wf-bar-step`, `--wf-bar-duty`) so a theme
 * document can change the *shape* of the waveform and not only its colour
 * (SPEC §20). These constants are the fallback for a webview whose token layer
 * has not loaded yet; the token values are clamped on the way in twice — once
 * by the theme validator and once here, because this one is load-bearing for
 * the reduction loop below.
 */

/**
 * Which colour a lane is drawn in. A token rather than a literal: deck A *is*
 * the accent, so a custom accent moves the waveform with it, and the light
 * theme swaps in its own bronze and steel (SPEC §14).
 */
const LANE_TOKEN: Record<Deck, PaintToken> = {
  a: "--lane-a-rgb",
  b: "--lane-b-rgb",
};
const MASKED_TOKEN: PaintToken = "--lane-masked-rgb";

/**
 * The bar layers are one ink level per theme (`--wf-outer-a`, `--wf-core-a`)
 * with the *travelling* gradient kept as ratios of it. The shape of the
 * gradient — brighter at 30 %, easing back by two thirds, bright again at the
 * end — is design, not colour, so it belongs here rather than in the token
 * layer; how much ink the lane gets in total is theming, so that does not.
 */

function skeletonHeight(i: number): number {
  const x = Math.sin(i * 12.9898) * 43758.5453;
  const r = x - Math.floor(x);
  return 0.1 + 0.24 * r;
}

export default function WaveformLane({
  deck,
  state,
  waveform,
  audible,
  masked = false,
  abEnabled,
  spanRatio,
  timelineSecs,
  offsetAware = false,
  dropTarget = false,
  interaction,
  onPointerDown,
  onPointerMove,
  onPointerLeave,
  onLaneDragOver,
  onLaneDragLeave,
  onLaneDrop,
}: Props) {
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const baseRef = useRef<HTMLCanvasElement | null>(null);
  const overRef = useRef<HTMLCanvasElement | null>(null);
  const colsRef = useRef<Columns | null>(null);
  /** pixels the material is shifted by; kept so the static layer can be lazy */
  const shiftRef = useRef(0);

  // a plain string, so the memos below it stay stable across renders — a fresh
  // array literal here used to tear down and rebuild the ResizeObserver on
  // every render
  const lane: PaintToken = masked ? MASKED_TOKEN : LANE_TOKEN[deck];
  const data = waveform.data;
  const version = waveform.version;

  const info = state?.info ?? null;
  const name = info?.title ?? info?.fileName ?? state?.error ?? "empty";
  const lufs = state?.analysis?.integratedLufs ?? null;

  /* ── bucket → bar reduction (only when data, width or dpr changes) ─────── */
  const rebuildColumns = useMemo(() => {
    // `version` is in the dependency list, not the body: a new chunk of buckets
    // for the same `data` object has to invalidate this reduction too.
    void version;
    return (width: number, dpr: number, ratio: number): Columns | null => {
      if (!data || data.expected <= 0 || width <= 0) return null;
      const { step, barW } = barGeometry(dpr, paint());
      const bars = Math.max(1, Math.floor(width / step));
      const span = Math.max(1, Math.round(bars * ratio));
      const peak = new Float32Array(span);
      const rms = new Float32Array(span);
      const per = data.expected / span;
      const count = Math.min(data.count, data.max.length);
      let filled = 0;
      for (let x = 0; x < span; x += 1) {
        // A short track has fewer buckets than bars, a long one has many more;
        // `per` can be either side of 1, so the window is always at least one
        // bucket wide and never skips one.
        const from = Math.floor(x * per);
        const to = Math.max(from + 1, Math.floor((x + 1) * per));
        if (from >= count) break;
        let hi = 0;
        let r = 0;
        const end = Math.min(to, count);
        for (let i = from; i < end; i += 1) {
          // bars are symmetric, so the envelope is the larger of the two sides
          const a = Math.max(Math.abs(data.min[i]), Math.abs(data.max[i]));
          if (a > hi) hi = a;
          const rv = data.rms[i];
          if (rv > r) r = rv;
        }
        peak[x] = hi;
        rms[x] = r;
        filled = x + 1;
      }
      return { step, barW, span, filled, peak, rms };
    };
  }, [data, version]);

  /* ── static layer: the bars ──────────────────────────────────────────── */
  const drawBase = useMemo(() => {
    return (): void => {
      const { w, h, dpr } = surface.current;
      const ctx = beginPaint(baseRef.current, surface.current);
      if (!ctx) return;
      // Resolved here, not at module scope: this layer is repainted on a theme
      // change (`useSurface`'s `onResize`) and must pick up the new palette.
      const p = paint();

      const mid = Math.round(h / 2) + 0.5;
      const half = h / 2 - 2;
      const cols = colsRef.current;
      const geom = cols ?? barGeometry(dpr, p);
      // Snap the A/B shift to the device grid as well: an unsnapped translate
      // put every bar on a half pixel and softened the whole lane (SPEC §11).
      const shift = Math.round(shiftRef.current * dpr) / dpr;
      /** one device pixel, the smallest visible bar */
      const px = 1 / dpr;
      /** whole-device-pixel x for bar `i` */
      const barX = (i: number): number => Math.round(i * geom.step * dpr) / dpr;

      // centre hairline, always full width so an empty lane still reads as a lane
      ctx.strokeStyle = p.color("--wf-mid");
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(0, mid);
      ctx.lineTo(w, mid);
      ctx.stroke();

      const bars = Math.max(1, Math.floor(w / geom.step));
      const span = cols?.span ?? bars;
      const filled = cols?.filled ?? 0;

      ctx.save();
      ctx.translate(-shift, 0);

      // skeleton for the not-yet-decoded remainder
      ctx.fillStyle = p.color("--wf-skeleton");
      for (let i = filled; i < span; i += 1) {
        const a = Math.max(px, skeletonHeight(i) * half);
        ctx.fillRect(barX(i), mid - a, geom.barW, a * 2);
      }

      if (cols && filled > 0) {
        const lit = Math.max(1, barX(filled - 1) + geom.barW);

        /** the lane's colour at an ink level, in whichever theme is on */
        const ink = (a: number): string => p.tint(lane, a);
        const { outer, core } = waveRamps(ctx, p, lane, lit);

        ctx.fillStyle = outer;
        for (let i = 0; i < filled; i += 1) {
          const a = Math.max(px, cols.peak[i] * half);
          ctx.fillRect(barX(i), mid - a, geom.barW, a * 2);
        }

        ctx.fillStyle = core;
        for (let i = 0; i < filled; i += 1) {
          const a = Math.max(px, cols.rms[i] * half * 0.98);
          ctx.fillRect(barX(i), mid - a, geom.barW, a * 2);
        }

        // Caps: a single lit device pixel at each peak, added over the top. It
        // is what keeps the transients legible once the bars are thin.
        ctx.globalCompositeOperation = p.blend();
        ctx.fillStyle = ink(p.num("--wf-cap-a"));
        for (let i = 0; i < filled; i += 1) {
          const a = cols.peak[i] * half;
          if (a < 2.5) continue;
          const x = barX(i);
          ctx.fillRect(x, mid - a, geom.barW, px);
          ctx.fillRect(x, mid + a - px, geom.barW, px);
        }
        ctx.globalCompositeOperation = "source-over";
      }

      ctx.restore();

      /* Regions the offset has pushed material out of are drawn flat and
         dimmed, never blank: the point is to make it obvious *why* deck B is
         silent there (SPEC §11). */
      if (shift !== 0) {
        const end = barX(span - 1) + geom.barW;
        const material: [number, number] = [-shift, end - shift];
        const gaps: Array<[number, number]> = [];
        if (material[0] > 0) gaps.push([0, Math.min(w, material[0])]);
        if (material[1] < w) gaps.push([Math.max(0, material[1]), w]);
        for (const [x0, x1] of gaps) {
          if (x1 - x0 <= 0.5) continue;
          ctx.fillStyle = p.color("--wf-gap-fill");
          ctx.fillRect(x0, 2, x1 - x0, h - 4);
          ctx.strokeStyle = p.color("--wf-gap-line");
          ctx.setLineDash([2, 3]);
          ctx.lineWidth = 1;
          ctx.beginPath();
          ctx.moveTo(x0, mid);
          ctx.lineTo(x1, mid);
          ctx.stroke();
          ctx.setLineDash([]);
          if (x1 - x0 > 70) {
            ctx.font = p.font(9);
            ctx.fillStyle = p.color("--wf-gap-text");
            ctx.textAlign = "center";
            ctx.textBaseline = "middle";
            ctx.fillText("no material", (x0 + x1) / 2, mid - 10);
          }
        }
      }
    };
  }, [lane]);

  const surface = useSurface(baseRef, {
    measure: wrapRef,
    extra: [overRef],
    onResize: () => {
      const { w, dpr } = surface.current;
      colsRef.current = rebuildColumns(w, dpr, spanRatio);
      drawBase();
    },
  });

  /* ── playhead layer, rAF only ────────────────────────────────────────── */
  useFrameEffect((frame, time) => {
    const { w, h } = surface.current;
    if (w <= 0 || h <= 0) return;
    const p = paint();
    const ink = (a: number): string => p.tint(lane, a);

    const t = frame?.transport;
    const duration = Math.max(0.001, timelineSecs || t?.durationSecs || 1);

    // keep the static layer in step with the A/B offset
    if (offsetAware) {
      const rate = t?.engineSampleRate ?? 48000;
      const shift = (alignRef.frames / Math.max(1, rate) / duration) * w;
      if (Math.abs(shift - shiftRef.current) > 0.25) {
        shiftRef.current = shift;
        drawBase();
      }
    } else if (shiftRef.current !== 0) {
      shiftRef.current = 0;
      drawBase();
    }

    const ctx = beginPaint(overRef.current, surface.current);
    if (!ctx) return;

    const pos = Math.max(0, Math.min(duration, t?.positionSecs ?? 0));
    const px = (pos / duration) * w;

    /* The frame is the authority; the 10 Hz snapshot is the fallback for the
       moment before the first frame arrives. `state` is read straight off this
       render — it is a fresh object on every snapshot, and the `stateRef` mirror
       that used to be here existed only to keep it out of a dependency array
       `useFrameEffect` no longer has. */
    const decodedFraction = frame
      ? (deck === "a" ? frame.deckA : frame.deckB).decodedFraction
      : state?.decodedFraction ?? 0;
    const decodeX = decodedFraction * w;

    // Un-played scrim. Heavier than it was for the old filled envelope: bars
    // carry less ink per column, so a light scrim left the played and unplayed
    // halves of the lane nearly indistinguishable.
    ctx.fillStyle = p.color("--wf-scrim");
    ctx.fillRect(px, 0, Math.max(0, w - px), h);

    // decode edge
    if (decodedFraction < 0.999 && decodeX > 1) {
      const edge = p.num("--wf-decode-a");
      const eg = ctx.createLinearGradient(decodeX - 26, 0, decodeX, 0);
      eg.addColorStop(0, ink(0));
      eg.addColorStop(1, ink(edge * 0.47));
      ctx.fillStyle = eg;
      ctx.fillRect(decodeX - 26, 0, 26, h);
      ctx.fillStyle = ink(edge);
      ctx.fillRect(decodeX, 0, 1, h);
    }

    // loop region
    const drag = interaction.current.dragLoop;
    const region = drag ?? t?.loopRegion ?? null;
    if (region) {
      const x0 = (Math.min(region[0], region[1]) / duration) * w;
      const x1 = (Math.max(region[0], region[1]) / duration) * w;
      ctx.fillStyle = p.color("--loop-fill");
      ctx.fillRect(x0, 0, x1 - x0, h);
      ctx.fillStyle = p.color("--loop-edge");
      ctx.fillRect(x0, 0, 1, h);
      ctx.fillRect(x1 - 1, 0, 1, h);
      ctx.fillStyle = p.color("--loop-grip");
      ctx.fillRect(x0, 0, 6, 2);
      ctx.fillRect(x1 - 6, h - 2, 6, 2);
    }

    // hover hairline + read-out, drawn on the lane under the pointer
    const hover = interaction.current.hoverRatio;
    if (hover != null) {
      const hxh = Math.round(hover * w) + 0.5;
      ctx.strokeStyle = p.color("--wf-hover-line");
      ctx.lineWidth = 1;
      ctx.setLineDash([2, 3]);
      ctx.beginPath();
      ctx.moveTo(hxh, 0);
      ctx.lineTo(hxh, h);
      ctx.stroke();
      ctx.setLineDash([]);

      if (interaction.current.hoverDeck === deck) {
        const label = drag
          ? `LOOP  ${formatTimeFine(Math.min(drag[0], drag[1]))} \u2192 ${formatTimeFine(
              Math.max(drag[0], drag[1]),
            )}`
          : formatTimeFine(hover * duration);
        ctx.font = p.font(10);
        const tw = ctx.measureText(label).width;
        const bw = tw + 12;
        const bx = Math.max(2, Math.min(w - bw - 2, hxh - bw / 2));
        ctx.fillStyle = p.color("--tip-bg");
        ctx.fillRect(bx, 4, bw, 17);
        ctx.strokeStyle = p.color("--tip-line");
        ctx.strokeRect(bx + 0.5, 4.5, bw - 1, 16);
        ctx.fillStyle = p.color("--tip-text");
        ctx.textAlign = "left";
        ctx.textBaseline = "middle";
        ctx.fillText(label, bx + 6, 13.5);
      }
    }

    // Alt-drag alignment: a live read-out following the cursor
    if (offsetAware && alignRef.dragging && alignRef.dragX != null) {
      const rate = t?.engineSampleRate ?? 48000;
      const label = `OFFSET  ${formatOffsetMs(alignRef.frames, rate)}  \u00B7  ${formatOffsetFrames(
        alignRef.frames,
      )}`;
      ctx.font = p.font(10);
      const tw = ctx.measureText(label).width;
      const bw = tw + 16;
      const bx = Math.max(2, Math.min(w - bw - 2, alignRef.dragX - bw / 2));
      ctx.fillStyle = p.color("--align-chip");
      ctx.fillRect(bx, h - 24, bw, 18);
      ctx.fillStyle = p.color("--align-chip-ink");
      ctx.textAlign = "left";
      ctx.textBaseline = "middle";
      ctx.fillText(label, bx + 8, h - 15);
      ctx.strokeStyle = p.color("--align-line");
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(Math.round(alignRef.dragX) + 0.5, 0);
      ctx.lineTo(Math.round(alignRef.dragX) + 0.5, h);
      ctx.stroke();
    }

    // audio-reactive shimmer travelling with the playhead
    if (audible && t?.playing) {
      const pulse = 0.5 + 0.5 * Math.sin(time / 520);
      const glow = p.num("--wf-glow-a");
      const g = ctx.createRadialGradient(px, h / 2, 0, px, h / 2, 54 + 12 * pulse);
      g.addColorStop(0, ink(glow * (1 + 0.36 * pulse)));
      g.addColorStop(1, ink(0));
      ctx.globalCompositeOperation = p.blend();
      ctx.fillStyle = g;
      ctx.fillRect(px - 70, 0, 140, h);
      ctx.globalCompositeOperation = "source-over";
    }

    // hairline playhead
    const hx = Math.round(px) + 0.5;
    const head = p.num("--wf-playhead-a");
    ctx.strokeStyle = audible ? ink(head) : p.color("--wf-playhead-idle");
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(hx, 0);
    ctx.lineTo(hx, h);
    ctx.stroke();
    if (audible) {
      ctx.fillStyle = ink(head);
      ctx.beginPath();
      ctx.moveTo(hx - 3.5, 0);
      ctx.lineTo(hx + 3.5, 0);
      ctx.lineTo(hx, 5);
      ctx.closePath();
      ctx.fill();
    }
  });

  /* ── redraw the static layer when new buckets land ───────────────────── */
  useEffect(() => {
    const { w, dpr } = surface.current;
    if (w <= 0) return;
    colsRef.current = rebuildColumns(w, dpr, spanRatio);
    drawBase();
  }, [drawBase, rebuildColumns, spanRatio, surface]);

  const loaded = state?.loaded ?? false;

  const deckName = deck.toUpperCase();

  return (
    <div
      className="wave-lane"
      data-deck={deck}
      data-active={audible}
      data-masked={masked}
      data-empty={!masked && !loaded}
      data-drop-target={dropTarget}
      onDragOver={onLaneDragOver && ((e) => onLaneDragOver(e, deck))}
      onDragLeave={onLaneDragLeave && ((e) => onLaneDragLeave(e, deck))}
      onDrop={onLaneDrop && ((e) => onLaneDrop(e, deck))}
    >
      <div className="lane-head">
        <i className="deck-badge" data-deck={deck} data-ghost={masked || !loaded}>
          {t(masked ? "?" : deck.toUpperCase())}
        </i>
        {t(masked ? (
          <span
            className="lane-name lane-hidden"
            title={t("A fixed reference drawing. It does not follow the audible slot \u2014 if it did, switching slots would show you the answer.")}
          >
            {t("hidden slot \u00B7 reference view")}
          </span>
        ) : (
          <>
            <span className="lane-name">{loaded ? name : "\u2014"}</span>
            {info?.artist && <span className="lane-artist">{info.artist}</span>}
          </>
        ))}
        <span className="lane-spacer" />
        {t(!masked && state?.invert && (
          <span className="lane-meta lane-invert num" title={t("Polarity inverted")}>
            {t("\u00F8")}{t("inverted")}</span>
        ))}
        {/* Which deck you are *hearing*, said in words on both lanes rather
            than implied by one dimmed lane. `A` / `B` only move this label:
            they never assign, and a reader who mistakes the two thinks
            assignment is broken (SPEC §2.8). */}
        {t(!masked && abEnabled && (
          <span className={audible ? "lane-audible" : "lane-silent"}>
            {t(audible ? "audible" : "silent")}
          </span>
        ))}
        {t(!masked && lufs != null && (
          <span className="lane-meta num">
            <em>{t("I")}</em>
            {t(formatLufs(lufs))}{t("LUFS")}</span>
        ))}
        {t(!masked && state && Math.abs(state.trimDb) > 0.049 && (
          <span className="lane-meta lane-trim num">
            <em>{t("trim")}</em>
            {t(formatSignedDb(state.trimDb))}{t("dB")}</span>
        ))}
      </div>
      <div
        className="lane-canvas"
        ref={wrapRef}
        onPointerDown={(e) => onPointerDown(e, deck)}
        onPointerMove={(e) => onPointerMove(e, deck)}
        onPointerLeave={onPointerLeave}
      >
        <canvas ref={baseRef} />
        <canvas ref={overRef} />
        {/* Named while the drag is still in the air: a highlight alone says
            "something will happen here", not "this becomes deck B". */}
        {t(!masked && (
          <div className="lane-drop-hint" aria-hidden={!dropTarget}>
            {t(`assign to deck ${deckName}`)}
          </div>
        ))}
        {/* An empty lane used to be a blank rectangle with no explanation —
            the single most confusing thing about A/B, because it looks like a
            deck that refuses to load. It now says how to fill itself, and
            names all three routes that reach this deck (SPEC §2.8). */}
        {t(!masked && !loaded && (
          <div className="lane-empty">
            <strong>{t(`Deck ${deckName} is empty`)}</strong>
            <span>
              {t(`Drag a track here \u00B7 tap `)}
              <em>{t(deckName)}</em>
              {t(` on a playlist row \u00B7 `)}
              <em>{t(`\u21E7${deckName}`)}</em>
              {t(" assigns the selected row")}
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}
