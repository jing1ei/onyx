/**
 * Keyboard layout portability for the shortcut map (SPEC.md §4, §6).
 *
 * A `KeyboardEvent` offers two ways to identify a key and they disagree on
 * every layout that is not US QWERTY:
 *
 *  - `event.key` is the *character produced*. It follows the user's layout and
 *    their modifiers, so `M` is always the key legended M — but `[`, `]` and
 *    `\` are AltGr on German (AltGr+8 / AltGr+9 / AltGr+ß), AltGr on French,
 *    somewhere else entirely on JIS, and dead keys on the Nordic layouts. It
 *    also changes under Shift and Option: Shift+`,` reports `<`, and on macOS
 *    Option+`,` reports `≤`.
 *  - `event.code` is the *physical position*, always the US legend of that
 *    position whatever the layout. Stable, but meaningless to read out.
 *
 * Onyx therefore splits its map:
 *
 *  - **Mnemonic keys stay on `event.key`.** `M` for mute, `L` for loop, `E`
 *    for EQ — the mnemonic is the point, and it survives translation.
 *  - **Positional keys move to `event.code`.** The monitor-matrix folds
 *    `[ ] \`, the alignment nudges `, .` and the blind votes `1 2` are chosen
 *    for where they sit under the hand, not for what they spell. Binding them
 *    by position makes them reachable without AltGr on every layout, and makes
 *    them immune to Shift and Option rewriting the character — which is what
 *    silently killed the Shift (100 ms) and Option (one sample) variants of
 *    the nudge on every keyboard in the world.
 *
 * The cost is that the overlay can no longer print a fixed legend: the key
 * that nudges deck B earlier is `,` on US and QWERTZ, `;` on AZERTY, `。`-side
 * on JIS. This module resolves the real legends, preferring the Keyboard Map
 * API (WebView2 and WebKitGTK) and otherwise learning them from the keys the
 * user actually presses (WKWebView on macOS does not implement the API).
 */

import { isComposing } from "./ime";

/** The keys Onyx binds by position, with their US legends as the fallback. */
export const POSITIONAL_KEYS = {
  BracketLeft: "[",
  BracketRight: "]",
  Backslash: "\\",
  Comma: ",",
  Period: ".",
  Slash: "/",
  Digit1: "1",
  Digit2: "2",
} as const;

export type PositionalCode = keyof typeof POSITIONAL_KEYS;

export function isPositionalCode(code: string): code is PositionalCode {
  return code in POSITIONAL_KEYS;
}

const legends = new Map<string, string>(Object.entries(POSITIONAL_KEYS));
const listeners = new Set<() => void>();
/** Bumped whenever a legend changes, so `useSyncExternalStore` can see it. */
let revision = 0;

function record(code: string, legend: string): void {
  const trimmed = legend.trim();
  if (!trimmed || legends.get(code) === trimmed) return;
  legends.set(code, trimmed);
  revision += 1;
  for (const fn of listeners) fn();
}

/** What is printed on the key at `code`, as far as we can tell. */
export function keyLegend(code: PositionalCode): string {
  return legends.get(code) ?? POSITIONAL_KEYS[code];
}

export function legendRevision(): number {
  return revision;
}

export function subscribeLegends(fn: () => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

/**
 * Learn a legend from a real keystroke.
 *
 * Only unmodified presses are trusted: Shift+`,` and Option+`,` are exactly
 * the characters this module exists to stop believing. Called from the global
 * key handler, so on macOS the overlay corrects itself the first time the user
 * touches one of these keys — including while typing in the search field.
 */
export function observeLegend(e: KeyboardEvent): void {
  if (e.altKey || e.ctrlKey || e.metaKey || e.shiftKey) return;
  /* Nothing an input method is composing is a legend: the character that
     eventually lands is the IME's, not the key's, and a Pinyin session would
     otherwise teach the overlay that `Comma` is printed “，”. `lib/ime.ts`
     explains the two signals. */
  if (isComposing(e)) return;
  if (!isPositionalCode(e.code)) return;
  // Single printable characters only: "Dead", "Unidentified", "Process" and
  // the named keys are not legends.
  if (e.key.length !== 1 || e.key === " ") return;
  record(e.code, e.key);
}

interface KeyboardLayoutMapLike {
  get(code: string): string | undefined;
}

interface KeyboardApiLike {
  getLayoutMap?: () => Promise<KeyboardLayoutMapLike>;
}

/**
 * Ask the platform for the real legends, once, at start-up.
 *
 * `navigator.keyboard` is Chromium-only: it works in WebView2 on Windows and
 * in WebKitGTK on Linux, and is simply absent in WKWebView on macOS, where
 * [`observeLegend`] takes over. Failure is not an error — the US legends are
 * a reasonable guess and the map still works, only the overlay is less
 * precise.
 */
export async function loadKeyboardLayout(): Promise<void> {
  const keyboard = (navigator as Navigator & { keyboard?: KeyboardApiLike }).keyboard;
  if (!keyboard?.getLayoutMap) return;
  try {
    const map = await keyboard.getLayoutMap();
    for (const code of Object.keys(POSITIONAL_KEYS)) {
      const legend = map.get(code);
      if (legend) record(code, legend);
    }
  } catch {
    // Permissions-Policy can refuse this; the fallback legends stand.
  }
}
