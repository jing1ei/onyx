/**
 * The theme document — the contract between Onyx and whoever (or whatever) is
 * writing a skin. SPEC §20.
 *
 * # The workflow this serves
 *
 * Copy the current appearance out as a document → hand it to an agent with a
 * sentence about what you want → paste the reply back. That is the whole
 * feature, and it only works if three things are true:
 *
 *  1. **the export is generated from the real defaults**, so what you copy is
 *     what you are actually looking at (`tokens.ts` reads `tokens.css`);
 *  2. **the document explains itself**, so a model editing it blind cannot
 *     guess wrong about a key or a type — hence the header, the per-group
 *     comments and `AGENT_BRIEF`;
 *  3. **a wrong document fails loudly and changes nothing**. Every key is
 *     checked against the catalogue, every value is parsed to a typed thing
 *     and re-emitted by `cssvalue.ts`, and a document with one error is
 *     rejected whole — with the line, the column and a "did you mean". Silence
 *     is the failure mode that makes a feature like this feel broken.
 *
 * # What a document is not
 *
 * It is not CSS, and no part of it is ever treated as CSS text. It is not
 * executable. It cannot name a token that does not exist, reach the network,
 * or set anything outside the token layer.
 *
 * No DOM here: `theme.ts` owns the DOM, this owns the meaning.
 */

import { derivedVars } from "./accent";
import type { Appearance, ResolvedTheme } from "./appearance";
import {
  DEFAULT_APPEARANCE,
  NUM_FONTS,
  SIZE_SCALES,
  THEME_SETTINGS,
  UI_FONTS,
} from "./appearance";
import { parseHex } from "./color";
import type { ContrastFinding } from "./contrast";
import { auditContrast, formatRatio } from "./contrast";
import { parseValue } from "./cssvalue";
import type { JsonNode, JsonPos } from "./jsonc";
import { readJsonc } from "./jsonc";
import type { TokenSpec } from "./tokens";
import {
  BASE_TOKENS,
  DERIVED_TOKENS,
  THEME_TOKENS,
  TOKENS,
  defaultValue,
  groupsOf,
  isToken,
  tokenSpec,
} from "./tokens";

/** The document format's version. Bumped when a key is renamed or removed. */
export const THEME_DOC_VERSION = 1;

const MAX_NAME_CHARS = 64;

/* ── the parsed document ──────────────────────────────────────────────────── */

export interface ThemeDoc {
  /** always `THEME_DOC_VERSION` after parsing, whatever the source said */
  version: number;
  name: string;
  /** the appearance fields the document states, if any */
  appearance: Partial<Appearance>;
  /** token → CSS, as emitted by `cssvalue.ts`. Maps, so no prototype to poison. */
  base: Map<string, string>;
  dark: Map<string, string>;
  light: Map<string, string>;
}

export interface Problem {
  level: "error" | "warning";
  line: number;
  col: number;
  message: string;
  /** "did you mean “text-hi”?" — the difference between a fix and a shrug */
  suggestion?: string;
}

export interface ParseOutcome {
  /** `null` if and only if there is at least one error */
  doc: ThemeDoc | null;
  problems: Problem[];
  /** measured for both themes when the document is valid */
  contrast: Array<ContrastFinding & { theme: ResolvedTheme }>;
}

export const isError = (p: Problem): boolean => p.level === "error";
export const errorsOf = (ps: readonly Problem[]): Problem[] => ps.filter(isError);

export const formatProblem = (p: Problem): string =>
  `line ${p.line}: ${p.message}${p.suggestion ? ` — ${p.suggestion}` : ""}`;

/* ── "did you mean" ───────────────────────────────────────────────────────── */

/** Levenshtein, bounded — the strings here are token names, never prose. */
export function editDistance(a: string, b: string): number {
  if (a === b) return 0;
  if (a.length === 0 || b.length === 0) return Math.max(a.length, b.length);
  let prev = Array.from({ length: b.length + 1 }, (_, i) => i);
  for (let i = 1; i <= a.length; i += 1) {
    const row = [i];
    for (let j = 1; j <= b.length; j += 1) {
      row[j] = Math.min(
        prev[j] + 1,
        row[j - 1] + 1,
        prev[j - 1] + (a[i - 1] === b[j - 1] ? 0 : 1),
      );
    }
    prev = row;
  }
  return prev[b.length];
}

/**
 * The nearest name worth suggesting, or `undefined`.
 *
 * A near miss is a typo; a distant one is a different idea, and suggesting
 * `--m-hot` for `--midi-channel` is worse than saying nothing.
 */
export function nearest(word: string, candidates: Iterable<string>): string | undefined {
  const w = word.toLowerCase();
  let best: string | undefined;
  let bestScore = Infinity;
  for (const c of candidates) {
    const d = editDistance(w, c.toLowerCase());
    if (d < bestScore) {
      bestScore = d;
      best = c;
    }
  }
  if (best === undefined) return undefined;
  const budget = Math.max(2, Math.floor(Math.max(w.length, best.length) * 0.4));
  return bestScore <= budget ? best : undefined;
}

const didYouMean = (word: string, candidates: Iterable<string>): string | undefined => {
  const hit = nearest(word, candidates);
  return hit ? `did you mean \u201C${hit}\u201D?` : undefined;
};

/* ── parsing ──────────────────────────────────────────────────────────────── */

const TOP_KEYS = ["onyx", "version", "name", "appearance", "base", "dark", "light"];
const APPEARANCE_KEYS = ["theme", "accent", "uiFont", "numericFont", "sizeScale"];

class Collector {
  readonly problems: Problem[] = [];

  error(at: JsonPos, message: string, suggestion?: string): void {
    this.problems.push({ level: "error", line: at.line, col: at.col, message, suggestion });
  }

  warn(at: JsonPos, message: string, suggestion?: string): void {
    this.problems.push({ level: "warning", line: at.line, col: at.col, message, suggestion });
  }

  get failed(): boolean {
    return this.problems.some(isError);
  }
}

/** The scalar a token value may be written as: a string, or a bare number. */
function scalarOf(node: JsonNode): string | null {
  if (node.kind === "string") return node.value;
  // `"wf-core-a": 0.96` — a model writes numbers unquoted about half the time,
  // and refusing that would be pedantry with a line number on it.
  if (node.kind === "number") return String(node.value);
  return null;
}

function readTokenBlock(
  node: JsonNode,
  scope: "base" | "dark" | "light",
  out: Map<string, string>,
  c: Collector,
): void {
  if (node.kind !== "object") {
    c.error(node, `\u201C${scope}\u201D must be an object of token → value`);
    return;
  }
  const wantBase = scope === "base";
  for (const entry of node.entries) {
    const key = entry.key.replace(/^--/, "");
    const spec = tokenSpec(key);
    if (!spec) {
      c.error(
        entry.at,
        `\u201C${entry.key}\u201D is not an Onyx token`,
        didYouMean(key, TOKENS.map((t) => t.name)),
      );
      continue;
    }
    if (spec.scope === "base" && !wantBase) {
      c.error(
        entry.at,
        `\u201C${key}\u201D is the same in both themes`,
        `move it into the \u201Cbase\u201D block`,
      );
      continue;
    }
    if (spec.scope === "theme" && wantBase) {
      c.error(
        entry.at,
        `\u201C${key}\u201D is stated per theme`,
        `move it into \u201Cdark\u201D and/or \u201Clight\u201D`,
      );
      continue;
    }
    if (DERIVED_TOKENS.has(key)) {
      c.warn(
        entry.at,
        `\u201C${key}\u201D is normally derived from appearance.accent; ` +
          "stating it here pins it and the accent picker will no longer move it",
      );
    }
    const raw = scalarOf(entry.node);
    if (raw === null) {
      c.error(entry.at, `\u201C${key}\u201D must be a string (or a number)`);
      continue;
    }
    const parsed = parseValue(spec, raw, { known: isToken });
    if (!parsed.ok) {
      c.error(entry.at, `\u201C${key}\u201D: ${parsed.message}`);
      continue;
    }
    if (parsed.note) c.warn(entry.at, `\u201C${key}\u201D: ${parsed.note}`);
    out.set(key, parsed.css);
  }
}

function readAppearance(node: JsonNode, c: Collector): Partial<Appearance> {
  const out: Partial<Appearance> = {};
  if (node.kind !== "object") {
    c.error(node, "\u201Cappearance\u201D must be an object");
    return out;
  }
  for (const entry of node.entries) {
    const value = scalarOf(entry.node);
    if (value === null) {
      c.error(entry.at, `\u201C${entry.key}\u201D must be a string`);
      continue;
    }
    switch (entry.key) {
      case "theme": {
        const t = THEME_SETTINGS.find((x) => x === value);
        if (!t) {
          c.error(
            entry.at,
            `theme must be ${THEME_SETTINGS.join(", ")}`,
            didYouMean(value, THEME_SETTINGS),
          );
          break;
        }
        out.theme = t;
        break;
      }
      case "accent": {
        const hex = parseHex(value);
        if (!hex) {
          c.error(entry.at, `accent must be a hex colour like \u201C#c9a227\u201D`);
          break;
        }
        out.accent = hex;
        break;
      }
      case "sizeScale": {
        const s = SIZE_SCALES.find((x) => x === value);
        if (!s) {
          c.error(
            entry.at,
            `sizeScale must be ${SIZE_SCALES.join(", ")}`,
            didYouMean(value, SIZE_SCALES),
          );
          break;
        }
        out.sizeScale = s;
        break;
      }
      case "uiFont":
      case "numericFont": {
        const list = entry.key === "uiFont" ? UI_FONTS : NUM_FONTS;
        const names = list.map((f) => f.token);
        if (!names.includes(value)) {
          c.error(
            entry.at,
            `${entry.key} must be one of: ${names.join(", ")}`,
            didYouMean(value, names) ??
              `to use a font that is not on the list, set the ` +
                `\u201C${entry.key === "uiFont" ? "font-ui" : "font-num"}\u201D token in \u201Cbase\u201D`,
          );
          break;
        }
        if (entry.key === "uiFont") out.uiFont = value;
        else out.numericFont = value;
        break;
      }
      default:
        c.error(
          entry.at,
          `\u201C${entry.key}\u201D is not an appearance setting`,
          didYouMean(entry.key, APPEARANCE_KEYS),
        );
    }
  }
  return out;
}

/**
 * Read a theme document. Never throws, never applies anything, and returns
 * `doc: null` if there is a single error — the atomicity of SPEC §20 starts
 * here.
 */
export function parseTheme(text: string, current: Appearance = DEFAULT_APPEARANCE): ParseOutcome {
  const c = new Collector();
  const read = readJsonc(text);
  if (!read.ok) {
    c.error(read.error.at, read.error.message);
    return { doc: null, problems: c.problems, contrast: [] };
  }
  const root = read.root;
  if (root.kind !== "object") {
    c.error(root, "a theme is a { … } object");
    return { doc: null, problems: c.problems, contrast: [] };
  }

  const doc: ThemeDoc = {
    version: THEME_DOC_VERSION,
    name: "Untitled",
    appearance: {},
    base: new Map(),
    dark: new Map(),
    light: new Map(),
  };
  let sawMarker = false;
  let sawVersion = false;

  for (const entry of root.entries) {
    switch (entry.key) {
      case "onyx": {
        sawMarker = true;
        if (scalarOf(entry.node) !== "theme") {
          c.error(entry.at, 'the marker must be "onyx": "theme"');
        }
        break;
      }
      case "version": {
        sawVersion = true;
        const v = entry.node.kind === "number" ? entry.node.value : NaN;
        if (!Number.isInteger(v) || v < 1) {
          c.error(entry.at, "version must be a whole number, and 1 is the current one");
          break;
        }
        if (v > THEME_DOC_VERSION) {
          c.error(
            entry.at,
            `this theme says version ${v}, and this build of Onyx understands ${THEME_DOC_VERSION}`,
            "update Onyx, or change the version and check the keys by hand",
          );
        }
        break;
      }
      case "name": {
        const n = scalarOf(entry.node);
        if (n === null) {
          c.error(entry.at, "name must be a string");
          break;
        }
        // eslint-disable-next-line no-control-regex
        const clean = n.replace(/[\u0000-\u001f\u007f]/g, "").trim();
        if (clean.length > MAX_NAME_CHARS) {
          c.warn(entry.at, `the name was shortened to ${MAX_NAME_CHARS} characters`);
        }
        doc.name = clean.slice(0, MAX_NAME_CHARS) || "Untitled";
        break;
      }
      case "appearance":
        doc.appearance = readAppearance(entry.node, c);
        break;
      case "base":
        readTokenBlock(entry.node, "base", doc.base, c);
        break;
      case "dark":
        readTokenBlock(entry.node, "dark", doc.dark, c);
        break;
      case "light":
        readTokenBlock(entry.node, "light", doc.light, c);
        break;
      default:
        c.error(
          entry.at,
          `\u201C${entry.key}\u201D is not part of a theme document`,
          didYouMean(entry.key, TOP_KEYS),
        );
    }
  }

  if (!sawMarker) {
    c.warn(root, 'this document has no "onyx": "theme" marker — is it an Onyx theme?');
  }
  if (!sawVersion) {
    // The migration path in one line: an unversioned document is a v0 one, and
    // v0 → v1 renamed nothing, so it is read as v1 and said so.
    c.warn(root, `no "version" — read as version ${THEME_DOC_VERSION}`);
  }
  if (doc.base.size + doc.dark.size + doc.light.size === 0 && Object.keys(doc.appearance).length === 0) {
    c.warn(root, "this document changes nothing");
  }

  if (c.failed) {
    c.problems.sort((a, b) => a.line - b.line || a.col - b.col);
    return { doc: null, problems: c.problems, contrast: [] };
  }

  const appearance: Appearance = { ...current, ...doc.appearance };
  const contrast: Array<ContrastFinding & { theme: ResolvedTheme }> = [];
  for (const theme of ["dark", "light"] as const) {
    const vars = effectiveVars(doc, theme, appearance);
    for (const finding of auditContrast((name) => vars.get(name))) {
      contrast.push({ ...finding, theme });
      if (!finding.ok && touches(doc, theme)) {
        c.warn(
          { line: 1, col: 1 },
          `${theme}: ${finding.label} is ${formatRatio(finding.ratio)}, ` +
            `below the ${finding.min.toFixed(1)}:1 this pair needs to stay readable`,
        );
      }
    }
  }

  c.problems.sort((a, b) => a.line - b.line || a.col - b.col);
  return { doc, problems: c.problems, contrast };
}

/** Does the document actually say anything about this theme? */
const touches = (doc: ThemeDoc, theme: ResolvedTheme): boolean =>
  doc[theme].size > 0 || doc.base.size > 0 || doc.appearance.accent !== undefined;

/* ── turning a document into custom properties ────────────────────────────── */

const deckSourceFor = (doc: ThemeDoc | null, theme: ResolvedTheme): string => {
  const stated = doc?.[theme].get("deck-b-src") ?? doc?.base.get("deck-b-src");
  const spec = tokenSpec("deck-b-src");
  return parseHex(stated ?? "") ?? (spec ? defaultValue(spec, theme) : "#7fa8b8");
};

/**
 * The *inline* layer: what `theme.ts` writes on `<html>` over the stylesheet.
 *
 * Only the runtime-derived accent family and whatever the document actually
 * states. Everything else stays in `tokens.css`, which is what makes a partial
 * document work, keeps the font picker alive for a document that does not
 * mention fonts, and makes reverting a matter of removing properties.
 */
export function inlineVars(
  doc: ThemeDoc | null,
  theme: ResolvedTheme,
  appearance: Appearance,
): Map<string, string> {
  const out = derivedVars(appearance.accent, theme, deckSourceFor(doc, theme));
  if (doc) {
    for (const [k, v] of doc.base) out.set(k, v);
    for (const [k, v] of doc[theme]) out.set(k, v);
  }
  return out;
}

/**
 * Every token's value under this document — the stylesheet's defaults with the
 * inline layer over the top. Used by the contrast audit and by the export, and
 * never by the DOM.
 */
export function effectiveVars(
  doc: ThemeDoc | null,
  theme: ResolvedTheme,
  appearance: Appearance,
): Map<string, string> {
  const out = new Map<string, string>();
  for (const spec of TOKENS) out.set(spec.name, defaultValue(spec, theme));
  for (const [k, v] of inlineVars(doc, theme, appearance)) out.set(k, v);
  return out;
}

/** The contrast findings for a document as it would actually be applied. */
export function auditTheme(
  doc: ThemeDoc | null,
  theme: ResolvedTheme,
  appearance: Appearance,
): ContrastFinding[] {
  const vars = effectiveVars(doc, theme, appearance);
  return auditContrast((name) => vars.get(name));
}

/* ── exporting ────────────────────────────────────────────────────────────── */

const TYPE_HINT: Readonly<Record<string, string>> = {
  color: "colour",
  rgb: "r g b triplet",
  number: "number",
  length: "px",
  duration: "ms",
  tracking: "em",
  easing: "easing",
  font: "font stack",
  background: "colour or gradient",
  shadow: "shadow",
  blend: "blend mode",
  image: "data:image/svg+xml url",
};

const GROUP_TITLE: Readonly<Record<string, string>> = {
  surfaces: "surfaces — the room everything sits in",
  text: "text — the four ink levels",
  accent: "accent — derived from appearance.accent; these are the fixed parts",
  decks: "decks — A is the accent, B must stay separable from it",
  "meter-scale": "meter scale — the four loudness stops",
  hairlines: "hairlines",
  fills: "overlay fills — hover, rows, inputs",
  type: "type",
  geometry: "geometry",
  motion: "motion",
  waveform: "waveform",
  meters: "level meter",
  loudness: "loudness strip",
  correlation: "correlation meter",
  eq: "EQ curve and analyser",
  chrome: "window chrome",
  other: "other",
};

const quote = (s: string): string => JSON.stringify(s);

function emitBlock(
  specs: readonly TokenSpec[],
  value: (spec: TokenSpec) => string,
  indent: string,
): string {
  const lines: string[] = [];
  for (const group of groupsOf(specs)) {
    lines.push(`${indent}// ${GROUP_TITLE[group] ?? group}`);
    for (const spec of specs.filter((s) => s.group === group)) {
      const hint = spec.type === "color" ? "" : `${TYPE_HINT[spec.type] ?? spec.type}`;
      const parts = [hint, spec.doc].filter(Boolean);
      const comment = parts.length > 0 ? `  // ${parts.join(" \u00B7 ")}` : "";
      lines.push(`${indent}${quote(spec.name)}: ${quote(value(spec))},${comment}`);
    }
    lines.push("");
  }
  while (lines.length > 0 && lines[lines.length - 1] === "") lines.pop();
  return lines.join("\n");
}

/**
 * The header. It is long on purpose: this text is the only documentation an
 * agent editing the document is guaranteed to see, and every rule it states is
 * one the validator actually enforces.
 */
const HEADER = `// ─────────────────────────────────────────────────────────────────────────────
// Onyx theme — a complete appearance, as data.
//
// To restyle Onyx: hand this whole document to an agent with a sentence about
// what you want ("make it a cold graphite studio look"), then paste the reply
// into Settings → Appearance → Theme code → Apply.
//
// The rules, all of them:
//   · The keys are fixed. You may change values, delete keys you do not want
//     to change, reorder freely and write comments. A key that is not in the
//     catalogue is an error with a line number, not a silent no-op.
//   · Values are typed. A colour may be #rrggbb, #rgb, a CSS colour name,
//     rgb()/rgba(), hsl(), oklch(), transparent, another token as
//     var(--token), or rgb(var(--token-rgb) / 0.4). Numbers are plain numbers,
//     lengths are "12px", durations "200ms". Nothing may contain a semicolon,
//     a brace, an @rule or a url() other than the one inline SVG below.
//   · Anything out of range is clamped and reported; anything unparseable
//     rejects the whole document and nothing changes.
//   · JSON with // comments and trailing commas is accepted.
//   · "dark" and "light" are two designed themes, not one inverted. Whichever
//     one you are not looking at is still there — change both, or say in
//     "appearance" which one you mean.
//   · Contrast is measured for real text-on-background pairs before a theme
//     lands. Onyx warns, it does not refuse; keep body text at 4.5:1 unless
//     you mean it.
//
// Escape hatch: Ctrl/Cmd + Alt + Shift + R resets the appearance, from any
// window, even if a theme has made the UI invisible.
// ─────────────────────────────────────────────────────────────────────────────`;

export interface ExportOptions {
  doc?: ThemeDoc | null;
  appearance?: Appearance;
  /** overrides the document's own name */
  name?: string;
}

/**
 * The current appearance as a document. Generated from the catalogue — which
 * is generated from `tokens.css` — so it cannot drift from what is on screen.
 */
export function exportTheme(options: ExportOptions = {}): string {
  const doc = options.doc ?? null;
  const appearance = options.appearance ?? DEFAULT_APPEARANCE;
  const name = options.name ?? doc?.name ?? "Onyx";
  const valueFor = (spec: TokenSpec, theme: ResolvedTheme | null): string => {
    const stated = theme ? doc?.[theme].get(spec.name) : doc?.base.get(spec.name);
    if (stated !== undefined) return stated;
    const raw = defaultValue(spec, theme ?? "dark");
    // Emit what the parser would produce, so export → import → export is a
    // fixed point rather than nearly one.
    const parsed = parseValue(spec, raw, { known: isToken });
    return parsed.ok ? parsed.css : raw;
  };

  return `${HEADER}
{
  "onyx": "theme",
  "version": ${THEME_DOC_VERSION},
  "name": ${quote(name)},

  // How the app starts up. "accent" is one hex: Onyx derives the hover,
  // pressed, deep and ink variants from it, per theme, in OKLCH.
  "appearance": {
    "theme": ${quote(appearance.theme)},          // dark | light | system
    "accent": ${quote(appearance.accent)},
    "uiFont": ${quote(appearance.uiFont)},        // ${UI_FONTS.map((f) => f.token).join(" | ")}
    "numericFont": ${quote(appearance.numericFont)},  // ${NUM_FONTS.map((f) => f.token).join(" | ")}
    "sizeScale": ${quote(appearance.sizeScale)}       // ${SIZE_SCALES.join(" | ")}
  },

  // Stated once — geometry, motion and type are the same in both themes.
  "base": {
${emitBlock(BASE_TOKENS, (s) => valueFor(s, null), "    ")}
  },

  // The dark theme: an obsidian gallery at night.
  "dark": {
${emitBlock(THEME_TOKENS, (s) => valueFor(s, "dark"), "    ")}
  },

  // The light theme: warm alabaster paper under a gallery light. Designed, not
  // inverted — canvas alphas are higher here and the blends are multiplies.
  "light": {
${emitBlock(THEME_TOKENS, (s) => valueFor(s, "light"), "    ")}
  }
}
`;
}

/**
 * The compact contract that goes on the clipboard *with* the theme, so that
 * pasting into any chat window is enough to get a usable answer back.
 */
export const AGENT_BRIEF = `You are editing an Onyx theme document. Onyx is a professional
audio player for mixing and mastering; its two designed themes are "dark"
(obsidian and champagne) and "light" (warm alabaster and bronze).

Return ONE complete JSON document in a single \`\`\`json code block, with no
prose around it. Keep the structure and the keys exactly as given — you may
change values, delete keys that should keep their default, and keep or rewrite
the comments. Do not invent keys: an unknown key is a hard error with a line
number, and the whole document is then rejected.

Value types:
  colour      #rrggbb · #rgb · a CSS colour name · rgb()/rgba() · hsl() ·
              oklch() · transparent · var(--another-token) ·
              rgb(var(--another-token-rgb) / 0.4)
  triplet     "232 217 160" (r g b, 0-255) or var(--x-rgb)
  number      plain, e.g. 0.46 — each has its own range and is clamped
  length      "12px"     duration "200ms"     tracking "0.14em"
  font        a family stack: only letters, digits, spaces and hyphens
  background  a colour, a gradient, or a comma-separated stack of them
  shadow      "0 24px 60px rgba(0,0,0,0.62)", optionally "inset"
  blend       one CSS blend keyword (the waveform uses "lighter" on dark and
              "multiply" on light — additive blending on paper looks like fog)

Rules that will reject your document if you break them: no semicolons, braces,
@rules, backslashes or url() other than the inline SVG chevron; no remote
anything; every var() must name a token that exists in this document.

Aim for at least 4.5:1 contrast between text-hi and ink-900, and 3:1 for
accent-on-background, text-mid and the waveform lanes; Onyx measures these and
warns. Both themes are used — edit both unless told otherwise. The waveform,
meters and EQ curve are read at a glance while someone is working: keep them
legible, keep the four meter stops distinguishable, and keep deck A and deck B
different hues.

The document follows.

`;

/** Theme + brief, for the "Copy for agent" button. */
export const forAgent = (themeText: string): string => `${AGENT_BRIEF}${themeText}`;
