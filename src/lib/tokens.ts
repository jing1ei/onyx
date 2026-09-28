/**
 * The token catalogue — the schema half of the theme contract (SPEC §20).
 *
 * # Why this file parses CSS instead of listing defaults
 *
 * The theme editor exports "the current look, as a document". If the default
 * values were written out here by hand they would drift from
 * `src/styles/tokens.css` the first time a designer nudged a colour, and the
 * export would then quietly lie to the user — they would paste a document that
 * claims to be the default and get something else. So the defaults are read
 * from the real sheet, at runtime, through Vite's `?raw` import. There is one
 * source of truth and drift is not possible, rather than merely tested for.
 *
 * What *is* hand-maintained here is the metadata a machine cannot infer: which
 * group a token belongs to, what it does in one line, and the range a numeric
 * token is allowed to move in. A token added to `tokens.css` and not mentioned
 * here still appears in the document, typed by inference and grouped under
 * "other" — the contract can never be missing a token, only missing prose.
 *
 * # Types
 *
 * Every value is parsed to a typed thing and re-emitted by `cssvalue.ts`; a
 * theme is never a string of CSS we paste into the page. The types are the
 * whole vocabulary a theme document may use:
 *
 *   color       #hex · named · rgb()/rgba() · hsl() · oklch() · transparent ·
 *               var(--other-token) · rgb(var(--x-rgb) / 0.4)
 *   rgb         "232 217 160" (a triplet, for rgb(var(--x) / a)) · var(--y-rgb)
 *   number      unitless, clamped per token
 *   length      px
 *   duration    ms
 *   tracking    em / px / 0
 *   easing      cubic-bezier(...) or a CSS easing keyword
 *   font        a family stack, sanitised hard
 *   background  a colour, a gradient, or a comma-separated stack of them
 *   shadow      a comma-separated list of shadows
 *   blend       one CSS blend keyword from a fixed list
 *   image       a `data:image/svg+xml` URL, nothing else
 */

import tokensCss from "../styles/tokens.css?raw";

export type TokenType =
  | "color"
  | "rgb"
  | "number"
  | "length"
  | "duration"
  | "tracking"
  | "easing"
  | "font"
  | "background"
  | "shadow"
  | "blend"
  | "image";

export interface Range {
  min: number;
  max: number;
}

export interface TokenSpec {
  /** the key in a theme document; the CSS custom property is `--${name}` */
  name: string;
  type: TokenType;
  /** `base` tokens are stated once; `theme` tokens are stated per theme */
  scope: "base" | "theme";
  /** the default, exactly as `tokens.css` declares it */
  base?: string;
  dark?: string;
  light?: string;
  group: string;
  doc?: string;
  /** numeric bounds, for `number` / `length` / `duration` / `tracking` */
  range?: Range;
}

/* ── tokens the runtime owns ──────────────────────────────────────────────
   `theme.ts` writes these on every apply: the accent family is *derived* from
   the single hex in `appearance.accent` (so that one hex re-themes the whole
   app), `--deck-b` is that derivation's hue-guard result, and `--zoom` is the
   size scale.

   They are excluded from the exported document on purpose. If the export
   pinned them, copying the default and pasting it straight back would freeze
   the accent picker at champagne — the user would change the accent and see
   nothing move, which is exactly the "it feels broken" failure this feature is
   supposed to avoid. They remain *valid* keys, so a theme that really wants to
   hand-tune the ramp can state them and win; the settings panel then says so
   rather than leaving a dead control on screen. */
export const DERIVED_TOKENS: ReadonlySet<string> = new Set([
  "accent",
  "accent-rgb",
  "accent-hi",
  "accent-hi-rgb",
  "accent-press",
  "accent-deep",
  "accent-deep-rgb",
  "accent-ink",
  "deck-b",
  "deck-b-rgb",
  "zoom",
]);

/* ── group and prose metadata ─────────────────────────────────────────────
   Longest matching prefix wins, so `--eq-node-fill` lands in "eq" and not in
   the generic bucket. Order within `GROUP_ORDER` is the order the export
   writes its sections in, which is the order a reader wants them: the things
   that change the character of the app first, the fine tuning last. */

const GROUP_ORDER = [
  "surfaces",
  "text",
  "accent",
  "decks",
  "meter-scale",
  "hairlines",
  "fills",
  "type",
  "geometry",
  "motion",
  "waveform",
  "meters",
  "loudness",
  "correlation",
  "eq",
  "chrome",
  "other",
] as const;

const GROUP_BY_PREFIX: ReadonlyArray<readonly [string, string]> = [
  ["ink-on-accent", "accent"],
  ["ink-", "surfaces"],
  ["surface-", "surfaces"],
  ["scrim", "surfaces"],
  ["bg-", "surfaces"],
  ["stage-glow", "surfaces"],
  ["well", "surfaces"],
  ["text-", "text"],
  ["accent", "accent"],
  ["gold", "accent"],
  ["selection", "accent"],
  ["deck-", "decks"],
  ["lane-", "waveform"],
  ["m-", "meter-scale"],
  ["hairline", "hairlines"],
  ["fill-", "fills"],
  ["masked-", "fills"],
  ["font-", "type"],
  ["ls-", "type"],
  ["r-", "geometry"],
  ["titlebar-", "geometry"],
  ["transport-", "geometry"],
  ["meters-w", "geometry"],
  ["mac-inset", "geometry"],
  ["ease", "motion"],
  ["t-", "motion"],
  ["wf-", "waveform"],
  ["loop-", "waveform"],
  ["align-", "waveform"],
  ["tip-", "waveform"],
  ["mt-", "meters"],
  ["lu-", "loudness"],
  ["corr-", "correlation"],
  ["eq-", "eq"],
  ["solo-", "eq"],
  ["winctl-", "chrome"],
  ["shadow-", "chrome"],
  ["select-arrow", "chrome"],
];

/** One line per token, for the tokens where the name is not the whole story. */
const DOCS: Readonly<Record<string, string>> = {
  "ink-900": "the app background — the deepest surface, everything sits on it",
  "ink-850": "transport / rail background",
  "ink-800": "raised surface",
  "ink-700": "the highest surface",
  "surface-panel": "floating panels (settings, playlist overlay)",
  "surface-menu": "menus",
  "surface-toast": "toasts",
  "surface-card": "cards",
  scrim: "the wash behind a modal",
  "bg-app": "the whole app background, layered: two glows over --ink-900",
  "bg-eq": "the EQ window background",
  "well-canvas": "the plate a canvas sits on",
  "text-hi": "primary text — the pair that decides whether the app is readable",
  "text-mid": "secondary text",
  "text-lo": "labels, axis ticks",
  "text-faint": "the quietest ink in the app",
  "ink-on-accent-dark": "text printed on a light accent fill",
  "ink-on-accent-light": "text printed on a dark accent fill",
  "deck-b-src": "deck B's steel; the runtime rotates it away if the accent invades its hue",
  "deck-b-ink": "text on a deck B fill",
  "m-safe": "meter scale: below -18 dB",
  "m-warn": "meter scale: -18 to -6 dB",
  "m-hot": "meter scale: -6 to -1 dB",
  "m-clip": "meter scale: clipping",
  "font-ui": "the UI family stack (the font picker overrides this)",
  "font-num": "the read-out family stack; use a monospace or the numbers dance",
  "font-size-base": "body size; most components state their own",
  "ls-label": "tracking on the all-caps section labels",
  "ls-num": "tracking on tabular read-outs",
  "r-control": "corner radius on buttons, inputs, chips",
  "r-panel": "corner radius on panels and cards",
  "wf-bar-step": "waveform bar pitch in px (bar + gap)",
  "wf-bar-duty": "share of the pitch the bar itself gets; the rest is the gap",
  "wf-outer-a": "waveform peak-envelope ink level",
  "wf-core-a": "waveform RMS-core ink level",
  "wf-scrim": "the wash over the un-played half of a lane",
  "wf-blend": "how waveform layers add up: lighter on a dark theme, multiply on a light one",
  "lane-a-rgb": "the colour deck A's bars are drawn in",
  "lane-b-rgb": "the colour deck B's bars are drawn in",
  "lane-idle-o": "how far the lane that is not audible recedes",
  "eq-curve": "the composite EQ curve",
  "eq-spec-top": "the analyser behind the curve",
  "select-arrow": "the select chevron; a data:image/svg+xml URL and nothing else",
  "winctl-close": "Windows' own close-button red — a platform affordance, not part of the palette",
};

/* ── numeric ranges ───────────────────────────────────────────────────────
   Clamped, not rejected: a theme that asks for a 400 px radius gets 64 px and
   a warning, because the alternative is refusing a document over one silly
   number. The bounds are "still a working audio tool", not "still pretty". */

const RANGE_BY_TYPE: Readonly<Record<string, Range>> = {
  number: { min: 0, max: 1 },
  length: { min: 0, max: 200 },
  duration: { min: 0, max: 4000 },
  tracking: { min: -0.2, max: 1 },
};

const RANGE_BY_NAME: Readonly<Record<string, Range>> = {
  "r-control": { min: 0, max: 32 },
  "r-panel": { min: 0, max: 48 },
  "titlebar-h": { min: 24, max: 64 },
  "transport-h": { min: 44, max: 140 },
  "meters-w": { min: 120, max: 420 },
  "mac-inset": { min: 0, max: 160 },
  "font-size-base": { min: 9, max: 22 },
  "wf-bar-step": { min: 2, max: 40 },
  "wf-bar-duty": { min: 0.15, max: 1 },
};

/* ── the CSS reader ───────────────────────────────────────────────────────
   A declaration scanner, not a regex: `--bg-app` is three lines long, holds
   commas and nested parentheses, and `--select-arrow` holds a quoted URL with
   both braces and semicolons inside it. */

interface Block {
  scope: "base" | "dark" | "light";
  body: string;
}

/** Strip `/* … *\/` comments, keeping newlines so nothing else shifts. */
function stripComments(css: string): string {
  let out = "";
  let i = 0;
  while (i < css.length) {
    if (css.startsWith("/*", i)) {
      const end = css.indexOf("*/", i + 2);
      const skipped = css.slice(i, end < 0 ? css.length : end + 2);
      out += skipped.replace(/[^\n]/g, " ");
      i = end < 0 ? css.length : end + 2;
      continue;
    }
    out += css[i];
    i += 1;
  }
  return out;
}

/** The `{ … }` body starting at `from`, honouring nesting and quotes. */
function blockAt(css: string, from: number): string {
  const open = css.indexOf("{", from);
  if (open < 0) return "";
  let depth = 0;
  let quote: string | null = null;
  for (let i = open; i < css.length; i += 1) {
    const c = css[i];
    if (quote) {
      if (c === quote) quote = null;
      continue;
    }
    if (c === '"' || c === "'") quote = c;
    else if (c === "{") depth += 1;
    else if (c === "}") {
      depth -= 1;
      if (depth === 0) return css.slice(open + 1, i);
    }
  }
  return "";
}

/** `--name: value` pairs, in source order. */
function declarations(body: string): Array<[string, string]> {
  const out: Array<[string, string]> = [];
  let depth = 0;
  let quote: string | null = null;
  let start = 0;
  const push = (chunk: string): void => {
    const text = chunk.trim();
    if (!text.startsWith("--")) return;
    const colon = text.indexOf(":");
    if (colon < 0) return;
    const name = text.slice(2, colon).trim();
    const value = text.slice(colon + 1).replace(/\s+/g, " ").trim();
    if (name && value) out.push([name, value]);
  };
  for (let i = 0; i < body.length; i += 1) {
    const c = body[i];
    if (quote) {
      if (c === quote) quote = null;
      continue;
    }
    if (c === '"' || c === "'") quote = c;
    else if (c === "(") depth += 1;
    else if (c === ")") depth -= 1;
    else if (c === ";" && depth === 0) {
      push(body.slice(start, i));
      start = i + 1;
    }
  }
  push(body.slice(start));
  return out;
}

function readBlocks(css: string): Block[] {
  const clean = stripComments(css);
  const blocks: Block[] = [];
  const find = (selector: string, scope: Block["scope"]): void => {
    const at = clean.indexOf(selector);
    if (at < 0) return;
    blocks.push({ scope, body: blockAt(clean, at) });
  };
  // The base block is the first `:root {` — the dark block's selector is
  // `:root,\n:root[data-theme="dark"]`, which starts with `:root,`.
  const baseAt = clean.search(/:root\s*\{/);
  if (baseAt >= 0) blocks.push({ scope: "base", body: blockAt(clean, baseAt) });
  find(':root,\n:root[data-theme="dark"]', "dark");
  find(':root[data-theme="light"]', "light");
  return blocks;
}

/* ── type inference ───────────────────────────────────────────────────────── */

export const BLEND_MODES = [
  "normal",
  "multiply",
  "screen",
  "overlay",
  "darken",
  "lighten",
  "color-dodge",
  "color-burn",
  "hard-light",
  "soft-light",
  "difference",
  "exclusion",
  "hue",
  "saturation",
  "color",
  "luminosity",
  // canvas-only, and the dark theme's own value
  "lighter",
  "source-over",
] as const;

const EASING_KEYWORDS = ["linear", "ease", "ease-in", "ease-out", "ease-in-out", "step-end"];

const FONT_STACK_TOKENS = new Set(["font-ui", "font-num"]);

function inferType(name: string, value: string): TokenType {
  if (name.endsWith("-rgb")) return "rgb";
  if (FONT_STACK_TOKENS.has(name)) return "font";
  if (name.startsWith("ls-")) return "tracking";
  if (value.startsWith("url(")) return "image";
  if (value.includes("gradient(")) return "background";
  if (name.startsWith("shadow")) return "shadow";
  if (value.startsWith("cubic-bezier") || EASING_KEYWORDS.includes(value)) return "easing";
  if (/^-?\d*\.?\d+ms$/.test(value)) return "duration";
  if (/^-?\d*\.?\d+px$/.test(value)) return "length";
  if (/^-?\d*\.?\d+$/.test(value)) return "number";
  if ((BLEND_MODES as readonly string[]).includes(value)) return "blend";
  return "color";
}

function groupOf(name: string): string {
  let best = "other";
  let bestLen = -1;
  for (const [prefix, group] of GROUP_BY_PREFIX) {
    if (name.startsWith(prefix) && prefix.length > bestLen) {
      best = group;
      bestLen = prefix.length;
    }
  }
  return best;
}

/* ── the catalogue ────────────────────────────────────────────────────────── */

function build(): TokenSpec[] {
  const blocks = readBlocks(tokensCss);
  const specs = new Map<string, TokenSpec>();
  const order: string[] = [];

  for (const block of blocks) {
    for (const [name, value] of declarations(block.body)) {
      let spec = specs.get(name);
      if (!spec) {
        spec = {
          name,
          type: inferType(name, value),
          scope: block.scope === "base" ? "base" : "theme",
          group: groupOf(name),
          doc: DOCS[name],
        };
        const range = RANGE_BY_NAME[name] ?? RANGE_BY_TYPE[spec.type];
        if (range) spec.range = range;
        specs.set(name, spec);
        order.push(name);
      }
      if (block.scope === "base") spec.base = value;
      else if (block.scope === "dark") spec.dark = value;
      else spec.light = value;
    }
  }

  const rank = (g: string): number => {
    const i = (GROUP_ORDER as readonly string[]).indexOf(g);
    return i < 0 ? GROUP_ORDER.length : i;
  };
  return order
    .map((n) => specs.get(n) as TokenSpec)
    .sort((a, b) => rank(a.group) - rank(b.group));
}

/** Every themeable token, in export order. */
export const TOKENS: readonly TokenSpec[] = build();

const BY_NAME = new Map(TOKENS.map((t) => [t.name, t]));

export const tokenSpec = (name: string): TokenSpec | undefined => BY_NAME.get(name);
export const isToken = (name: string): boolean => BY_NAME.has(name);

/** Tokens stated once, outside the two themes. */
export const BASE_TOKENS: readonly TokenSpec[] = TOKENS.filter(
  (t) => t.scope === "base" && !DERIVED_TOKENS.has(t.name),
);

/** Tokens stated per theme, in export order. */
export const THEME_TOKENS: readonly TokenSpec[] = TOKENS.filter(
  (t) => t.scope === "theme" && !DERIVED_TOKENS.has(t.name),
);

/** The groups actually present, in export order. */
export function groupsOf(tokens: readonly TokenSpec[]): string[] {
  const seen: string[] = [];
  for (const t of tokens) if (!seen.includes(t.group)) seen.push(t.group);
  return seen;
}

/** The default value of a token in a resolved theme. */
export function defaultValue(spec: TokenSpec, theme: "dark" | "light"): string {
  if (spec.scope === "base") return spec.base ?? "";
  return (theme === "light" ? spec.light : spec.dark) ?? spec.dark ?? spec.base ?? "";
}
