/**
 * Accent derivation — the one hex the user picks, expanded into a family.
 *
 * Split out of `theme.ts` because it is pure arithmetic that three places
 * need: the DOM applier, the theme document's contrast audit (which has to
 * know what the accent will *become* before anything is applied), and the
 * tests, which run in Node with no window.
 */

import type { Lch, Rgb } from "./color";
import { hexToRgb, lchToHex, parseHex, rgbToLch } from "./color";

/** Same default as `Appearance::default()` in Rust. */
export const DEFAULT_ACCENT = "#c9a227";

export type ResolvedTheme = "dark" | "light";

interface Step {
  /** lightness offset from the chosen accent */
  dL: number;
  /** chroma multiplier */
  cs: number;
  /** hue offset in degrees */
  dh: number;
  /** legibility window for L */
  lo: number;
  hi: number;
  /** chroma ceiling */
  cMax: number;
}

export interface AccentFamily {
  /** the primary accent: text, hairlines, waveform A, the EQ curve */
  accent: string;
  /** hover / brighter */
  hi: string;
  /** pressed / the far end of a fill gradient */
  press: string;
  /** the deepest tone: washes, glows, the analyser's floor */
  deep: string;
  /** what to print *on* an accent fill, as a token reference */
  ink: string;
}

/* The transforms are *relative* to the chosen accent and were measured from the
   hand-tuned champagne set of SPEC §4, so feeding the default accent `#c9a227`
   reproduces `#e8d9a0 / #f3e8bf / #cdb478 / #c9a227` exactly — the dark
   identity does not move — while any other accent gets the same treatment
   instead of inheriting champagne's numbers.

   Lightness is clamped to a legibility window per theme: an accent that is
   nearly black must still be visible on obsidian, and one that is nearly white
   must still be visible on alabaster. Chroma is capped so a fluorescent hex
   cannot shout its way past the rest of the palette. */
const DARK_STEPS: Record<keyof Omit<AccentFamily, "ink">, Step> = {
  accent: { dL: +0.1559, cs: 0.547, dh: +5.39, lo: 0.72, hi: 0.94, cMax: 0.17 },
  hi: { dL: +0.2014, cs: 0.397, dh: +5.03, lo: 0.78, hi: 0.97, cMax: 0.14 },
  press: { dL: +0.0497, cs: 0.603, dh: -1.93, lo: 0.62, hi: 0.86, cMax: 0.18 },
  deep: { dL: 0, cs: 1, dh: 0, lo: 0.5, hi: 0.8, cMax: 0.22 },
};

/* Light is not the dark ramp inverted. On paper the accent has to *darken* to
   carry contrast, and it holds nearly all of its chroma: a bronze that loses
   chroma as it darkens turns into mud, and champagne at paper lightness is
   invisible. */
const LIGHT_STEPS: Record<keyof Omit<AccentFamily, "ink">, Step> = {
  accent: { dL: -0.145, cs: 0.95, dh: -7, lo: 0.4, hi: 0.64, cMax: 0.15 },
  hi: { dL: -0.205, cs: 1.0, dh: -7, lo: 0.34, hi: 0.58, cMax: 0.16 },
  press: { dL: -0.255, cs: 1.0, dh: -8, lo: 0.3, hi: 0.54, cMax: 0.16 },
  deep: { dL: -0.3, cs: 0.98, dh: -9, lo: 0.26, hi: 0.5, cMax: 0.16 },
};

/** Ink on an accent fill flips at the point where the fill stops being dark. */
const INK_FLIP_L = 0.6;

export function accentFamily(hex: string, theme: ResolvedTheme): AccentFamily {
  const base: Lch = rgbToLch(hexToRgb(parseHex(hex) ?? DEFAULT_ACCENT));
  const steps = theme === "light" ? LIGHT_STEPS : DARK_STEPS;
  const tone = (s: Step): string =>
    lchToHex({
      L: Math.min(s.hi, Math.max(s.lo, base.L + s.dL)),
      C: Math.min(s.cMax, base.C * s.cs),
      h: base.h + s.dh,
    });
  const accent = tone(steps.accent);
  const accentL = rgbToLch(hexToRgb(accent)).L;
  return {
    accent,
    hi: tone(steps.hi),
    press: tone(steps.press),
    deep: tone(steps.deep),
    // A token reference, not a literal: the two inks are the theme's own
    // extremes and stay in the token layer.
    ink: accentL >= INK_FLIP_L ? "var(--ink-on-accent-dark)" : "var(--ink-on-accent-light)",
  };
}

/** `232 217 160` — the triplet form CSS needs for `rgb(var(--x) / 0.2)`. */
export const triplet = (hex: string): string => (hexToRgb(hex) as Rgb).join(" ");

/**
 * Deck B must stay instantly separable from deck A (SPEC §4), and deck A *is*
 * the accent. A steel-blue accent would put the two lanes in the same colour,
 * so when the accent invades deck B's hue the steel is rotated out of its way —
 * keeping its own lightness and chroma, which is what makes it read as the
 * cold half of the pair.
 */
const DECK_HUE_GUARD = 40;

export function deckB(sourceHex: string, accent: string): string {
  const hex = parseHex(sourceHex) ?? "#7fa8b8";
  const a = rgbToLch(hexToRgb(accent));
  const b = rgbToLch(hexToRgb(hex));
  const delta = Math.abs(((a.h - b.h + 540) % 360) - 180);
  if (delta > DECK_HUE_GUARD) return hex;
  // far enough round that no accent can chase it, and still a cold hue for any
  // warm accent
  return lchToHex({ ...b, h: a.h + 160 });
}

/**
 * The custom properties the runtime owns, for one accent and one theme.
 *
 * Keys are token names *without* the leading `--`, which is the form the theme
 * document and the catalogue use; the applier adds the dashes.
 */
export function derivedVars(
  accentHex: string,
  theme: ResolvedTheme,
  deckBSrc: string,
): Map<string, string> {
  const family = accentFamily(accentHex, theme);
  const steel = deckB(deckBSrc, family.accent);
  return new Map([
    ["accent", family.accent],
    ["accent-rgb", triplet(family.accent)],
    ["accent-hi", family.hi],
    ["accent-hi-rgb", triplet(family.hi)],
    ["accent-press", family.press],
    ["accent-deep", family.deep],
    ["accent-deep-rgb", triplet(family.deep)],
    ["accent-ink", family.ink],
    ["deck-b", steel],
    ["deck-b-rgb", triplet(steel)],
  ]);
}
