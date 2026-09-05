/**
 * Who needs the FFT? — SPEC.md §12.
 *
 * `set_spectrum_enabled` is a single engine-wide switch and the analyser must
 * not run when nothing is drawing it. The EQ panel's backdrop is the only view
 * that wants it (the standalone spectrum in the old meter bridge is gone), and
 * it releases the analyser while a blind test runs, so a closed EQ panel really
 * does cost no FFT.
 *
 * Consumers are still counted rather than toggled directly: the panel can be
 * remounted, and a blind test can start and stop, faster than a command
 * round-trip settles, and "enable on mount / disable on unmount" from two
 * overlapping lifetimes leaves the engine in whichever state lost the race.
 */

import * as api from "./api";
import { logWarn } from "./log";
import { useStore } from "./store";

let consumers = 0;
let desired = false;
let applied = false;
let inflight = false;
/**
 * The target whose command failed. `reconcile` re-runs itself when a call
 * settles so that a toggle made mid-flight is not lost, which without this
 * guard turns a persistently failing command into an unbounded IPC retry loop
 * (and, when enabling, one toast per attempt). A failed target is therefore
 * parked until the desired state actually changes again.
 */
let failed: boolean | null = null;

function reconcile(): void {
  if (inflight || desired === applied || desired === failed) return;
  const target = desired;
  inflight = true;
  void api
    .setSpectrumEnabled(target)
    .then(() => {
      applied = target;
      failed = null;
    })
    .catch((err) => {
      failed = target;
      // Enabling matters (an empty analyser looks like a broken app); disabling
      // only wastes CPU, so it is logged rather than shouted about.
      if (target) {
        useStore.getState().pushToast("warn", `Analyser unavailable: ${api.errorMessage(err)}`);
      } else {
        logWarn("could not disable the spectrum analyser", err);
      }
    })
    .finally(() => {
      inflight = false;
      reconcile();
    });
}

/** Register a view that needs spectrum frames. Call the result on unmount. */
export function acquireSpectrum(): () => void {
  consumers += 1;
  desired = true;
  reconcile();
  let released = false;
  return () => {
    if (released) return;
    released = true;
    consumers = Math.max(0, consumers - 1);
    if (consumers === 0) {
      desired = false;
      reconcile();
    }
  };
}
