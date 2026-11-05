/** Mock-only canvas geometry and subscription diagnostics for UI tests.
 * Production builds omit this module and its window globals.
 */
import { paintCount, paintedSurface, type Surface } from "./canvas";
import { frameAttachments, frameSubscribers } from "./frame";

/** One canvas, as the DOM has it and as its painter believes it to be. */
export interface CanvasReport {
  /** where the canvas is, in words: `wave-lane b · lane-canvas[1]` */
  key: string;
  /** the CSS box, rounded to whole px */
  css: { w: number; h: number };
  /** the backing store, which `useSurface` sizes */
  backing: { w: number; h: number };
  /** the surface the last paint used, or `null` if it has never been painted */
  painted: Surface | null;
  /** paints since the canvas was first drawn — a still number is a dead surface */
  paints: number;
}

export interface DiagnosticsReport {
  dpr: number;
  frame: {
    /** painters attached to the rAF loop right now: one per mounted surface */
    subscribers: number;
    /** attachments since the window loaded: one per mount, and no more */
    attachments: number;
  };
  canvases: CanvasReport[];
}

/** A name a failure message can be read out loud from. */
function describe(canvas: HTMLCanvasElement): string {
  const parent = canvas.parentElement;
  const siblings = parent ? [...parent.querySelectorAll("canvas")] : [canvas];
  const deck = canvas.closest<HTMLElement>("[data-deck]")?.dataset.deck;
  const where = canvas.closest<HTMLElement>("[class]")?.className.split(/\s+/)[0] ?? "canvas";
  const index = siblings.length > 1 ? `[${siblings.indexOf(canvas)}]` : "";
  return `${where}${deck ? ` ${deck}` : ""}${index}`;
}

/** Every canvas in this document, with what its painter last did to it. */
export function canvasReports(): CanvasReport[] {
  return [...document.querySelectorAll("canvas")].map((canvas) => {
    const box = canvas.getBoundingClientRect();
    return {
      key: describe(canvas),
      css: { w: Math.round(box.width), h: Math.round(box.height) },
      backing: { w: canvas.width, h: canvas.height },
      painted: paintedSurface(canvas),
      paints: paintCount(canvas),
    };
  });
}

export function report(): DiagnosticsReport {
  return {
    dpr: window.devicePixelRatio || 1,
    frame: { subscribers: frameSubscribers(), attachments: frameAttachments() },
    canvases: canvasReports(),
  };
}

/**
 * Publish the report on `window.__onyxDiag`.
 *
 * A function rather than a snapshot object: the harness reads it after a resize
 * and after a track change, and what it needs is the answer *then*.
 */
export function installDiagnosticsBridge(): void {
  (window as unknown as Record<string, unknown>).__onyxDiag = { report, canvasReports };
}
