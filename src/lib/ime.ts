/**
 * One question, asked in every key handler: *is an IME composing right now?*
 *
 * # Why this exists
 *
 * A Pinyin, Kana, Hangul or Vietnamese input method turns keystrokes into a
 * *composition*: the keys go to the IME, a candidate window opens, and the
 * characters only reach the field when the composition ends. While that is
 * happening the browser still delivers `keydown` — with `Tab`, `Enter`,
 * `Escape`, `Space` and the digits all meaning something to the IME and
 * nothing to us. A handler that runs anyway, calls `preventDefault()` and
 * rewrites the field's value does two things at once: it steals the key from
 * the candidate window, and it drops whatever was pending in the composition.
 *
 * This was a live defect in the theme editor (SPEC §20): `Tab` indents there,
 * and pressing `Tab` to pick a candidate — which is what Tab does in several
 * IMEs, and what a user reaching for the next field does mid-word — broke the
 * composition and lost the characters. It is not a hypothetical for anyone
 * writing Chinese.
 *
 * # The rule
 *
 * Every keyboard handler in the app asks this first and returns if the answer
 * is yes. No exceptions except one, which is deliberate and documented where it
 * lives: the appearance reset chord in `lib/theme.ts`
 * (`Ctrl/Cmd + Alt + Shift + R`) is the escape hatch from an unreadable theme
 * and must fire whatever else is going on. A three-modifier chord is not part
 * of any composition, so nothing is stolen by letting it through.
 *
 * # Why two checks
 *
 * `KeyboardEvent.isComposing` is the standard answer and is what every current
 * engine sets. `keyCode === 229` is the older signal the same engines still
 * send for the keystroke that *starts* or *commits* a composition — WebKit and
 * Chromium both report the commit `Enter` with `isComposing: false` and
 * `keyCode: 229`, and Onyx ships inside a WebKit webview on macOS and a
 * Chromium one on Windows. Asking both costs nothing and covers the seam.
 *
 * Deliberately free of DOM and of React: the same predicate serves a native
 * `KeyboardEvent` (`lib/keys.ts`, the window-level handlers) and a React
 * synthetic one via `e.nativeEvent`, and `scripts/check-theme.mjs` can load it
 * in Node.
 */

/** The parts of a keyboard event this predicate needs. Nothing more. */
export interface ComposingLike {
  isComposing?: boolean;
  keyCode?: number;
}

/** `keydown` for a key the IME has taken: WebKit and Chromium both send it. */
export const IME_KEY_CODE = 229;

/**
 * True while an input method owns the keystroke.
 *
 * Handlers must return early: the key belongs to the candidate window, and a
 * `preventDefault()` here is a dropped character.
 */
export function isComposing(e: ComposingLike | null | undefined): boolean {
  if (!e) return false;
  return e.isComposing === true || e.keyCode === IME_KEY_CODE;
}

/**
 * The same question for a React synthetic event, whose `isComposing` lives on
 * the native event it wraps. Written as its own function because
 * `e.nativeEvent.isComposing` is exactly the thing that gets forgotten.
 */
export function isComposingReact(
  e: { nativeEvent?: ComposingLike | null } | null | undefined,
): boolean {
  return isComposing(e?.nativeEvent);
}
