/**
 * Direct DOM write helpers for the 60 Hz path.
 *
 * Values that change every frame (position, peak read-outs, toggle states) are
 * written straight to the node instead of going through React state. Each write
 * is guarded by a read so we never dirty the DOM with an identical value.
 * The meters, the transport and the A/B rail each used to carry their own copy
 * of these two functions.
 */

import { t } from './i18n';
export function setText(el: HTMLElement | null, value: string): void {
  value = t(value);
  if (el && el.textContent !== value) el.textContent = value;
}

export function setAttr(el: Element | null, name: string, value: string): void {
  if (name === 'title' || name === 'aria-label') value = t(value);
  if (el && el.getAttribute(name) !== value) el.setAttribute(name, value);
}

export function setStyle(el: HTMLElement | null, prop: "width" | "left" | "color", value: string): void {
  if (el && el.style[prop] !== value) el.style[prop] = value;
}

/**
 * Platform, for the handful of places that name something the OS names
 * differently. `navigator.platform` is deprecated but is still the only
 * reliable signal inside a Tauri webview; the user-agent is the fallback.
 */
export const IS_MAC = /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);

/** True when the keyboard should stay out of the way (focus is in a field). */
export function isTypingTarget(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  if (!el || typeof el.tagName !== "string") return false;
  const tag = el.tagName.toLowerCase();
  if (tag === "input" || tag === "textarea" || tag === "select" || tag === "option") return true;
  return el.isContentEditable === true;
}
