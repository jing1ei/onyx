/**
 * The 60 Hz bridge.
 *
 * `onyx://frame` arrives sixty times a second. Pushing that into React state
 * would re-render the whole tree sixty times a second, so the payload is
 * written into a module-level ref and *nothing else happens*. Canvas
 * components read `frameRef.current` from inside a single shared
 * requestAnimationFrame loop; React never learns that a frame arrived.
 *
 * `onyx://state` (low frequency) and `onyx://toast` do go through the store.
 *
 * Both windows run this bridge. Rust emits app-wide, so the detached EQ window
 * (SPEC §12) gets the same 60 Hz stream as the main one without a second pump —
 * but it has no waveform lanes, so it starts the bridge with `waveforms: false`
 * and never asks for a `waveform_get` chunk it would immediately throw away.
 */

import { useEffect, useRef } from "react";
import * as api from "./api";
import { syncFromEngine } from "./align";
import { logError } from "./log";
import { onSnapshot, useStore } from "./store";
import type { AppSnapshot, Deck, FramePayload, ToastPayload } from "./types";

/** Newest frame. Mutable on purpose — read it, never store it in React state. */
export const frameRef: { current: FramePayload | null } = { current: null };

export type FrameTick = (frame: FramePayload | null, timeMs: number, dtMs: number) => void;

const subscribers = new Set<FrameTick>();
/** every attach since the module loaded — see `frameAttachments` */
let attachments = 0;
let rafId = 0;
let lastTime = 0;
/** a throwing painter throws 60 times a second; report the first one only */
let paintErrorLogged = false;

function loop(time: number): void {
  const dt = lastTime ? time - lastTime : 16.7;
  lastTime = time;
  const frame = frameRef.current;
  for (const cb of subscribers) {
    try {
      cb(frame, time, dt);
    } catch (err) {
      // One bad canvas must not stop the others, but a painter that throws
      // every frame is a real defect and used to leave no trace at all.
      if (!paintErrorLogged) {
        paintErrorLogged = true;
        logError("a frame painter threw; that surface will not update", err);
      }
    }
  }
  rafId = subscribers.size > 0 ? requestAnimationFrame(loop) : 0;
}

/** Register a per-animation-frame painter. Returns an unsubscribe function. */
export function subscribeFrame(cb: FrameTick): () => void {
  subscribers.add(cb);
  attachments += 1;
  if (!rafId) {
    lastTime = 0;
    rafId = requestAnimationFrame(loop);
  }
  return () => {
    subscribers.delete(cb);
    if (subscribers.size === 0 && rafId) {
      cancelAnimationFrame(rafId);
      rafId = 0;
    }
  };
}

/**
 * How many painters are attached to the loop.
 *
 * Diagnostics, and the two contract checks that care: a surface must attach
 * **once** per mount (a hook that resubscribed on every prop change tore the
 * rAF loop down and rebuilt it ten times a second while a track decoded), and
 * a resize or a track change must not add or drop one.
 */
export const frameSubscribers = (): number => subscribers.size;

/**
 * How many times a painter has attached since the window loaded.
 *
 * A surface subscribes once per mount, so this number stops moving as soon as
 * the app has drawn itself. If it keeps climbing while the window is resized or
 * a track is loaded, some surface is re-attaching on render — the pattern
 * `useFrameEffect` exists to remove — and the 60 Hz loop is being torn down and
 * rebuilt underneath the meters. The mock preview publishes it so the harness
 * can assert that, rather than photograph a canvas and hope.
 */
export const frameAttachments = (): number => attachments;

/**
 * Subscribe once and always call the *newest* painter in `ref.current`.
 *
 * The mechanism behind {@link useFrameEffect}, exported on its own so the
 * contract script can drive it without React: hand it a ref, swap what is in the
 * ref, and the next frame uses the new one.
 */
export function subscribeFrameLatest(ref: { current: FrameTick }): () => void {
  return subscribeFrame((frame, time, dt) => ref.current(frame, time, dt));
}

/**
 * The hook every 60 Hz surface uses. **Do not call `subscribeFrame` from a
 * component.**
 *
 * The rule this exists to enforce: a frame painter must see the props and state
 * of the render it belongs to. `useEffect(() => subscribeFrame(paint), [])`
 * does not — it captures the *first* render's `width`, `entryId`, `blindActive`
 * and paints with them forever, so a window resize or a track change leaves the
 * canvas drawing the old size and the old material. The three ways this codebase
 * had worked around that were all worse than fixing it:
 *
 *  - a hand-written dependency array (`WaveformLane`, `TransportBar`): correct
 *    only while it is exhaustive, and it tears the rAF loop down and rebuilds it
 *    on every prop change;
 *  - a hand-written "latest" mirror per value (`blindRef.current = blindActive`,
 *    written out twice in two components): correct, and one line of ceremony per
 *    value forever;
 *  - `useStore.getState()` inside the painter: correct, and it hides which state
 *    the surface actually depends on.
 *
 * `useFrameEffect` subscribes **once** per mount and re-points the subscription
 * at the current render's callback, so the painter is always fresh, the rAF loop
 * is never rebuilt, and there is no dependency array to get wrong. The 60 Hz
 * path still never touches React state — that is the whole design (SPEC §5) and
 * this hook cannot break it: it only ever *reads* what the last render produced.
 */
export function useFrameEffect(tick: FrameTick): void {
  // Assigned during render, on purpose: an effect would leave one frame's worth
  // of window in which the painter is a render behind, and `canvas.ts` keeps its
  // `onResize` callback fresh the same way.
  const latest = useRef(tick);
  latest.current = tick;
  useEffect(() => subscribeFrameLatest(latest), []);
}

/* ── waveform streaming ──────────────────────────────────────────────────── */

const inflight: Record<Deck, boolean> = { a: false, b: false };
/** consecutive `waveform_get` failures; one is normal, a run of them is not */
const failures: Record<Deck, number> = { a: 0, b: 0 };
/** the track those failures belong to, so they do not accumulate across loads */
const failedEntry: Record<Deck, number | null> = { a: null, b: null };
const FAILURE_ALARM = 5;

async function syncWaveform(deck: Deck): Promise<void> {
  if (inflight[deck]) return;
  const store = useStore.getState();
  const snap = store.snapshot;
  if (!snap) return;
  const ds = deck === "a" ? snap.deckA : snap.deckB;
  if (!ds.loaded || ds.entryId == null) return;

  const frame = frameRef.current;
  const available = frame ? (deck === "a" ? frame.deckA : frame.deckB).waveformBuckets : ds.waveformBuckets;
  const cached = store.waveforms[deck];
  const have = cached.entryId === ds.entryId ? cached.data?.count ?? 0 : 0;
  if (available <= have) return;
  // Don't hammer the backend for a handful of new buckets mid-decode.
  const complete = ds.decoded || available >= (cached.data?.expected ?? Number.MAX_SAFE_INTEGER);
  if (!complete && available - have < 16 && have > 0) return;

  inflight[deck] = true;
  try {
    const chunk = await api.waveformGet(deck, have);
    failures[deck] = 0;
    failedEntry[deck] = null;
    // The deck can be swapped while the request is in flight. Appending then
    // files the *new* track's buckets under the *old* entry id, at the old
    // offset — a visibly wrong, time-shifted waveform on that lane until the
    // next poll corrects it. Re-read the deck and drop the chunk instead.
    const now = useStore.getState().snapshot;
    const stillThere = (deck === "a" ? now?.deckA : now?.deckB)?.entryId === ds.entryId;
    if (stillThere && chunk && chunk.max.length > 0) {
      useStore.getState().appendWaveform(deck, ds.entryId, chunk, have);
    }
  } catch (err) {
    // A single failure is expected: the deck can be swapped underneath us
    // between the frame that advertised buckets and this request. A *run* of
    // failures on one track means the waveform will silently never appear, so
    // say so once — but the count has to be per track, or five ordinary deck
    // swaps over a session added up to a false alarm about a healthy deck.
    if (failedEntry[deck] !== ds.entryId) {
      failedEntry[deck] = ds.entryId;
      failures[deck] = 0;
    }
    failures[deck] += 1;
    if (failures[deck] === FAILURE_ALARM) {
      useStore
        .getState()
        .pushToast("warn", `Waveform for deck ${deck.toUpperCase()} unavailable: ${api.errorMessage(err)}`);
    }
  } finally {
    inflight[deck] = false;
  }
}

/* ── bridge lifecycle ────────────────────────────────────────────────────── */

let started = false;

export interface BridgeOptions {
  /**
   * Poll `waveform_get` as decoding advances. Only the window that draws lanes
   * wants this; the EQ window would pull megabytes of buckets across the IPC
   * boundary for nothing.
   */
  waveforms?: boolean;
}

/** Attach all event listeners + (optionally) the waveform poller. Idempotent. */
export function startBridge(options: BridgeOptions = {}): () => void {
  if (started) return () => undefined;
  started = true;
  const wantWaveforms = options.waveforms ?? true;

  const unlisteners: Array<() => void> = [];
  let disposed = false;

  // the A/B offset is mirrored outside React so the waveform painters can read
  // it every frame; keep that mirror honest with whatever the engine reports
  const stopOffsetSync = onSnapshot((snap) => syncFromEngine(snap.ab.abOffsetFrames));

  const keep = (event: string, p: Promise<() => void>): void => {
    void p
      .then((un) => {
        if (disposed) un();
        else unlisteners.push(un);
      })
      .catch((err) => {
        // A dropped subscription is fatal to the UI (no frames, no state, no
        // toasts) and used to be swallowed silently.
        if (disposed) return;
        useStore.getState().setConnected(false, api.errorMessage(err));
        useStore.getState().pushToast("error", `Lost the ${event} stream: ${api.errorMessage(err)}`);
      });
  };

  keep(
    "frame",
    api.listenEvent<FramePayload>("onyx://frame", (payload) => {
      frameRef.current = payload;
    }),
  );

  keep(
    "state",
    api.listenEvent<AppSnapshot>("onyx://state", (payload) => {
      useStore.getState().setSnapshot(payload);
    }),
  );

  keep(
    "toast",
    api.listenEvent<ToastPayload>("onyx://toast", (payload) => {
      useStore.getState().pushToast(payload.kind, payload.message);
    }),
  );

  const poll = wantWaveforms
    ? window.setInterval(() => {
        void syncWaveform("a");
        void syncWaveform("b");
      }, 220)
    : 0;

  // First snapshot.
  void (async () => {
    try {
      const snap = await api.appState();
      useStore.getState().setSnapshot(snap);
    } catch (err) {
      useStore.getState().setConnected(false, api.errorMessage(err));
      useStore.getState().pushToast("error", `Engine did not answer: ${api.errorMessage(err)}`);
    }
  })();

  return () => {
    disposed = true;
    started = false;
    frameRef.current = null;
    if (poll) window.clearInterval(poll);
    stopOffsetSync();
    for (const un of unlisteners) {
      try {
        un();
      } catch {
        /* an already-detached listener is not worth reporting */
      }
    }
    unlisteners.length = 0;
  };
}

/** Convenience for components that only need the transport line of a frame. */
export function currentTransport() {
  return frameRef.current?.transport ?? useStore.getState().snapshot?.transport ?? null;
}
