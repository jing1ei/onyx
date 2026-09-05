/**
 * Focus hygiene, app-wide.
 *
 * Onyx is keyboard-first, so a keyboard focus ring has to be obvious. But a
 * `<button>` clicked with the mouse *keeps* DOM focus, and the very next key
 * press — `Space`, i.e. play/pause, the most used key in the app — makes
 * Chromium promote that stale focus to `:focus-visible` and paint its bright UA
 * ring on a control the user stopped interacting with several minutes ago. That
 * is the "the RESET button lights up and never goes out" report: nothing in the
 * app was lit, the browser was.
 *
 * The rule installed here: a pointer-driven activation does not leave focus
 * behind, a keyboard-driven one does.
 *
 *   - `click` with `detail > 0` came from a pointer → blur the control.
 *   - `click` with `detail === 0` came from `Enter` / `Space` on a focused
 *     control → focus stays exactly where the keyboard user put it.
 *
 * Text fields, selects and anything the user is meant to keep typing into are
 * left alone; they are the one case where a mouse click *should* park focus.
 */

import { isTypingTarget } from "./dom";

function isPointerActivation(e: MouseEvent): boolean {
  // Keyboard activation of a button synthesises a click with detail 0. A real
  // pointer always reports the click count (1, 2, 3…).
  return e.detail > 0;
}

export function installFocusHygiene(): () => void {
  const onClick = (e: MouseEvent): void => {
    if (!isPointerActivation(e)) return;
    const active = document.activeElement;
    if (!(active instanceof HTMLElement)) return;
    if (isTypingTarget(active)) return;
    // Only the control that was actually clicked, so a click somewhere else
    // never steals focus from a field the user is typing in.
    const target = e.target instanceof Node ? e.target : null;
    if (!target || !(active === target || active.contains(target))) return;
    active.blur();
  };

  // Bubble phase, so a component's own handler has already run: if it moved
  // focus somewhere deliberately, `activeElement` is no longer the clicked
  // control and the check above leaves that decision alone.
  window.addEventListener("click", onClick);
  return () => window.removeEventListener("click", onClick);
}
