/** IME keystrokes belong to the composition, not application shortcuts.
 * Check both isComposing and keyCode 229: WebKit can report the committing
 * Enter with isComposing false. The appearance-reset chord is the exception.
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
