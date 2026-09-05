/**
 * The detached EQ window, seen from the front end (SPEC §12).
 *
 * The EQ is not a panel any more: it is a second `WebviewWindow` with its own
 * document (`eq.html`), so an engineer can put the curve on another monitor and
 * work it while the main window keeps playing.
 *
 * # Who decides what
 *
 * Rust decides. `src-tauri/src/eqwindow.rs` creates the window, remembers
 * whether it was open and whether it is pinned, and broadcasts
 * `onyx://eq-window` to *every* webview whenever either changes. Nothing here
 * keeps a guess of its own: the main window's EQ button and the EQ window's pin
 * control both render the last broadcast, so the two windows cannot disagree
 * about a fact that only the window system really knows.
 *
 * That is also why there is no `WebviewWindow.create()` anywhere in this file.
 * The renderer holds no window-creation permission (see
 * `src-tauri/capabilities/`), so "open the EQ" is one command, there is exactly
 * one code path, and a second EQ window is not something a bug can produce.
 *
 * In the mock preview there is no Tauri at all: `src/lib/mock.ts` implements
 * the same five commands and the same event by driving a browser popup. This
 * module cannot tell the difference, and neither can the panel.
 */

import * as api from "./api";
import { logWarn } from "./log";
import { useStore } from "./store";
import type { EqWindowState } from "./types";

export const EQ_WINDOW_EVENT = "onyx://eq-window";

/** Optimistic default: closed, and pinned like every other tool window. */
export const EQ_WINDOW_CLOSED: EqWindowState = { open: false, pinned: true };

function fail(what: string, err: unknown): void {
  // A window that will not open is worth a toast — the user pressed a key and
  // nothing happened, which otherwise reads as a frozen app.
  useStore.getState().pushToast("error", `${what}: ${api.errorMessage(err)}`);
}

export function openEqWindow(): void {
  void api.eqWindowOpen().catch((err) => fail("Could not open the EQ window", err));
}

export function closeEqWindow(): void {
  void api.eqWindowClose().catch((err) => fail("Could not close the EQ window", err));
}

export function toggleEqWindow(): void {
  void api.eqWindowToggle().catch((err) => fail("Could not open the EQ window", err));
}

export function setEqWindowPinned(pinned: boolean): void {
  void api
    .eqWindowSetPinned(pinned)
    .catch((err) => fail("Could not change the EQ window's float setting", err));
}

/**
 * Mirror `onyx://eq-window` into the store, and ask for the current value once
 * at startup because a window that loads *after* the last broadcast would
 * otherwise show a stale button until the next change.
 *
 * Idempotent per window, and both windows call it: the EQ window needs the
 * pinned flag for its own control, the main window needs `open` for its button.
 */
export function startEqWindowSync(): () => void {
  let disposed = false;
  let unlisten: (() => void) | null = null;
  const apply = (state: EqWindowState): void => {
    if (!disposed) useStore.getState().setEqWindow(state);
  };

  void api
    .listenEvent<EqWindowState>(EQ_WINDOW_EVENT, apply)
    .then((un) => {
      if (disposed) un();
      else unlisten = un;
    })
    .catch((err) => {
      // Not fatal: the buttons still work, they just stop reflecting a change
      // made from the other window.
      logWarn("could not subscribe to the EQ window state", err);
    });

  void api
    .eqWindowState()
    .then(apply)
    .catch((err) => logWarn("could not read the EQ window state", err));

  return () => {
    disposed = true;
    unlisten?.();
    unlisten = null;
  };
}
