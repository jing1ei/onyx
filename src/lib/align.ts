/**
 * A/B time alignment state (SPEC.md §11).
 *
 * The offset is dragged at pointer rate and read by the waveform painters at
 * 60 Hz, so — like the audition sweep — it lives in a module ref rather than in
 * React state. `frames` is optimistic: it moves immediately, IPC is coalesced
 * to one call per animation frame **and to one call at a time**, and a rejection
 * rolls back to the last value the engine confirmed instead of leaving the UI
 * lying about the offset. See `inflight` for why one frame's worth of
 * coalescing is not enough on its own.
 *
 * `scripts/check-ab-parity.mjs` drives a drag through this module against a
 * recording backend and asserts what that adds up to: writes are ordered, never
 * overlap, and the last one carries the value the drag ended on.
 */

import * as api from "./api";
import { useStore } from "./store";

/** ±30 s at 192 kHz is the widest the engine will accept (SPEC §11). */
const MAX_OFFSET_SECS = 30;

interface AlignRef {
  /** what the UI is currently showing */
  frames: number;
  /** last value the engine acknowledged */
  confirmed: number;
  /** live ms read-out follows the cursor while Alt-dragging lane B */
  dragX: number | null;
  /** true while a pointer gesture owns the value */
  dragging: boolean;
}

export const alignRef: AlignRef = { frames: 0, confirmed: 0, dragX: null, dragging: false };

let pending: number | null = null;
/** The queued animation frame, `0` when none is queued (rAF handles are ≥ 1). */
let scheduled = 0;
/**
 * A write is on the wire. Snapshots that arrive in this window still carry the
 * pre-write offset, and adopting one snapped the waveform back under the
 * cursor and then desynced the UI from the engine when the ack landed.
 *
 * It is also what keeps two `set_ab_offset` calls from overlapping. Coalescing
 * to one write per animation frame is not enough on its own: a machine slow
 * enough that a write takes longer than a frame — every machine, once the
 * engine is re-decoding a deck — would otherwise have a second frame's write on
 * the wire before the first was acked, and the engine may ack them in either
 * order. `confirmed` would then hold whichever landed last rather than the last
 * offset asked for, so a rejection would roll the lane back to a superseded
 * value. One write at a time, and the newest pending value goes out next.
 */
let inflight = 0;

function flush(): void {
  scheduled = 0;
  if (pending == null) return;
  // one write at a time: concurrent `set_ab_offset` calls can be acked out of
  // order, which would leave `confirmed` holding a superseded value
  if (inflight > 0) {
    scheduled = requestAnimationFrame(flush);
    return;
  }
  const frames = pending;
  pending = null;
  inflight += 1;
  void api
    .setAbOffset(frames)
    .then(() => {
      alignRef.confirmed = frames;
    })
    .catch((err) => {
      // Roll the optimistic value back; a silent failure here would mean the
      // waveform and the audio disagree about where deck B is. A newer value
      // queued while this one was on the wire is *not* rolled back: the drag
      // has moved on, and snapping the lane back to a superseded offset only
      // to write the newer one a frame later reads as a glitch.
      if (pending == null) alignRef.frames = alignRef.confirmed;
      useStore.getState().pushToast("error", `Alignment refused: ${api.errorMessage(err)}`);
    })
    .finally(() => {
      inflight -= 1;
    });
}

function clampFrames(frames: number, sampleRate: number): number {
  const limit = Math.round(MAX_OFFSET_SECS * Math.max(1, sampleRate));
  return Math.max(-limit, Math.min(limit, Math.round(frames)));
}

/** Optimistic set. Returns the clamped value actually applied. */
export function setOffset(frames: number, sampleRate: number): number {
  const next = clampFrames(frames, sampleRate);
  if (next === alignRef.frames && next === alignRef.confirmed) return next;
  alignRef.frames = next;
  pending = next;
  if (!scheduled) scheduled = requestAnimationFrame(flush);
  return next;
}

/** Adopt an offset the engine reported (auto-align result, reset). */
export function adoptOffset(frames: number): void {
  alignRef.frames = frames;
  alignRef.confirmed = frames;
  pending = null;
  /* Nothing left to write, so the frame this module asked for is dropped rather
     than left to wake up and find `pending` empty — and, more importantly, a
     frame still queued from a *drag* must not fire after an adopted value and
     write the offset the drag had reached. */
  if (scheduled) {
    cancelAnimationFrame(scheduled);
    scheduled = 0;
  }
}

/**
 * How many `set_ab_offset` writes are on the wire, and whether one is queued.
 * Exported for `scripts/check-ab-parity.mjs`, which drives a drag through this
 * module and asserts the writes never overlap; nothing in the app reads it.
 */
export function alignWriteState(): { inflight: number; queued: boolean } {
  return { inflight, queued: pending != null };
}

/**
 * Reconcile with an `AppSnapshot`. A snapshot that arrives mid-gesture, or
 * while a write is still in flight, is stale by definition and is ignored —
 * otherwise the offset would visibly snap backwards under the cursor.
 */
export function syncFromEngine(frames: number): void {
  if (alignRef.dragging || pending != null || inflight > 0) return;
  adoptOffset(frames);
}

/**
 * Move the offset by a step. `samples` wins over `ms` when both are given.
 * The sample rate is passed in rather than read from the frame stream so this
 * module stays free of a cycle with `frame.ts`.
 */
export function nudgeOffset(step: { ms?: number; samples?: number }, sampleRate: number): void {
  const delta = step.samples ?? framesForMs(step.ms ?? 0, sampleRate);
  setOffset(alignRef.frames + delta, sampleRate);
}

function framesForMs(ms: number, sampleRate: number): number {
  return Math.round((ms / 1000) * Math.max(1, sampleRate));
}

export function offsetMs(frames: number, sampleRate: number): number {
  return (frames / Math.max(1, sampleRate)) * 1000;
}

const MINUS = "\u2212";

/** Signed ms with 2 dp: `+12.34 ms`, `−0.02 ms`, `0.00 ms`. */
export function formatOffsetMs(frames: number, sampleRate: number): string {
  const ms = offsetMs(frames, sampleRate);
  const abs = Math.abs(ms).toFixed(2);
  if (Math.abs(ms) < 0.005) return `0.00 ms`;
  return `${ms > 0 ? "+" : MINUS}${abs} ms`;
}

/** Signed sample count: `+592 smp`. */
export function formatOffsetFrames(frames: number): string {
  if (frames === 0) return "0 smp";
  return `${frames > 0 ? "+" : MINUS}${Math.abs(frames).toLocaleString()} smp`;
}
