/**
 * Canvas plumbing, in one place.
 *
 * Every meter in Onyx paints from the shared rAF loop, so they all need the
 * same three things: a backing store sized to CSS px × devicePixelRatio, a
 * subscription that fires when either the element *or* the device pixel ratio
 * changes, and a `setTransform` + `clearRect` prologue. Before this module each
 * canvas component carried its own slightly different copy — and none of them
 * reacted to a dpr change, so dragging the window to a non-retina display left
 * every meter blurry until the next layout change.
 */

import { useEffect, useRef, type RefObject } from "react";
import { onThemeChange } from "./theme";

export interface Surface {
  /** CSS pixels */
  w: number;
  h: number;
  dpr: number;
}

/** Clamped device pixel ratio — above 3× the extra pixels buy nothing. */
function pixelRatio(): number {
  return Math.min(3, window.devicePixelRatio || 1);
}

/** Size a canvas backing store. Returns true when it actually changed. */
function sizeCanvas(canvas: HTMLCanvasElement | null, s: Surface): boolean {
  if (!canvas) return false;
  const w = Math.max(1, Math.round(s.w * s.dpr));
  const h = Math.max(1, Math.round(s.h * s.dpr));
  if (canvas.width === w && canvas.height === h) return false;
  canvas.width = w;
  canvas.height = h;
  return true;
}

/**
 * What each canvas was last painted with — the geometry the painter *believed*
 * it had, as opposed to `canvas.width`, which is set by the resize path above,
 * and how many times it has been painted at all.
 *
 * The two geometries disagree exactly when a painter is a closure over a stale
 * render, and a paint count that stops moving is a surface that has silently
 * stopped drawing; neither is visible in the DOM and both are only visible in
 * pixels. So they are recorded: one `WeakMap` lookup per paint, no allocation
 * after the first, no DOM writes, and nothing retained once a canvas is
 * collected. `frame.ts`'s `frameAttachments` is the other half, and `lib/diag.ts`
 * publishes all three in the mock preview (`src/main.tsx`, `src/eq/main.tsx`) so
 * the harness can assert the state rather than photograph it and hope.
 */
interface PaintTrace {
  surface: Surface;
  paints: number;
}

const traces = new WeakMap<HTMLCanvasElement, PaintTrace>();

/** The surface `canvas` was last painted with, if it has been painted. */
export const paintedSurface = (canvas: HTMLCanvasElement): Surface | null =>
  traces.get(canvas)?.surface ?? null;

/** How many times `canvas` has been painted since it was created. */
export const paintCount = (canvas: HTMLCanvasElement): number => traces.get(canvas)?.paints ?? 0;

/** `setTransform` + `clearRect`: the two lines every painter starts with. */
export function beginPaint(
  canvas: HTMLCanvasElement | null,
  s: Surface,
): CanvasRenderingContext2D | null {
  if (!canvas || s.w <= 0 || s.h <= 0) return null;
  const ctx = canvas.getContext("2d");
  if (!ctx) return null;
  const trace = traces.get(canvas);
  if (trace) {
    trace.surface = s;
    trace.paints += 1;
  } else {
    traces.set(canvas, { surface: s, paints: 1 });
  }
  ctx.setTransform(s.dpr, 0, 0, s.dpr, 0, 0);
  ctx.clearRect(0, 0, s.w, s.h);
  return ctx;
}

/**
 * Element size *and* devicePixelRatio changes in one subscription.
 * `matchMedia("(resolution: Ndppx)")` fires when the window moves to a display
 * with a different scale factor, which ResizeObserver never sees.
 */
function observeSurface(el: Element, onChange: () => void): () => void {
  const ro = new ResizeObserver(onChange);
  ro.observe(el);

  let mql: MediaQueryList | null = null;
  const onDpr = (): void => {
    watchDpr();
    onChange();
  };
  const watchDpr = (): void => {
    mql?.removeEventListener("change", onDpr);
    mql = window.matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`);
    mql.addEventListener("change", onDpr);
  };
  watchDpr();

  return () => {
    ro.disconnect();
    mql?.removeEventListener("change", onDpr);
    mql = null;
  };
}

interface SurfaceOptions {
  /** element whose client box defines the CSS size; defaults to the canvas */
  measure?: RefObject<HTMLElement | null>;
  /** fixed CSS height in px; when omitted the measured element's height wins */
  height?: number;
  /** extra canvases layered over the same box (waveform: base + playhead) */
  extra?: Array<RefObject<HTMLCanvasElement | null>>;
  /**
   * Called after a resize so static layers can repaint — and, for the same
   * reason, after a theme change (SPEC §14). A layer that is not redrawn by
   * the 60 Hz loop keeps the palette it was painted with, so switching to the
   * light theme would otherwise leave the waveform bars champagne-on-alabaster
   * until the next resize. Repainting on both events costs one frame at a
   * moment the user is already expecting the window to change.
   */
  onResize?: () => void;
}

/**
 * Keep one or more canvases correctly sized. Returns a ref holding the current
 * CSS size + dpr, read by painters inside the rAF loop (never by React).
 */
export function useSurface(
  canvas: RefObject<HTMLCanvasElement | null>,
  options: SurfaceOptions = {},
): RefObject<Surface> {
  const { measure, height, extra, onResize } = options;
  const surface = useRef<Surface>({ w: 0, h: height ?? 0, dpr: 1 });
  // read through refs so the effect never re-subscribes on every render
  const latest = useRef({ extra, onResize });
  latest.current = { extra, onResize };

  useEffect(() => {
    const target = measure?.current ?? canvas.current;
    if (!target) return;

    const apply = (): void => {
      const dpr = pixelRatio();
      const w = target.clientWidth;
      const h = height ?? target.clientHeight;
      if (w <= 0 || h <= 0) return;
      surface.current = { w, h, dpr };
      const all = [canvas, ...(latest.current.extra ?? [])];
      for (const ref of all) {
        const c = ref.current;
        if (!c) continue;
        sizeCanvas(c, surface.current);
        if (height != null && c.style.height !== `${height}px`) c.style.height = `${height}px`;
      }
      latest.current.onResize?.();
    };

    apply();
    const stopSurface = observeSurface(target, apply);
    const stopTheme = onThemeChange(() => latest.current.onResize?.());
    return () => {
      stopSurface();
      stopTheme();
    };
  }, [canvas, measure, height]);

  return surface;
}
