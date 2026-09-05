/**
 * A tolerant JSON reader that remembers where everything was — SPEC §20.
 *
 * # Why not `JSON.parse`
 *
 * The theme editor's input is a document a human pastes out of a chat window.
 * Two things are certain about it: it will contain `//` comments (because the
 * document Onyx *exports* is full of them, and a model echoes the shape it was
 * given) and it will occasionally contain a trailing comma. `JSON.parse`
 * rejects both, and when it does reject something it says
 * `Unexpected token } in JSON at position 1487`, which is not a position a
 * person can find in a textarea.
 *
 * So: a small recursive-descent reader that
 *
 *  - accepts `//` and block comments, trailing commas, single-quoted strings,
 *    bare identifier keys and a stray ``` fence around the whole thing — every
 *    one of those is a real thing a language model emits;
 *  - reports **line and column** for every failure and, more importantly, for
 *    every *key*, so "`--tect-hi` is not a token" can be pointed at the line it
 *    is on (`src/lib/themedoc.ts`);
 *  - refuses `__proto__`, `constructor` and `prototype` as keys, and never
 *    builds a plain object from user input in the first place — entries stay in
 *    an array, so prototype pollution has nowhere to land;
 *  - is bounded in every direction: input length, nesting depth, entry counts
 *    and string length, so a pathological paste costs a rejection rather than
 *    the window.
 *
 * It is deliberately *not* a general JSON5 implementation: no unquoted values,
 * no hex numbers, no `NaN`/`Infinity`. Everything it accepts, it accepts
 * because a model actually writes it.
 */

export interface JsonPos {
  /** 1-based, so it matches what an editor's gutter says */
  line: number;
  col: number;
}

export interface JsonEntry {
  key: string;
  /** where the key starts, for "unknown key" diagnostics */
  at: JsonPos;
  node: JsonNode;
}

export type JsonNode =
  | ({ kind: "string"; value: string } & JsonPos)
  | ({ kind: "number"; value: number } & JsonPos)
  | ({ kind: "boolean"; value: boolean } & JsonPos)
  | ({ kind: "null" } & JsonPos)
  | ({ kind: "array"; items: JsonNode[] } & JsonPos)
  | ({ kind: "object"; entries: JsonEntry[] } & JsonPos);

export interface JsonFailure {
  message: string;
  at: JsonPos;
}

export type JsonResult = { ok: true; root: JsonNode } | { ok: false; error: JsonFailure };

/** Hard limits. A theme is ~170 short lines; these are orders of magnitude up. */
const MAX_CHARS = 256 * 1024;
const MAX_DEPTH = 16;
const MAX_ENTRIES = 2048;
const MAX_STRING = 4096;

/** Keys that must never reach an object, whatever we do with the result. */
const POISON = new Set(["__proto__", "constructor", "prototype"]);

/**
 * Fenced code blocks: ```json … ``` — what "copy from the chat" produces.
 *
 * Line numbers are the whole diagnostic story (SPEC §20.5), so the fence is
 * **blanked in place** rather than cut out: every line of the document keeps
 * the number it has in the box the user is looking at.
 *
 * The version this replaces trimmed first and then re-added a single newline,
 * which is only right when the fence is the very first character. Paste a
 * fenced reply with a blank line above it — which is what a copy out of a chat
 * transcript gives you — and every reported line was short by however many
 * lines came before the fence, so "line 7" pointed at line 9. Rust trims the
 * document when it stores it (§20.8), so the numbers came *back* right after a
 * save and a reload: the mismatch only existed on the paste it mattered on.
 */
function unfence(text: string): string {
  const lines = text.split("\n");
  const open = lines.findIndex((l) => l.trim() !== "");
  // Prose before the fence is not a fenced block, and guessing would be worse
  // than a syntax error that says where it gave up.
  if (open < 0 || !lines[open].trim().startsWith("```")) return text;
  // A fence with no newline after it is not a block at all; leave it be and let
  // the reader report the position it gave up at.
  if (open === lines.length - 1) return text;
  let close = -1;
  for (let i = lines.length - 1; i > open; i -= 1) {
    if (lines[i].trim().startsWith("```")) {
      close = i;
      break;
    }
  }
  const out = lines.slice();
  out[open] = "";
  // Whatever a model wrote after the closing fence ("I kept the accent warm…")
  // is commentary, not document — dropped, but its lines are kept so anything
  // before it still reports the line it is on.
  if (close >= 0) for (let i = close; i < out.length; i += 1) out[i] = "";
  return out.join("\n");
}

class Reader {
  private readonly s: string;
  private i = 0;
  private line = 1;
  private lineStart = 0;

  constructor(source: string) {
    this.s = source;
  }

  private here(at = this.i): JsonPos {
    return { line: this.line, col: at - this.lineStart + 1 };
  }

  private fail(message: string, at = this.i): never {
    const err: JsonFailure = { message, at: this.here(at) };
    throw err;
  }

  private advance(n = 1): void {
    for (let k = 0; k < n; k += 1) {
      if (this.s[this.i] === "\n") {
        this.line += 1;
        this.lineStart = this.i + 1;
      }
      this.i += 1;
    }
  }

  /** whitespace and comments */
  private skip(): void {
    for (;;) {
      const c = this.s[this.i];
      if (c === undefined) return;
      if (c === " " || c === "\t" || c === "\n" || c === "\r" || c === "\uFEFF") {
        this.advance();
        continue;
      }
      if (c === "/" && this.s[this.i + 1] === "/") {
        while (this.i < this.s.length && this.s[this.i] !== "\n") this.advance();
        continue;
      }
      if (c === "/" && this.s[this.i + 1] === "*") {
        const start = this.i;
        this.advance(2);
        while (this.i < this.s.length && !(this.s[this.i] === "*" && this.s[this.i + 1] === "/")) {
          this.advance();
        }
        if (this.i >= this.s.length) this.fail("this /* comment is never closed", start);
        this.advance(2);
        continue;
      }
      return;
    }
  }

  parse(): JsonNode {
    this.skip();
    const node = this.value(0);
    this.skip();
    if (this.i < this.s.length) {
      this.fail(
        `there is more text after the theme object — a theme is one { … } document`,
      );
    }
    return node;
  }

  private value(depth: number): JsonNode {
    if (depth > MAX_DEPTH) this.fail(`nested more than ${MAX_DEPTH} deep`);
    this.skip();
    const c = this.s[this.i];
    if (c === undefined) this.fail("the document ends before the value does");
    if (c === "{") return this.object(depth);
    if (c === "[") return this.array(depth);
    if (c === '"' || c === "'") {
      const at = this.here();
      return { kind: "string", value: this.string(), ...at };
    }
    if (c === "-" || (c >= "0" && c <= "9")) return this.number();
    if (this.s.startsWith("true", this.i)) {
      const at = this.here();
      this.advance(4);
      return { kind: "boolean", value: true, ...at };
    }
    if (this.s.startsWith("false", this.i)) {
      const at = this.here();
      this.advance(5);
      return { kind: "boolean", value: false, ...at };
    }
    if (this.s.startsWith("null", this.i)) {
      const at = this.here();
      this.advance(4);
      return { kind: "null", ...at };
    }
    this.fail(`\u201C${c}\u201D does not start a JSON value`);
  }

  private object(depth: number): JsonNode {
    const at = this.here();
    this.advance(); // {
    const entries: JsonEntry[] = [];
    const seen = new Set<string>();
    for (;;) {
      this.skip();
      const c = this.s[this.i];
      if (c === undefined) this.fail("this { is never closed");
      if (c === "}") {
        this.advance();
        break;
      }
      const keyAt = this.here();
      const key = c === '"' || c === "'" ? this.string() : this.bareKey();
      if (POISON.has(key)) {
        this.fail(`\u201C${key}\u201D cannot be used as a key`, this.i - key.length);
      }
      if (seen.has(key)) this.fail(`\u201C${key}\u201D appears twice in the same object`);
      seen.add(key);
      if (entries.length >= MAX_ENTRIES) this.fail(`more than ${MAX_ENTRIES} keys in one object`);
      this.skip();
      if (this.s[this.i] !== ":") this.fail(`expected \u201C:\u201D after \u201C${key}\u201D`);
      this.advance();
      const node = this.value(depth + 1);
      entries.push({ key, at: keyAt, node });
      this.skip();
      if (this.s[this.i] === ",") {
        this.advance();
        continue;
      }
      if (this.s[this.i] === "}") {
        this.advance();
        break;
      }
      this.fail(`expected \u201C,\u201D or \u201C}\u201D after the value of \u201C${key}\u201D`);
    }
    return { kind: "object", entries, ...at };
  }

  private array(depth: number): JsonNode {
    const at = this.here();
    this.advance(); // [
    const items: JsonNode[] = [];
    for (;;) {
      this.skip();
      const c = this.s[this.i];
      if (c === undefined) this.fail("this [ is never closed");
      if (c === "]") {
        this.advance();
        break;
      }
      if (items.length >= MAX_ENTRIES) this.fail(`more than ${MAX_ENTRIES} items in one array`);
      items.push(this.value(depth + 1));
      this.skip();
      if (this.s[this.i] === ",") {
        this.advance();
        continue;
      }
      if (this.s[this.i] === "]") {
        this.advance();
        break;
      }
      this.fail("expected \u201C,\u201D or \u201C]\u201D");
    }
    return { kind: "array", items, ...at };
  }

  private bareKey(): string {
    const start = this.i;
    while (/[A-Za-z0-9_$-]/.test(this.s[this.i] ?? "")) this.advance();
    if (this.i === start) this.fail(`\u201C${this.s[this.i]}\u201D does not start a key`);
    return this.s.slice(start, this.i);
  }

  private string(): string {
    const quote = this.s[this.i];
    const open = this.i;
    this.advance();
    let out = "";
    for (;;) {
      const c = this.s[this.i];
      if (c === undefined || c === "\n") this.fail("this string is never closed", open);
      if (c === quote) {
        this.advance();
        return out;
      }
      if (out.length >= MAX_STRING) this.fail(`a string longer than ${MAX_STRING} characters`);
      if (c === "\\") {
        this.advance();
        const e = this.s[this.i];
        if (e === undefined) this.fail("the document ends inside an escape");
        if (e === "u") {
          const hex = this.s.slice(this.i + 1, this.i + 5);
          if (!/^[0-9a-fA-F]{4}$/.test(hex)) this.fail("\\u must be followed by four hex digits");
          out += String.fromCharCode(parseInt(hex, 16));
          this.advance(5);
          continue;
        }
        const simple: Record<string, string> = {
          '"': '"',
          "'": "'",
          "\\": "\\",
          "/": "/",
          b: "\b",
          f: "\f",
          n: "\n",
          r: "\r",
          t: "\t",
        };
        const mapped = simple[e];
        if (mapped === undefined) this.fail(`\u201C\\${e}\u201D is not an escape`);
        out += mapped;
        this.advance();
        continue;
      }
      out += c;
      this.advance();
    }
  }

  private number(): JsonNode {
    const at = this.here();
    const start = this.i;
    if (this.s[this.i] === "-") this.advance();
    while (/[0-9]/.test(this.s[this.i] ?? "")) this.advance();
    if (this.s[this.i] === ".") {
      this.advance();
      while (/[0-9]/.test(this.s[this.i] ?? "")) this.advance();
    }
    if (this.s[this.i] === "e" || this.s[this.i] === "E") {
      this.advance();
      if (this.s[this.i] === "+" || this.s[this.i] === "-") this.advance();
      while (/[0-9]/.test(this.s[this.i] ?? "")) this.advance();
    }
    const text = this.s.slice(start, this.i);
    const value = Number(text);
    // The one number that must never get through: JSON cannot spell NaN, but
    // `1e999` parses to Infinity and would then be clamped into something
    // plausible-looking rather than refused.
    if (!Number.isFinite(value)) this.fail(`${text} is not a finite number`, start);
    return { kind: "number", value, ...at };
  }
}

/** Read a theme document. Never throws; a failure carries a line and a column. */
export function readJsonc(text: string): JsonResult {
  if (text.length > MAX_CHARS) {
    return {
      ok: false,
      error: {
        message: `this is ${Math.round(text.length / 1024)} kB of text; a theme is a few kB`,
        at: { line: 1, col: 1 },
      },
    };
  }
  try {
    return { ok: true, root: new Reader(unfence(text)).parse() };
  } catch (e) {
    const failure = e as Partial<JsonFailure>;
    if (failure && typeof failure.message === "string" && failure.at) {
      return { ok: false, error: { message: failure.message, at: failure.at } };
    }
    /* A real exception (a bug in this reader) must not look like a syntax
       error in the user's document. */
    return {
      ok: false,
      error: { message: `could not read the document: ${String(e)}`, at: { line: 1, col: 1 } },
    };
  }
}

/** The entry named `key`, or `undefined`. Entries are a list, never an object. */
export const entryOf = (node: JsonNode, key: string): JsonEntry | undefined =>
  node.kind === "object" ? node.entries.find((e) => e.key === key) : undefined;
