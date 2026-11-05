/** Preserve keyboard focus; clear stale pointer focus after activation. */
import { isTypingTarget } from "./dom";
import { useEffect, useRef } from "react";

/** Non-modal panels enter on their close control and return to their opener. */
export function usePanelFocus(opener: string) {
  const panel = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const trigger = document.querySelector<HTMLElement>(opener);
    const node = panel.current;
    node?.querySelector<HTMLButtonElement>(".close-btn")?.focus({ preventScroll: true });
    return () => {
      const active = document.activeElement;
      if (active === document.body || (active && node?.contains(active))) {
        trigger?.focus({ preventScroll: true });
      }
    };
  }, [opener]);
  return panel;
}

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
