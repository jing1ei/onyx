/**
 * The legibility guard — SPEC §20.
 *
 * A theme document can say anything, and one of the things it can say is
 * "grey text on a grey background". This module measures the pairs that
 * actually decide whether the app can be read — the real ink over the real
 * surface it lands on, composited, because almost every ink in Onyx is a
 * translucent white or near-black — and reports the WCAG 2.1 contrast ratio.
 *
 * It **warns and never blocks**. A mastering engineer working in the dark may
 * genuinely want a display that is quieter than any accessibility guideline
 * would allow, and a tool that refuses is a tool they stop using. What it must
 * not do is let the consequence arrive as a surprise: the warning names the
 * pair, the ratio and the threshold, before the theme lands.
 *
 * No DOM here either: the audit runs on the parsed document, so the editor can
 * show it *before* anything is applied.
 */

import type { Rgba } from "./color";
import { contrastRatio, over, parseRgbTriplet } from "./color";
import { colorOf } from "./cssvalue";
import { DERIVED_TOKENS, TOKENS, isToken, tokenSpec } from "./tokens";

export interface ContrastPair {
  /** what the user would call it */
  label: string;
  fg: string;
  bg: string;
  /** below this the pair is reported */
  min: number;
}

export interface ContrastFinding extends ContrastPair {
  ratio: number;
  ok: boolean;
}

/**
 * The bar each ink is held to, by token — WCAG 2.1: 4.5 for body text, 3.0 for
 * large or secondary text and for non-text objects (1.4.11).
 *
 * The deliberately quiet tokens (`--text-lo` is an axis tick, `--text-faint` is
 * a separator dot) are held to a lower bar than body text, because holding them
 * to 4.5 would warn about every theme including the default one — a warning
 * that always fires is a warning nobody reads.
 */
const TEXT_MIN: Readonly<Record<string, number>> = {
  "text-hi": 4.5,
  "text-mid": 3,
  "text-lo": 2,
  // A separator dot and nothing else, so the bar is "visible at all". The
  // shipped dark theme measures 1.53:1 here; a stricter number would warn about
  // the designed appearance, and see above.
  "text-faint": 1.5,
};

/** Anything else in the `text` group is assumed to be real text. */
const DEFAULT_TEXT_MIN = 3;

/**
 * Every ink in the catalogue's `text` group, over the app background.
 *
 * Generated, not listed. §20.2 makes the token catalogue itself generated from
 * `tokens.css`, and an audit written out by hand beside it drifts the first time
 * a token is renamed or added: the pair stops resolving, `auditContrast` skips
 * it silently, and the guard quietly covers less than it says it does. Deriving
 * the text pairs from the catalogue means a new `--text-*` colour is audited on
 * the build that adds it.
 */
function textPairs(): ContrastPair[] {
  return TOKENS.filter(
    (t) => t.group === "text" && t.type === "color" && !DERIVED_TOKENS.has(t.name),
  ).map((t) => ({
    label: `${t.doc?.split(/[—,;(]/)[0].trim() ?? t.name} on the app background`,
    fg: t.name,
    bg: "ink-900",
    min: TEXT_MIN[t.name] ?? DEFAULT_TEXT_MIN,
  }));
}

/**
 * The pairs whose *pairing* is a design fact rather than something the
 * catalogue knows: this ink lands on that surface. Text first, because
 * unreadable text is the failure that makes the app unusable rather than ugly;
 * then the graphics whose only job is to be distinguishable from the field
 * behind them.
 */
const EXPLICIT_PAIRS: readonly ContrastPair[] = [
  { label: "body text on a panel", fg: "text-hi", bg: "surface-panel", min: 4.5 },
  { label: "body text on a menu", fg: "text-hi", bg: "surface-menu", min: 4.5 },
  { label: "secondary text on a panel", fg: "text-mid", bg: "surface-panel", min: 3 },
  { label: "labels and axis ticks on a raised surface", fg: "text-lo", bg: "ink-800", min: 2 },
  { label: "canvas tooltips", fg: "tip-text", bg: "tip-bg", min: 4.5 },
  { label: "accent text and hairlines", fg: "accent", bg: "ink-900", min: 3 },
  // 3.0, not 4.5: an accent fill in Onyx only ever carries a short bold label
  // on a button or a badge, and the shipped light theme sits at 4.03 — the
  // best of the two inks OKLCH can pick for bronze. Holding this pair to 4.5
  // would warn about the default theme, and see above.
  { label: "text on an accent fill", fg: "accent-ink", bg: "accent", min: 3 },
  { label: "the clip colour", fg: "m-clip", bg: "ink-900", min: 3 },
  { label: "deck B against the stage", fg: "deck-b", bg: "ink-900", min: 3 },
  { label: "deck A's waveform bars", fg: "lane-a-rgb", bg: "ink-900", min: 3 },
  { label: "deck B's waveform bars", fg: "lane-b-rgb", bg: "ink-900", min: 3 },
  { label: "the EQ curve on its plate", fg: "eq-curve", bg: "ink-900", min: 3 },
];

export const CONTRAST_PAIRS: readonly ContrastPair[] = [...textPairs(), ...EXPLICIT_PAIRS];

/**
 * Pair sides that are not tokens any more — the drift §20.2 exists to prevent,
 * made visible instead of silently shrinking the audit.
 *
 * A list rather than a thrown error: a token rename must not white-screen the
 * app. `scripts/check-theme.mjs` fails the build on it, which is early enough.
 */
export const CONTRAST_DRIFT: readonly string[] = [
  ...new Set(CONTRAST_PAIRS.flatMap((p) => [p.fg, p.bg]).filter((n) => !isToken(n))),
];

/** Every token the audit measures, for anything that wants to check coverage. */
export const contrastTokens = (): string[] => [
  ...new Set(CONTRAST_PAIRS.flatMap((p) => [p.fg, p.bg])),
];

/** Is this token one the audit is expected to cover? Used by the contract. */
export const auditedByCatalogue = (name: string): boolean => {
  const spec = tokenSpec(name);
  return spec?.group === "text" && spec.type === "color" && !DERIVED_TOKENS.has(name);
};

/** A token name → the CSS this theme gives it. */
export type Lookup = (name: string) => string | undefined;

const MAX_HOPS = 8;

/**
 * Resolve a token to an actual colour, following the token references the
 * theme layer is built out of: `var(--x)`, `rgb(var(--x-rgb) / 0.4)` and bare
 * triplets. `null` when it does not resolve to a literal colour — a gradient,
 * or a chain that runs off the end of the theme.
 */
export function resolveColor(name: string, lookup: Lookup, hops = 0): Rgba | null {
  if (hops > MAX_HOPS) return null;
  const raw = lookup(name)?.trim();
  if (!raw) return null;

  const varOnly = /^var\(--([a-z0-9-]+)\)$/i.exec(raw);
  if (varOnly) return resolveColor(varOnly[1], lookup, hops + 1);

  const tinted = /^rgba?\(\s*var\(--([a-z0-9-]+)\)\s*\/\s*([\d.]+%?)\s*\)$/i.exec(raw);
  if (tinted) {
    const base = resolveColor(tinted[1], lookup, hops + 1);
    if (!base) return null;
    const a = tinted[2].endsWith("%") ? parseFloat(tinted[2]) / 100 : parseFloat(tinted[2]);
    return Number.isFinite(a) ? { ...base, a: base.a * a } : null;
  }

  const triplet = parseRgbTriplet(raw);
  if (triplet) return triplet;

  return colorOf(raw);
}

/**
 * Measure every pair. `page` is the colour a translucent surface is seen
 * against — the app background — so a 96 %-opaque panel is measured as what
 * the eye receives rather than as the colour it was declared with.
 */
export function auditContrast(lookup: Lookup): ContrastFinding[] {
  const page = resolveColor("ink-900", lookup) ?? { r: 0, g: 0, b: 0, a: 1 };
  const opaquePage: Rgba = { ...page, a: 1 };
  const out: ContrastFinding[] = [];
  for (const pair of CONTRAST_PAIRS) {
    const fg = resolveColor(pair.fg, lookup);
    const bg = resolveColor(pair.bg, lookup);
    if (!fg || !bg) continue;
    const behind = over(bg, opaquePage);
    const ink = over(fg, behind);
    const ratio = contrastRatio(ink, behind);
    out.push({ ...pair, ratio, ok: ratio >= pair.min });
  }
  return out;
}

export const failing = (findings: readonly ContrastFinding[]): ContrastFinding[] =>
  findings.filter((f) => !f.ok);

/** `4.53` — two decimals, the way every contrast checker prints it. */
export const formatRatio = (ratio: number): string => `${ratio.toFixed(2)}:1`;
