/**
 * Band-solo audition state (SPEC.md §12).
 *
 * Sweeping is a pointer-rate gesture and the indicator has to feel instant, so
 * the state lives in a module-level ref that the 60 Hz painters read directly.
 * Nothing here re-renders React.
 *
 * IPC writes are **serialised**: one `set_eq_audition` is allowed on the wire
 * at a time and only the newest value is kept while it is in flight. That
 * coalesces a fast sweep down to what the engine can actually absorb, and —
 * the reason it matters — it makes the final "off" write the last one to
 * arrive. Two overlapping calls can be handled out of order by the async
 * runtime, and if the loser is the "off" write the engineer is left monitoring
 * a narrow bandpass with nothing on screen to explain it.
 *
 * Nothing here waits on `requestAnimationFrame` for the same reason: a sweep
 * interrupted by Cmd-Tab must still be cleared while the window is occluded,
 * and rAF does not run there.
 *
 * # Two windows
 *
 * `auditionRef` is the *sweeping* window's own copy, and only the EQ window
 * ever writes it — it is what makes the overlay follow the pointer without an
 * IPC round trip. It is deliberately not what the main window reads: that
 * window has no idea a sweep is happening and cannot be told by a module ref
 * in another webview, so its badge reads `frame.audition`, which comes from the
 * engine (SPEC §12, `AuditionFrame`). One authority, two views of it.
 */

import * as api from "./api";
import { useStore } from "./store";

export interface AuditionState {
  freqHz: number;
  q: number;
}

export const auditionRef: { current: AuditionState | null } = { current: null };

/** `undefined` = nothing queued; `null` = "switch the audition off" is queued. */
let pending: AuditionState | null | undefined;
/** A `set_eq_audition` is on the wire. */
let inflight = false;
/** Last value the engine was asked for, so a no-op write can be skipped. */
let lastSent: AuditionState | null = null;
/** Send the next "off" even if this webview thinks nothing is engaged. */
let forceOff = false;

function flush(): void {
  if (inflight || pending === undefined) return;

  const next = pending;
  pending = undefined;

  if (next == null) {
    if (lastSent == null && !forceOff) return;
    forceOff = false;
    const was = lastSent;
    lastSent = null;
    send(api.setEqAudition(null, 1), null, was);
    return;
  }
  // a new sweep supersedes a queued force-off
  forceOff = false;
  if (lastSent && Math.abs(lastSent.freqHz - next.freqHz) < 0.5 && Math.abs(lastSent.q - next.q) < 0.01) {
    return;
  }
  lastSent = next;
  send(api.setEqAudition(next.freqHz, next.q), next, null);
}

/**
 * `target` is what the engine was asked for; `was` is what it is presumed to
 * still be doing if the call fails (only meaningful when switching off).
 */
function send(p: Promise<void>, target: AuditionState | null, was: AuditionState | null): void {
  inflight = true;
  void p
    .catch((err) => {
      const message = api.errorMessage(err);
      if (target != null) {
        // The bandpass never reached the engine: nothing is being auditioned.
        auditionRef.current = null;
        lastSent = null;
        useStore.getState().pushToast("error", `Band solo failed: ${message}`);
      } else if (pending === undefined) {
        // The engine may well still be filtering. Reporting "off" here would
        // make the badge lie about the signal path, so the badge stays lit and
        // clicking it retries the same command.
        auditionRef.current = was;
        lastSent = was;
        useStore
          .getState()
          .pushToast("error", `Still in band solo: ${message}. Click the Band solo badge to retry.`);
      }
    })
    .finally(() => {
      inflight = false;
      flush();
    });
}

/** Set (or clear, with `null`) the audition bandpass. */
export function setAudition(next: AuditionState | null): void {
  auditionRef.current = next;
  pending = next;
  flush();
}

/**
 * Hard stop — used on unmount, on focus loss and when the EQ window closes.
 *
 * Deliberately a no-op when this webview believes nothing is engaged: it is
 * called from several teardown paths at once and each one must be free.
 */
export function stopAudition(): void {
  if (auditionRef.current == null && lastSent == null && pending == null) return;
  setAudition(null);
}

/**
 * Stop the bandpass from a window that never started one.
 *
 * The main window's Band solo badge reads the *engine's* audition state off the
 * frame stream, because the sweep happens in the EQ window and no module ref
 * crosses that boundary. So the badge can be lit while every variable in this
 * module is still null, and [`stopAudition`] would return early and look like a
 * dead button. This one asserts the "off" write regardless of local belief —
 * the belief is this webview's, the bandpass is the engine's.
 */
export function forceStopAudition(): void {
  forceOff = true;
  auditionRef.current = null;
  pending = null;
  flush();
}
