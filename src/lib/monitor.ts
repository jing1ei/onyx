/**
 * Monitor matrix metadata (SPEC.md §6).
 *
 * One table, shared by the transport control, the persistent badge and the
 * keyboard map, so a mode can never be described one way in the UI and another
 * way in the shortcut overlay.
 */

import { isPositionalCode, keyLegend, POSITIONAL_KEYS, type PositionalCode } from "./layout";
import type { MonitorMode } from "./types";

export interface MonitorSpec {
  mode: MonitorMode;
  /** short label for the segmented control */
  short: string;
  /** spelt-out name for the badge */
  long: string;
  /** what the fold actually does, in signal terms */
  maths: string;
  /**
   * Physical key that selects (and, when active, un-selects) this mode.
   *
   * `KeyO` / `KeyS` / `KeyP` are matched by the character they produce, so the
   * mnemonic holds on every layout; the bracket and backslash folds are
   * matched by position, because those characters need AltGr on most European
   * layouts and do not exist at all on some (see `lib/layout.ts`).
   */
  code: string;
  /** The character to match on, for the mnemonic folds only. */
  char: string | null;
  /** side / polarity folds sound broken to a novice — they get the hot tone */
  alarming?: boolean;
}

/** Every non-stereo fold, in the order the transport control shows them. */
export const MONITOR_FOLDS: MonitorSpec[] = [
  {
    mode: "mono",
    short: "Mono",
    long: "Mono",
    maths: "(L+R)/2 to both legs",
    code: "KeyO",
    char: "o",
  },
  {
    mode: "side",
    short: "Side",
    long: "Side solo",
    maths: "(L\u2212R)/2 to both legs",
    code: "KeyS",
    char: "s",
    alarming: true,
  },
  {
    mode: "left",
    short: "L",
    long: "Left only",
    maths: "L to both legs",
    code: "BracketLeft",
    char: null,
  },
  {
    mode: "right",
    short: "R",
    long: "Right only",
    maths: "R to both legs",
    code: "BracketRight",
    char: null,
  },
  {
    mode: "swap",
    short: "Swap",
    long: "Channels swapped",
    maths: "R, L",
    code: "Backslash",
    char: null,
  },
  {
    mode: "flipRight",
    short: "\u00D8 R",
    long: "Right polarity inverted",
    maths: "L, \u2212R",
    code: "KeyP",
    char: "p",
    alarming: true,
  },
];

const STEREO: MonitorSpec = {
  mode: "stereo",
  short: "Stereo",
  long: "Stereo",
  maths: "untouched \u00B7 bit-transparent",
  code: "",
  char: null,
};

export function monitorSpec(mode: MonitorMode): MonitorSpec {
  return MONITOR_FOLDS.find((m) => m.mode === mode) ?? STEREO;
}

/** The physical key a fold is bound to, or null for the mnemonic folds. */
export function monitorCode(spec: MonitorSpec): PositionalCode | null {
  return isPositionalCode(spec.code) ? spec.code : null;
}

/**
 * What is printed on the key that selects `spec`, on this user's keyboard.
 *
 * Empty for stereo, which has no key of its own — pressing the active fold's
 * key again is what returns to stereo.
 */
export function monitorLegend(spec: MonitorSpec): string {
  if (spec.char) return spec.char.toUpperCase();
  if (isPositionalCode(spec.code)) return keyLegend(spec.code);
  return "";
}

/**
 * The fold a keystroke selects, or null if it selects none.
 *
 * Mnemonic folds match the character; positional folds match the physical key.
 * `code` is empty under some input methods and on virtual keyboards, so the US
 * legend is accepted as a fallback there rather than losing the binding.
 */
export function monitorForEvent(e: { key: string; code: string }): MonitorMode | null {
  const char = e.key.toLowerCase();
  const hit = MONITOR_FOLDS.find((m) => {
    if (m.char) return m.char === char;
    if (e.code) return m.code === e.code;
    // No `code` at all: fall back to the US legend of that position.
    return isPositionalCode(m.code) && POSITIONAL_KEYS[m.code] === e.key;
  });
  return hit?.mode ?? null;
}

/** Pressing a fold's key while it is active returns to stereo (SPEC §6). */
export function toggleMonitor(current: MonitorMode, wanted: MonitorMode): MonitorMode {
  return current === wanted ? "stereo" : wanted;
}

/** The one-line hint that has to appear wherever the matrix is offered. */
export const MONITOR_HINT =
  "Monitoring fold only \u2014 it sits after the meter tap, so LUFS, true peak and LRA keep describing the true stereo programme, not the fold.";
