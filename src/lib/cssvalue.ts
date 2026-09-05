/**
 * Typed CSS values for the theme contract (SPEC §20).
 *
 * # The rule this file exists to enforce
 *
 * A pasted theme is hostile input. Nothing a user or an LLM writes is ever
 * concatenated into a stylesheet: every value is **tokenised, checked against a
 * whitelist, converted to typed data and re-emitted from that data**. If a
 * value cannot be re-emitted it is rejected, so the only CSS the app ever
 * applies is CSS this module wrote.
 *
 * That closes the obvious hole — `--text-hi: red; } * { display: none } .x {`
 * cannot survive a tokeniser that has no concept of `;` or `}` — and it also
 * closes the quieter ones: `url(https://…)` phoning home, an `@import`, a font
 * family with a control character in it, `1e999` reaching layout as `Infinity`,
 * a gradient with ten thousand stops, a data URI with a `<script>` in it.
 *
 * # Everything is bounded
 *
 * Length of the source, number of tokens, nesting depth, magnitude of every
 * number, number of font families. A theme cannot be slow to apply, because
 * there is no input for which the work is unbounded.
 *
 * Node runs this file in `scripts/check-theme.mjs`; it must stay free of DOM.
 */

import type { Rgba } from "./color";
import { NAMED_COLORS, clamp, formatRgba, parseCssColor, parseHex } from "./color";
import type { Range, TokenSpec, TokenType } from "./tokens";
import { BLEND_MODES } from "./tokens";

/* ── limits ───────────────────────────────────────────────────────────────── */

/** Per value. `image` gets its own, larger, budget. */
const MAX_VALUE_CHARS = 600;
const MAX_IMAGE_CHARS = 4096;
const MAX_TOKENS = 240;
const MAX_DEPTH = 6;
/** Anything past this is not a design decision. */
const MAX_MAGNITUDE = 10_000;
const MAX_FAMILIES = 12;
const MAX_FAMILY_CHARS = 48;

export interface ValueOk {
  ok: true;
  /** the value to write into the DOM — always emitted by this module */
  css: string;
  /** the colour it resolves to, when that does not depend on another token */
  rgba?: Rgba;
  /** clamped, quietly corrected, or otherwise not exactly what was asked for */
  note?: string;
}

export interface ValueErr {
  ok: false;
  message: string;
}

export type ValueResult = ValueOk | ValueErr;

const err = (message: string): ValueErr => ({ ok: false, message });

/* ── tokeniser ────────────────────────────────────────────────────────────── */

type Tok =
  | { k: "num"; v: number; unit: string }
  | { k: "ident"; v: string }
  | { k: "hash"; v: string }
  | { k: "str"; v: string }
  | { k: "fn"; name: string; args: Tok[] }
  | { k: "," }
  | { k: "/" };

const UNITS = new Set(["", "%", "px", "deg", "turn", "rad", "grad", "em", "rem", "ms", "s"]);

class Lexer {
  private i = 0;
  private count = 0;

  constructor(private readonly src: string) {}

  fail(message: string): never {
    throw new SyntaxError(message);
  }

  private ws(): void {
    while (this.i < this.src.length && /\s/.test(this.src[this.i])) this.i += 1;
  }

  atEnd(): boolean {
    this.ws();
    return this.i >= this.src.length;
  }

  /** One component value, or a separator. */
  next(depth: number): Tok {
    this.ws();
    this.count += 1;
    if (this.count > MAX_TOKENS) this.fail("too many parts in one value");
    if (depth > MAX_DEPTH) this.fail("nested too deeply");
    const c = this.src[this.i];
    if (c === undefined) this.fail("value ended early");
    if (c === ",") {
      this.i += 1;
      return { k: "," };
    }
    if (c === "/") {
      this.i += 1;
      return { k: "/" };
    }
    if (c === '"' || c === "'") return this.string(c);
    if (c === "#") return this.hash();
    if (/[-+.\d]/.test(c) && /[-+.\d]/.test(c) && this.looksNumeric()) return this.number();
    if (/[a-zA-Z-]/.test(c)) return this.identOrFn(depth);
    this.fail(`unexpected "${c}"`);
  }

  private looksNumeric(): boolean {
    return /^[-+]?(\d+\.?\d*|\.\d+)/.test(this.src.slice(this.i));
  }

  private string(quote: string): Tok {
    this.i += 1;
    let out = "";
    while (this.i < this.src.length && this.src[this.i] !== quote) {
      const ch = this.src[this.i];
      // No escapes at all: `\` is how a CSS string smuggles anything.
      if (ch === "\\") this.fail("backslash escapes are not allowed");
      out += ch;
      this.i += 1;
    }
    if (this.src[this.i] !== quote) this.fail("unterminated string");
    this.i += 1;
    return { k: "str", v: out };
  }

  private hash(): Tok {
    const m = /^#([0-9a-fA-F]{3,8})/.exec(this.src.slice(this.i));
    if (!m) this.fail("not a hex colour");
    this.i += m[0].length;
    return { k: "hash", v: m[1].toLowerCase() };
  }

  private number(): Tok {
    const m = /^[-+]?(\d+\.?\d*|\.\d+)([eE][-+]?\d+)?([a-zA-Z%]*)/.exec(this.src.slice(this.i));
    if (!m) this.fail("not a number");
    this.i += m[0].length;
    const v = parseFloat(m[0]);
    if (!Number.isFinite(v)) this.fail(`"${m[0]}" is not a finite number`);
    const unit = (m[3] ?? "").toLowerCase();
    if (!UNITS.has(unit)) this.fail(`unit "${unit}" is not allowed here`);
    if (Math.abs(v) > MAX_MAGNITUDE) this.fail(`${v} is out of range`);
    return { k: "num", v, unit };
  }

  private identOrFn(depth: number): Tok {
    const m = /^-{0,2}[a-zA-Z][a-zA-Z0-9-]*/.exec(this.src.slice(this.i));
    if (!m) this.fail("not a name");
    this.i += m[0].length;
    const name = m[0];
    // Idents keep their case: a font family is one, and "Helvetica Neue" is
    // not "helvetica neue" to a font matcher on a case-sensitive platform.
    if (this.src[this.i] !== "(") return { k: "ident", v: name };
    this.i += 1;
    const args: Tok[] = [];
    for (;;) {
      this.ws();
      if (this.src[this.i] === ")") {
        this.i += 1;
        break;
      }
      if (this.i >= this.src.length) this.fail(`${name}( is never closed`);
      args.push(this.next(depth + 1));
    }
    return { k: "fn", name: name.toLowerCase(), args };
  }
}

/** The whole value as a flat list of component values and separators. */
function lex(src: string): Tok[] {
  const lexer = new Lexer(src);
  const out: Tok[] = [];
  while (!lexer.atEnd()) out.push(lexer.next(0));
  if (out.length === 0) throw new SyntaxError("empty value");
  return out;
}

/* ── emitter ──────────────────────────────────────────────────────────────── */

const fmt = (n: number): string => {
  const r = Math.round(n * 1e6) / 1e6;
  return Object.is(r, -0) ? "0" : String(r);
};

function emit(toks: Tok[]): string {
  let out = "";
  for (const t of toks) {
    if (t.k === ",") {
      out = `${out.trimEnd()}, `;
      continue;
    }
    if (t.k === "/") {
      out += "/ ";
      continue;
    }
    out += `${emitOne(t)} `;
  }
  return out.trim();
}

function emitOne(t: Tok): string {
  switch (t.k) {
    case "num":
      return `${fmt(t.v)}${t.unit}`;
    case "ident":
      return t.v;
    case "hash":
      return `#${t.v}`;
    case "str":
      return `"${t.v}"`;
    case "fn":
      return `${t.name}(${emit(t.args)})`;
    default:
      return "";
  }
}

/* ── whitelists ───────────────────────────────────────────────────────────── */

const COLOR_FNS = new Set(["rgb", "rgba", "hsl", "hsla", "oklch", "var"]);
const GRADIENT_FNS = new Set([
  "linear-gradient",
  "radial-gradient",
  "conic-gradient",
  "repeating-linear-gradient",
  "repeating-radial-gradient",
]);

/** Idents a gradient's geometry may use. */
const GRADIENT_WORDS = new Set([
  "to",
  "at",
  "from",
  "in",
  "top",
  "bottom",
  "left",
  "right",
  "center",
  "circle",
  "ellipse",
  "closest-side",
  "closest-corner",
  "farthest-side",
  "farthest-corner",
  "srgb",
  "oklab",
  "oklch",
  "hue",
  "shorter",
  "longer",
  "increasing",
  "decreasing",
]);

const COLOR_WORDS = new Set(["transparent", "currentcolor", ...NAMED_COLORS.keys()]);

/* ── colour resolution ────────────────────────────────────────────────────── */

/**
 * The literal colour a *checked* token is, or `null` for "cannot tell".
 *
 * The grammar lives in `color.ts` and nowhere else: this function's whole job
 * is to decide whether the token tree in front of it *has* a literal value, and
 * then to hand the re-emitted text to the one parser. When this file had its own
 * copy of the grammar, `oklch()` meant one thing to the validator and something
 * else to the canvas — see the note at the top of `color.ts`.
 */
function toRgba(t: Tok): Rgba | null {
  if (t.k === "hash" || t.k === "ident") return parseCssColor(emitOne(t));
  if (t.k !== "fn") return null;
  // `none` and `from` are legal in a modern colour function and contribute no
  // number; anything else non-numeric (a var(), a nested function) means the
  // value depends on another token and has no literal colour here.
  const args = t.args.filter(
    (a) => !(a.k === "ident" && ["none", "from"].includes(a.v.toLowerCase())),
  );
  if (args.some((a) => a.k !== "num" && a.k !== "," && a.k !== "/")) return null;
  return parseCssColor(emitOne({ ...t, args }));
}

/* ── shared checks ────────────────────────────────────────────────────────── */

interface Ctx {
  /** does this name exist in the catalogue? */
  known(name: string): boolean;
}

/** `var(--token)` — the only var form allowed, and only for a real token. */
function checkVar(t: Extract<Tok, { k: "fn" }>, ctx: Ctx): string | null {
  if (t.args.length !== 1 || t.args[0].k !== "ident") {
    return "var() takes exactly one token name and no fallback";
  }
  const name = t.args[0].v;
  if (!name.startsWith("--")) return `var(${name}) is not a token name — it must start with "--"`;
  if (!ctx.known(name.slice(2))) return `var(${name}) is not a token in this theme`;
  return null;
}

/** A colour component value: hex, name, colour function, or a token reference. */
function checkColor(t: Tok, ctx: Ctx): string | null {
  if (t.k === "hash") {
    return [3, 4, 6, 8].includes(t.v.length) ? null : `#${t.v} is not a 3, 4, 6 or 8 digit hex`;
  }
  if (t.k === "ident") {
    return COLOR_WORDS.has(t.v.toLowerCase()) ? null : `"${t.v}" is not a colour`;
  }
  if (t.k !== "fn") return "not a colour";
  if (!COLOR_FNS.has(t.name)) return `${t.name}() is not allowed in a colour`;
  if (t.name === "var") return checkVar(t, ctx);
  // `rgb(var(--accent-rgb) / 0.4)` is the app's own idiom, so a colour function
  // may hold one var() — of a triplet token — and numbers.
  for (const a of t.args) {
    if (a.k === "num" || a.k === "," || a.k === "/") continue;
    if (a.k === "fn" && a.name === "var") {
      const bad = checkVar(a, ctx);
      if (bad) return bad;
      continue;
    }
    if (a.k === "ident" && ["from", "none"].includes(a.v.toLowerCase())) continue;
    return `${t.name}() cannot contain ${emitOne(a)}`;
  }
  return null;
}

/** Walk anything: gradients, shadows, layered backgrounds. */
function checkTree(t: Tok, ctx: Ctx, words: Set<string>): string | null {
  switch (t.k) {
    case "num":
      return null;
    case "hash":
      return checkColor(t, ctx);
    case "str":
      return "a quoted string is not allowed here";
    case "ident": {
      const word = t.v.toLowerCase();
      return COLOR_WORDS.has(word) || words.has(word) ? null : `"${t.v}" is not allowed here`;
    }
    case "fn": {
      if (t.name === "var") return checkVar(t, ctx);
      if (COLOR_FNS.has(t.name)) return checkColor(t, ctx);
      if (!GRADIENT_FNS.has(t.name)) return `${t.name}() is not allowed here`;
      for (const a of t.args) {
        const bad = checkTree(a, ctx, words);
        if (bad) return bad;
      }
      return null;
    }
    default:
      return null;
  }
}

/* ── per-type parsers ─────────────────────────────────────────────────────── */

function single(toks: Tok[], what: string): Tok | ValueErr {
  if (toks.length !== 1) return err(`expected a single ${what}`);
  return toks[0];
}

function parseNumeric(toks: Tok[], type: TokenType, range: Range | undefined): ValueResult {
  const one = single(toks, "number");
  if ("ok" in one) return one;
  if (one.k !== "num") return err(`expected a number, got "${emitOne(one)}"`);
  const want = { number: "", length: "px", duration: "ms", tracking: "em" }[
    type as "number" | "length" | "duration" | "tracking"
  ];
  let { v, unit } = one;
  let note: string | undefined;
  if (type === "tracking") {
    if (!["", "em", "px", "rem"].includes(unit)) return err(`letter spacing cannot be in ${unit}`);
    if (v === 0) unit = "";
  } else if (type === "duration") {
    if (unit === "s") {
      v *= 1000;
      unit = "ms";
    }
    if (unit !== "ms") return err(`expected a duration in ms, got "${emitOne(one)}"`);
  } else if (unit !== want) {
    if (unit === "" && v === 0) unit = want;
    else return err(`expected a value in ${want || "no unit"}, got "${emitOne(one)}"`);
  }
  if (range) {
    const c = clamp(v, range.min, range.max);
    if (c !== v) {
      note = `${fmt(v)}${unit} is outside ${fmt(range.min)}…${fmt(range.max)}; clamped to ${fmt(c)}${unit}`;
      v = c;
    }
  }
  return { ok: true, css: `${fmt(v)}${unit}`, note };
}

function parseTriplet(toks: Tok[], ctx: Ctx): ValueResult {
  if (toks.length === 1 && toks[0].k === "fn") {
    const bad = checkColor(toks[0], ctx);
    if (bad) return err(bad);
    if (toks[0].name !== "var") return err("a triplet must be three numbers or var(--…-rgb)");
    return { ok: true, css: emit(toks) };
  }
  const nums = toks.filter((t) => t.k !== ",");
  if (nums.length !== 3 || nums.some((t) => t.k !== "num")) {
    return err('expected three 0…255 numbers, like "232 217 160"');
  }
  const parts = (nums as Array<Extract<Tok, { k: "num" }>>).map((n) => n.v);
  const clamped = parts.map((v) => Math.round(clamp(v, 0, 255)));
  const note = clamped.some((v, i) => v !== parts[i]) ? "clamped to 0…255" : undefined;
  const rgba: Rgba = { r: clamped[0], g: clamped[1], b: clamped[2], a: 1 };
  return { ok: true, css: clamped.join(" "), rgba, note };
}

function parseFont(toks: Tok[]): ValueResult {
  const families: string[] = [];
  let current: string[] = [];
  const flush = (): string | null => {
    if (current.length === 0) return "an empty font family";
    const name = current.join(" ");
    if (name.length > MAX_FAMILY_CHARS) return `font family "${name.slice(0, 20)}…" is too long`;
    // Belt and braces: the tokeniser cannot produce these, but this is the one
    // value that reaches the DOM as text a browser will parse as a family list.
    if (!/^[A-Za-z0-9 _-]+$/.test(name)) return `font family "${name}" has characters that are not allowed`;
    families.push(/[^A-Za-z0-9-]/.test(name) ? `"${name}"` : name);
    current = [];
    return null;
  };
  for (const t of toks) {
    if (t.k === ",") {
      const bad = flush();
      if (bad) return err(bad);
      continue;
    }
    if (t.k === "str" || t.k === "ident") {
      current.push(t.k === "str" ? t.v.trim() : t.v);
      continue;
    }
    return err(`"${emitOne(t)}" is not a font family`);
  }
  const bad = flush();
  if (bad) return err(bad);
  if (families.length === 0) return err("a font stack needs at least one family");
  let note: string | undefined;
  if (families.length > MAX_FAMILIES) {
    families.length = MAX_FAMILIES;
    note = `only the first ${MAX_FAMILIES} families were kept`;
  }
  return { ok: true, css: families.join(", "), note };
}

const EASING_WORDS = new Set(["linear", "ease", "ease-in", "ease-out", "ease-in-out", "step-start", "step-end"]);

function parseEasing(toks: Tok[]): ValueResult {
  const one = single(toks, "easing");
  if ("ok" in one) return one;
  if (one.k === "ident") {
    const word = one.v.toLowerCase();
    return EASING_WORDS.has(word) ? { ok: true, css: word } : err(`"${one.v}" is not an easing`);
  }
  if (one.k === "fn" && one.name === "cubic-bezier") {
    const nums = one.args.filter((a): a is Extract<Tok, { k: "num" }> => a.k === "num");
    if (nums.length !== 4 || nums.some((n) => n.unit !== "")) {
      return err("cubic-bezier() takes four plain numbers");
    }
    const [x1, y1, x2, y2] = nums.map((n) => n.v);
    // The x controls are the ones a browser rejects outright.
    const cx = [clamp(x1, 0, 1), clamp(x2, 0, 1)];
    const note = cx[0] !== x1 || cx[1] !== x2 ? "the x controls were clamped to 0…1" : undefined;
    return {
      ok: true,
      css: `cubic-bezier(${[cx[0], clamp(y1, -5, 5), cx[1], clamp(y2, -5, 5)].map(fmt).join(", ")})`,
      note,
    };
  }
  if (one.k === "fn" && one.name === "steps") {
    const n = one.args.find((a) => a.k === "num");
    if (!n || n.k !== "num" || n.v < 1) return err("steps() needs a positive step count");
    return { ok: true, css: `steps(${fmt(Math.min(100, Math.round(n.v)))})` };
  }
  return err(`"${emitOne(one)}" is not an easing`);
}

function parseShadow(toks: Tok[], ctx: Ctx): ValueResult {
  let lengths = 0;
  for (const t of toks) {
    if (t.k === ",") {
      lengths = 0;
      continue;
    }
    if (t.k === "num") {
      lengths += 1;
      if (lengths > 4) return err("a shadow takes at most four lengths");
      if (t.unit !== "px" && !(t.unit === "" && t.v === 0)) return err("shadow lengths are in px");
      continue;
    }
    if (t.k === "ident" && t.v.toLowerCase() === "inset") continue;
    const bad = checkColor(t, ctx);
    if (bad) return err(bad);
  }
  return { ok: true, css: emit(toks) };
}

function parseBackground(toks: Tok[], ctx: Ctx): ValueResult {
  for (const t of toks) {
    if (t.k === "," || t.k === "/") continue;
    const bad = checkTree(t, ctx, GRADIENT_WORDS);
    if (bad) return err(bad);
  }
  // A single flat colour is a perfectly good background; report it so the
  // contrast audit can see the page it is measuring against.
  const rgba = toks.length === 1 ? toRgba(toks[0]) : null;
  return rgba ? { ok: true, css: emit(toks), rgba } : { ok: true, css: emit(toks) };
}

/**
 * The one token that is an image (`--select-arrow`).
 *
 * Only an inline SVG data URL, only from a tiny character set, and only
 * shapes: no `<script>`, no `<foreignObject>`, no `href`, no `on…=` handler,
 * no external reference of any kind. The CSP already forbids remote origins;
 * this is the second lock, because "the CSP will catch it" is not a thing to
 * rely on for input the user pasted from a chat window.
 */
function parseImage(raw: string): ValueResult {
  const m = /^url\(\s*(["']?)([\s\S]*)\1\s*\)$/.exec(raw.trim());
  if (!m) return err('expected url("data:image/svg+xml;utf8,<svg …>")');
  const data = m[2].trim();
  if (!data.startsWith("data:image/svg+xml")) {
    return err("only an inline data:image/svg+xml URL is allowed — no remote images");
  }
  if (data.length > MAX_IMAGE_CHARS) return err("the image is too large");
  if (!/^[\w\-.,:;/?=&%+#'" <>()!*\n]+$/.test(data)) {
    return err("the image has characters that are not allowed");
  }
  const lower = data.toLowerCase();
  for (const bad of ["script", "foreignobject", "href", "onload", "javascript:", "<use", "<image", "&#"]) {
    if (lower.includes(bad)) return err(`the image may not contain "${bad}"`);
  }
  if (!lower.includes("<svg")) return err("the image must be an inline <svg>");
  return { ok: true, css: `url("${data}")` };
}

/* ── the entry point ──────────────────────────────────────────────────────── */

/**
 * Parse one token's value. Returns the CSS this module will write, never the
 * caller's string.
 */
export function parseValue(spec: TokenSpec, raw: unknown, ctx: Ctx): ValueResult {
  if (typeof raw !== "string") return err(`expected a string, got ${typeof raw}`);
  const text = raw.trim();
  if (text === "") return err("empty value");
  const limit = spec.type === "image" ? MAX_IMAGE_CHARS : MAX_VALUE_CHARS;
  if (text.length > limit) return err(`value is too long (${text.length} > ${limit} characters)`);
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/.test(text)) {
    return err("value contains control characters");
  }
  if (spec.type === "image") return parseImage(text);
  for (const forbidden of [";", "{", "}", "@", "\\", "/*", "//", "<", ">"]) {
    if (text.includes(forbidden)) return err(`"${forbidden}" is not allowed in a value`);
  }

  let toks: Tok[];
  try {
    toks = lex(text);
  } catch (e) {
    return err(e instanceof SyntaxError ? e.message : "could not read this value");
  }

  switch (spec.type) {
    case "color": {
      const one = single(toks, "colour");
      if ("ok" in one) return one;
      const bad = checkColor(one, ctx);
      if (bad) return err(bad);
      const rgba = toRgba(one);
      // Re-emit literal colours in a canonical form; keep symbolic ones as
      // written, because their meaning is another token.
      const css = rgba && one.k !== "fn" ? formatRgba(rgba) : emit([one]);
      return rgba ? { ok: true, css, rgba } : { ok: true, css };
    }
    case "rgb":
      return parseTriplet(toks, ctx);
    case "number":
    case "length":
    case "duration":
    case "tracking":
      return parseNumeric(toks, spec.type, spec.range);
    case "easing":
      return parseEasing(toks);
    case "font":
      return parseFont(toks);
    case "background":
      return parseBackground(toks, ctx);
    case "shadow":
      return parseShadow(toks, ctx);
    case "blend": {
      const one = single(toks, "blend mode");
      if ("ok" in one) return one;
      const word = one.k === "ident" ? one.v.toLowerCase() : "";
      if (!(BLEND_MODES as readonly string[]).includes(word)) {
        return err(`"${emitOne(one)}" is not one of: ${BLEND_MODES.join(", ")}`);
      }
      return { ok: true, css: word };
    }
    default:
      return err(`unknown token type "${spec.type as string}"`);
  }
}

/** Exposed for the contrast audit: the literal colour of an already-valid value. */
export function colorOf(css: string): Rgba | null {
  try {
    const toks = lex(css);
    if (toks.length !== 1) return null;
    return toRgba(toks[0]);
  } catch {
    return null;
  }
}

/** Exposed for tests and for the hex field in the settings panel. */
export const normaliseHex = parseHex;
