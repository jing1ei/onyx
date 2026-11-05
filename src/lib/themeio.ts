/**
 * The theme document, seen from the UI — SPEC §20.
 *
 * `themedoc.ts` decides what a document *means* and never touches the DOM;
 * `theme.ts` owns the DOM and never parses anything. This is the seam between
 * them: validate, apply, persist, revert, and put text on the clipboard. Two
 * surfaces use it — the editor window (`src/theme/`) and the compact editor in
 * the settings panel — and they must behave identically, so there is one
 * implementation of "apply" and not two.
 *
 * # The rules this file exists to keep
 *
 * **Nothing is applied until everything validates.** `parseTheme` returns
 * `doc: null` if a single key or value is wrong, and [`applyTheme`] returns
 * before touching the DOM in that case. There is no partial theme.
 *
 * **A failed persist puts the previous look back.** Applying is two steps —
 * paint now, save through Rust a moment later — and the second can refuse (a
 * document too large for the settings file, a backend that is gone). If it
 * does, the appearance that was on screen before is restored, because a theme
 * that survives until the next launch and then vanishes is worse than one that
 * never landed.
 *
 * **The escape hatch does not depend on any of this.** See
 * [`resetAppearanceEverywhere`] and `theme.ts`'s capture-phase key handler.
 */

import * as api from "./api";
import { logWarn } from "./log";
import { useStore } from "./store";
import {
  applyAppearance,
  applyThemeDoc,
  currentAppearance,
  currentThemeDoc,
  currentThemeText,
  DEFAULT_APPEARANCE,
  resetAppearanceLocally,
  type Appearance,
} from "./theme";
import type { ParseOutcome, ThemeDoc } from "./themedoc";
import { errorsOf, exportTheme, forAgent, parseTheme } from "./themedoc";

/* ── reading the current state ────────────────────────────────────────────── */

/** The designed appearance as a document: what "Copy default" puts on the clipboard. */
export const defaultThemeText = (): string =>
  exportTheme({ doc: null, appearance: DEFAULT_APPEARANCE, name: "Onyx" });

/**
 * What is on screen, as a document.
 *
 * If a document is in force it is returned **verbatim** — comments, ordering
 * and all. Round-tripping it through the exporter would be lossless as far as
 * the tokens go (there is a check for that) but would throw away the notes the
 * user or the model wrote in it, and those are half of what makes the next edit
 * work. With no document in force, the current appearance is exported instead,
 * so "Copy current" is always a complete, valid, self-describing theme.
 */
export const currentThemeSource = (): string =>
  currentThemeText() ?? exportTheme({ doc: null, appearance: currentAppearance(), name: "Onyx" });

/** Theme text plus the compact contract — "Copy for agent". */
export const themeForAgent = (text?: string): string => forAgent(text ?? currentThemeSource());

/** Validate without applying. The editor calls this on every keystroke pause. */
export const validateTheme = (text: string): ParseOutcome =>
  parseTheme(text, currentAppearance());

/* ── applying ─────────────────────────────────────────────────────────────── */

export interface ApplyResult {
  outcome: ParseOutcome;
  /** true only when the document is now in force in this window */
  applied: boolean;
}

/**
 * Validate, apply everywhere, persist.
 *
 * The order matters and is deliberate:
 *
 *  1. parse — one error and nothing else happens;
 *  2. apply locally, in one pass, so the window is already wearing the theme
 *     before any IPC happens (this is what makes Apply feel like a switch
 *     rather than a request);
 *  3. persist the appearance block and the document text. The *other* windows
 *     re-skin from the resulting `onyx://state` broadcast, or — in a mock
 *     preview, where there is no backend to broadcast — from the appearance
 *     cache relay in `theme.ts`. Both paths carry the document.
 *
 * If step 3 fails, step 2 is undone.
 */
export async function applyTheme(text: string): Promise<ApplyResult> {
  const outcome = validateTheme(text);
  if (!outcome.doc) return { outcome, applied: false };

  const previousDoc = currentThemeDoc();
  const previousText = currentThemeText();
  const previousLook = currentAppearance();
  const look: Appearance = { ...previousLook, ...outcome.doc.appearance };

  applyThemeDoc(outcome.doc, text, look);

  try {
    // The appearance first: it is the smaller, stricter payload, and a backend
    // that refuses it (an accent that is not a colour) should not leave a
    // document persisted against an appearance that was not.
    const confirmed = await api.setAppearance(look);
    await api.setThemeDoc(text);
    // The backend normalises (`#C9A227` → `#c9a227`); adopt what it says is in
    // force rather than what we asked for.
    applyThemeDoc(outcome.doc, text, confirmed);
  } catch (err) {
    applyThemeDoc(previousDoc, previousText, previousLook);
    throw new Error(api.errorMessage(err));
  }
  return { outcome, applied: true };
}

/**
 * Drop the document, keep the accent and fonts. "Revert" in the editor.
 *
 * Not the same thing as the escape hatch: this is the ordinary "I do not want
 * this skin any more" button, and it leaves the simple controls where the user
 * left them.
 */
export async function revertTheme(): Promise<void> {
  const previousDoc = currentThemeDoc();
  const previousText = currentThemeText();
  applyThemeDoc(null, null);
  try {
    await api.setThemeDoc(null);
  } catch (err) {
    applyThemeDoc(previousDoc, previousText);
    throw new Error(api.errorMessage(err));
  }
}

/**
 * The escape hatch: document *and* appearance back to the designed defaults,
 * in every window.
 *
 * Local first and unconditionally — the point of this path is that it works
 * when other things do not — then through the backend so it survives a
 * relaunch. A backend failure is logged, not thrown: the user pressed this
 * because they could not read the screen, and a toast they cannot read is not
 * an answer.
 */
export function resetAppearanceEverywhere(): void {
  resetAppearanceLocally();
  void api
    .resetAppearance()
    .then((appearance) => applyAppearance(appearance))
    .catch((err) => logWarn("could not persist the appearance reset", err));
}

/* ── the clipboard ────────────────────────────────────────────────────────── */

/**
 * Copy text, with the fallback every desktop webview eventually needs.
 *
 * `navigator.clipboard` requires a secure context and a focused document, and a
 * tool window that has just been dragged to a second monitor is not always
 * focused. The `execCommand` path is deprecated and works everywhere, which is
 * the correct trade for a button whose entire job is "put this on the
 * clipboard".
 */
export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    /* fall through */
  }
  try {
    const area = document.createElement("textarea");
    area.value = text;
    area.setAttribute("readonly", "");
    // Off-screen but focusable; `display: none` cannot be selected.
    area.style.cssText = "position:fixed;top:-1000px;left:-1000px;opacity:0";
    document.body.appendChild(area);
    area.select();
    const ok = document.execCommand("copy");
    area.remove();
    return ok;
  } catch (err) {
    logWarn("could not copy to the clipboard", err);
    return false;
  }
}

/** Copy, and say so in the usual place. Returns what the toast said. */
export async function copyWithToast(text: string, what: string): Promise<boolean> {
  const ok = await copyText(text);
  const { pushToast } = useStore.getState();
  if (ok) pushToast("info", `${what} copied — ${lineCount(text)} lines`);
  else pushToast("error", `Could not reach the clipboard — select the text and copy it by hand`);
  return ok;
}

const lineCount = (text: string): number => text.split("\n").length;

/* ── describing a document, for the UI ────────────────────────────────────── */

export interface ThemeSummary {
  name: string;
  /** tokens the document actually states, across all three blocks */
  tokens: number;
  errors: number;
  warnings: number;
  /** contrast pairs below their threshold, in the theme that is in force */
  unreadable: number;
}

export function summarise(outcome: ParseOutcome, doc: ThemeDoc | null = outcome.doc): ThemeSummary {
  const problems = outcome.problems;
  return {
    name: doc?.name ?? "Untitled",
    tokens: doc ? doc.base.size + doc.dark.size + doc.light.size : 0,
    errors: errorsOf(problems).length,
    warnings: problems.length - errorsOf(problems).length,
    unreadable: outcome.contrast.filter((f) => !f.ok).length,
  };
}
