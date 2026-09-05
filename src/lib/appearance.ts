/**
 * The appearance model — the small, typed thing the settings panel edits and
 * `settings.json` stores.
 *
 * Split out of `theme.ts` so that the parts of the theming system with no DOM
 * in them (this, `accent.ts`, `themedoc.ts`) can be imported by the theme
 * document validator and exercised in Node. `theme.ts` re-exports all of it,
 * so the rest of the app still has one theming import.
 *
 * Mirrors `Appearance` in `src-tauri/src/settings.rs`, but every field is
 * optional on the way in and every value is validated here: this is the
 * boundary the settings UI writes through, and it must survive a half-built
 * payload, an older `settings.json` and a snapshot from a backend that does
 * not carry an appearance block yet.
 */

import { DEFAULT_ACCENT } from "./accent";
import { parseHex } from "./color";

export type ThemeSetting = "dark" | "light" | "system";
export type ResolvedTheme = "dark" | "light";
export type SizeScale = "compact" | "normal" | "large";

export interface Appearance {
  theme: ThemeSetting;
  /** `#rrggbb`, lower case */
  accent: string;
  uiFont: string;
  numericFont: string;
  sizeScale: SizeScale;
}

/** Anything the settings UI, a snapshot or a theme document may hand us. */
export interface AppearanceLike {
  theme?: string | null;
  accent?: string | null;
  uiFont?: string | null;
  numericFont?: string | null;
  sizeScale?: string | null;
}

/** Same defaults as `Appearance::default()` in Rust. */
export const DEFAULT_APPEARANCE: Appearance = {
  theme: "dark",
  accent: DEFAULT_ACCENT,
  uiFont: "system",
  numericFont: "system-mono",
  sizeScale: "normal",
};

export const THEME_SETTINGS: readonly ThemeSetting[] = ["dark", "light", "system"];
export const SIZE_SCALES: readonly SizeScale[] = ["compact", "normal", "large"];

/* ── the curated font lists (§15) ─────────────────────────────────────────
   Tokens, not family names: `src/styles/tokens.css` maps each one to a stack
   made only of faces the platform already ships, so nothing is ever fetched —
   the CSP forbids remote origins and this is an offline tool. Adding an option
   here means adding the matching `:root[data-ui-font="…"]` block there.

   A theme document may also set the `font-ui` / `font-num` tokens outright,
   which beats the picker; these are the names the picker offers and the names
   the document validator suggests when it sees a near miss. */

export const UI_FONTS: ReadonlyArray<{ token: string; label: string }> = [
  { token: "system", label: "System UI" },
  { token: "grotesk", label: "Grotesk \u00B7 Helvetica" },
  { token: "humanist", label: "Humanist \u00B7 Avenir" },
  { token: "neutral", label: "Neutral \u00B7 Inter" },
];

/** Monospaced only: the read-outs are tabular and must stay column-stable. */
export const NUM_FONTS: ReadonlyArray<{ token: string; label: string }> = [
  { token: "system-mono", label: "System mono" },
  { token: "sf-mono", label: "SF Mono" },
  { token: "menlo", label: "Menlo" },
  { token: "consolas", label: "Consolas" },
  { token: "courier", label: "Courier" },
];

export function normaliseAppearance(a: AppearanceLike | null | undefined): Appearance {
  const theme = THEME_SETTINGS.find((t) => t === a?.theme) ?? DEFAULT_APPEARANCE.theme;
  const scale = SIZE_SCALES.find((s) => s === a?.sizeScale) ?? DEFAULT_APPEARANCE.sizeScale;
  const font = (v: string | null | undefined, fallback: string): string =>
    typeof v === "string" && /^[a-z0-9-]{1,32}$/i.test(v) ? v.toLowerCase() : fallback;
  return {
    theme,
    accent: (a?.accent ? parseHex(a.accent) : null) ?? DEFAULT_APPEARANCE.accent,
    uiFont: font(a?.uiFont, DEFAULT_APPEARANCE.uiFont),
    numericFont: font(a?.numericFont, DEFAULT_APPEARANCE.numericFont),
    sizeScale: scale,
  };
}

export const sameAppearance = (a: Appearance, b: Appearance): boolean =>
  a.theme === b.theme &&
  a.accent === b.accent &&
  a.uiFont === b.uiFont &&
  a.numericFont === b.numericFont &&
  a.sizeScale === b.sizeScale;
