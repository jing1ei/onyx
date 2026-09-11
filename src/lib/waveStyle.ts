import type { Paint, PaintToken } from './theme';

/** Shared visual language for Audition and Edit, snapped to device pixels. */
export function barGeometry(dpr: number, p: Paint) {
  const stepDev = Math.max(2, Math.round(Math.min(40, Math.max(2, p.num('--wf-bar-step') || 5)) * dpr));
  const duty = Math.min(1, Math.max(0.15, p.num('--wf-bar-duty') || 0.6));
  return { step: stepDev / dpr, barW: Math.max(1, Math.min(stepDev - 1, Math.round(stepDev * duty))) / dpr };
}

export function waveRamps(ctx: CanvasRenderingContext2D, p: Paint, lane: PaintToken, width: number) {
  const ramp = (level: number, shape: number[]) => {
    const g = ctx.createLinearGradient(0, 0, Math.max(1, width), 0);
    const stops = [0, 0.3, 0.66, 1];
    shape.forEach((k, i) => g.addColorStop(stops[i], p.tint(lane, level * k)));
    return g;
  };
  return {
    outer: ramp(p.num('--wf-outer-a'), [0.652, 1, 0.739, 1]),
    core: ramp(p.num('--wf-core-a'), [0.75, 1, 0.833, 1]),
  };
}
