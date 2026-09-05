/**
 * The theme contract, checked against the real modules — SPEC §20.
 *
 * `npm run check:theme`, and part of `npm run build`.
 *
 * There is no test runner in this repo on purpose (see
 * `scripts/check-eq-curve.mjs`): the front end's checks transpile the *actual*
 * TypeScript and exercise it, rather than a transcription of it. Same here.
 * The theme subsystem is deliberately free of DOM — `tokens.ts`, `cssvalue.ts`,
 * `jsonc.ts`, `contrast.ts`, `accent.ts`, `themedoc.ts` — so all of it runs in
 * Node exactly as it runs in the webview.
 *
 * What is pinned here:
 *
 *   · the catalogue and `tokens.css` cover the same tokens, in both directions;
 *   · every real default parses, and parsing is a fixed point;
 *   · export → import → export is identity, and the imported default applies
 *     the same custom properties the stylesheet already has;
 *   · the tolerant JSON reader: comments, trailing commas, fences, line and
 *     column numbers, poison keys, depth and size limits;
 *   · the sanitiser, against the hostile inputs it exists for;
 *   · the contrast maths, against hand-computed WCAG values;
 *   · unknown keys are reported with a line number and a suggestion;
 *   · one theme written the way a language model writes them, applied end to
 *     end (`fixtures/llm-theme.jsonc`);
 *   · and the *other* backend: `src/lib/mock.ts`, driven for real against the
 *     same storage fixture `src-tauri/src/settings.rs` is held to. A preview
 *     whose backend disagrees with the engine proves nothing — that is how a
 *     deck-B bug shipped once (see `check-ab-parity.mjs`).
 */

import { readFileSync, readdirSync } from "node:fs";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const MODULES = [
  "color",
  "accent",
  "appearance",
  "tokens",
  "cssvalue",
  "jsonc",
  "contrast",
  "themedoc",
  "indent",
  "surface",
];

/** Transpile the front end's own sources and import them, types stripped. */
async function load() {
  const ts = (await import("typescript")).default;
  const out = await mkdtemp(path.join(tmpdir(), "onyx-theme-"));
  const css = await readFile(path.join(root, "src/styles/tokens.css"), "utf8");
  await writeFile(path.join(out, "tokenscss.mjs"), `export default ${JSON.stringify(css)};`);
  for (const name of MODULES) {
    const source = await readFile(path.join(root, "src/lib", `${name}.ts`), "utf8");
    let js = ts.transpileModule(source, {
      compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 },
      fileName: `${name}.ts`,
    }).outputText;
    js = js.replaceAll('"../styles/tokens.css?raw"', '"./tokenscss.mjs"');
    for (const dep of MODULES) js = js.replaceAll(`"./${dep}"`, `"./${dep}.mjs"`);
    await writeFile(path.join(out, `${name}.mjs`), js);
  }
  const mods = {};
  for (const name of MODULES) {
    mods[name] = await import(pathToFileURL(path.join(out, `${name}.mjs`)).href);
  }
  await rm(out, { recursive: true, force: true });
  return { mods, css };
}

/* ── a very small test harness ───────────────────────────────────────────── */

let passed = 0;
const failures = [];
let group = "";

const describe = (name) => {
  group = name;
};

function check(name, fn) {
  try {
    fn();
    passed += 1;
  } catch (e) {
    failures.push({ name: `${group} — ${name}`, error: e });
  }
}

/** An alias, for the runtime checks below, which read as a sequence. */
const checkSync = check;

/** The same, for a check that has to await the mock backend. */
async function checkAsync(name, fn) {
  try {
    await fn();
    passed += 1;
  } catch (e) {
    failures.push({ name: `${group} — ${name}`, error: e });
  }
}

function assert(cond, message) {
  if (!cond) throw new Error(message ?? "assertion failed");
}

function equal(actual, expected, message) {
  const a = typeof actual === "string" ? actual : JSON.stringify(actual);
  const b = typeof expected === "string" ? expected : JSON.stringify(expected);
  if (a !== b) throw new Error(`${message ?? "not equal"}\n  actual:   ${a}\n  expected: ${b}`);
}

function near(actual, expected, tolerance, message) {
  if (!(Math.abs(actual - expected) <= tolerance)) {
    throw new Error(`${message ?? "not near"}: ${actual} vs ${expected} ±${tolerance}`);
  }
}

/* ── the checks ──────────────────────────────────────────────────────────── */

const { mods, css } = await load();
const { tokens: T, cssvalue: V, jsonc: J, contrast: C, themedoc: D, appearance: A, color: X } = mods;
const S = mods.surface;
const IND = mods.indent;
const ctx = { known: (n) => T.isToken(n) };
const DEFAULTS = A.DEFAULT_APPEARANCE;

describe("catalogue");

check("every token in tokens.css is in the catalogue, and nothing else is", () => {
  // The other direction of the same coin, read straight out of the sheet: any
  // `--name:` declaration inside a :root block.
  const declared = new Set();
  const body = css.replace(/\/\*[\s\S]*?\*\//g, "");
  for (const m of body.matchAll(/^\s{2}(--[a-z0-9-]+)\s*:/gim)) declared.add(m[1].slice(2));
  const catalogued = new Set(T.TOKENS.map((t) => t.name));
  const missing = [...declared].filter((n) => !catalogued.has(n));
  const extra = [...catalogued].filter((n) => !declared.has(n));
  equal(missing, [], "tokens.css declares tokens the catalogue does not carry");
  equal(extra, [], "the catalogue carries tokens tokens.css does not declare");
  assert(declared.size > 150, `only ${declared.size} tokens found — did the parser break?`);
});

check("the exported document covers every non-derived token, in both themes", () => {
  const text = D.exportTheme();
  const outcome = D.parseTheme(text);
  assert(outcome.doc, `the exported default must parse: ${JSON.stringify(outcome.problems)}`);
  const blocks = [outcome.doc.base, outcome.doc.dark, outcome.doc.light];
  for (const spec of T.TOKENS) {
    if (T.DERIVED_TOKENS.has(spec.name)) {
      assert(
        blocks.every((b) => !b.has(spec.name)),
        `${spec.name} is runtime-derived and must not be exported`,
      );
      continue;
    }
    const where = spec.scope === "base" ? [outcome.doc.base] : [outcome.doc.dark, outcome.doc.light];
    for (const block of where) {
      assert(block.has(spec.name), `${spec.name} is missing from the exported document`);
    }
  }
  // Nothing in the export needs a warning either — a default that warns about
  // itself would teach whoever pastes it to ignore the warnings.
  equal(outcome.problems.map(D.formatProblem), [], "the default document must be quiet");
});

check("tokens.css is written in the form the parser emits", () => {
  // Otherwise `#fff` in the sheet and `#ffffff` from the parser make applying
  // the default document a no-op that still rewrites 170 properties.
  const drift = [];
  for (const spec of T.TOKENS) {
    for (const theme of ["dark", "light"]) {
      const raw = T.defaultValue(spec, theme);
      if (!raw) continue;
      const parsed = V.parseValue(spec, raw, ctx);
      if (parsed.ok && parsed.css !== raw) drift.push(`${spec.name} (${theme}) ${raw} → ${parsed.css}`);
    }
  }
  equal(drift, [], "write these the canonical way in tokens.css");
});

check("every default value parses, and parsing it again changes nothing", () => {
  for (const spec of T.TOKENS) {
    for (const theme of ["dark", "light"]) {
      const raw = T.defaultValue(spec, theme);
      if (!raw) continue;
      const first = V.parseValue(spec, raw, ctx);
      assert(first.ok, `${spec.name} (${theme}) does not parse: ${first.message}`);
      const again = V.parseValue(spec, first.css, ctx);
      assert(again.ok, `${spec.name} (${theme}) does not re-parse: ${again.message}`);
      equal(again.css, first.css, `${spec.name} (${theme}) is not a fixed point`);
    }
  }
});

describe("round trip");

check("export → import → export is identity", () => {
  const once = D.exportTheme();
  const outcome = D.parseTheme(once);
  assert(outcome.doc, "the export must parse");
  const twice = D.exportTheme({ doc: outcome.doc, appearance: DEFAULTS, name: "Onyx" });
  equal(twice, once, "a document changed by being read and written");
});

check("importing the default document changes nothing on screen", () => {
  const outcome = D.parseTheme(D.exportTheme());
  for (const theme of ["dark", "light"]) {
    const withDoc = D.effectiveVars(outcome.doc, theme, DEFAULTS);
    const without = D.effectiveVars(null, theme, DEFAULTS);
    for (const [name, value] of without) {
      equal(withDoc.get(name), value, `${name} (${theme}) moved when the default was re-applied`);
    }
  }
});

check("the default document's inline layer is only the derived accent family", () => {
  const outcome = D.parseTheme(D.exportTheme());
  const vars = D.inlineVars(outcome.doc, "dark", DEFAULTS);
  const derived = [...T.DERIVED_TOKENS].filter((n) => n !== "zoom").sort();
  // Everything the default document states equals the stylesheet, so the only
  // properties that must actually be written are the ones the runtime owns…
  const stated = [...vars.keys()].sort();
  assert(
    derived.every((d) => stated.includes(d)),
    "the accent family must always be written inline",
  );
});

check("the derived champagne family still reproduces the dark identity", () => {
  const vars = D.inlineVars(null, "dark", DEFAULTS);
  equal(vars.get("accent"), "#e8d9a0", "the dark accent moved");
  equal(vars.get("accent-hi"), "#f3e8bf", "the dark hover accent moved");
  equal(vars.get("accent-press"), "#cdb478", "the dark pressed accent moved");
  equal(vars.get("accent-deep"), "#c9a227", "the dark deep accent moved");
  equal(vars.get("deck-b"), "#7fa8b8", "deck B moved");
});

describe("the tolerant reader");

check("comments, trailing commas, fences and single quotes all read", () => {
  const text = [
    "```json",
    "{",
    "  // a line comment",
    "  /* and a block one */",
    "  'onyx': 'theme',",
    "  \"version\": 1,",
    "  \"dark\": { \"text-hi\": \"#ffffff\", },",
    "}",
    "```",
  ].join("\n");
  const outcome = D.parseTheme(text);
  assert(outcome.doc, `should parse: ${JSON.stringify(outcome.problems)}`);
  equal(outcome.doc.dark.get("text-hi"), "#ffffff");
});

check("a syntax error carries the line it is on", () => {
  const text = '{\n  "onyx": "theme",\n  "dark": {\n    "text-hi" "#fff"\n  }\n}';
  const outcome = D.parseTheme(text);
  assert(!outcome.doc, "must be rejected");
  equal(outcome.problems[0].line, 4, "wrong line for the missing colon");
});

check("__proto__ and friends cannot be keys", () => {
  for (const poison of ["__proto__", "constructor", "prototype"]) {
    const out = J.readJsonc(`{ "${poison}": { "x": 1 } }`);
    assert(!out.ok, `${poison} must be refused`);
    assert(out.error.message.includes(poison), `the message should name ${poison}`);
  }
  // …and the parsed document is built out of Maps, so there is nowhere to land
  const outcome = D.parseTheme('{ "onyx": "theme", "dark": { "text-hi": "#fff" } }');
  assert(outcome.doc.dark instanceof Map, "token blocks must be Maps");
  assert(Object.getPrototypeOf({}).polluted === undefined, "the prototype was touched");
});

check("1e999 is not a number", () => {
  const out = J.readJsonc('{ "version": 1e999 }');
  assert(!out.ok, "Infinity must be refused");
});

check("depth, size and count are bounded", () => {
  const deep = `${"[".repeat(40)}1${"]".repeat(40)}`;
  assert(!J.readJsonc(deep).ok, "40 levels of nesting must be refused");
  const huge = `{"name":"${"x".repeat(300 * 1024)}"}`;
  assert(!J.readJsonc(huge).ok, "300 kB must be refused");
});

check("a fenced paste reports the line the user is looking at", () => {
  /* The paste that comes out of a chat transcript has a blank line or two above
     the fence. `unfence` used to trim the document and then re-add a single
     newline, so every reported line was short by whatever came before the
     fence: the editor's "go to line" button jumped two lines past the problem.
     The fence is blanked in place now, so the numbers are the numbers in the
     box — which is also what Rust stores after its own trim (§20.8). */
  const body = [
    "{",
    '  "onyx": "theme",',
    '  "dark": {',
    '    "text-hi" "#fff"',
    "  }",
    "}",
  ];
  const bad = body.findIndex((l) => l.includes('"text-hi"')) + 1; // 1-based, in `body`
  for (const lead of [[], [""], ["", ""], ["   "]]) {
    const lines = [...lead, "```json", ...body, "```", "", "I kept the accent warm."];
    const outcome = D.parseTheme(lines.join("\n"));
    assert(!outcome.doc, "the fixture must be rejected");
    const want = lead.length + 1 + bad; // the lead, the fence, then the body
    equal(
      outcome.problems[0].line,
      want,
      `with ${lead.length} line(s) above the fence the error is on line ${want}, ` +
        `and the editor sends the caret to whatever this says`,
    );
    // The line the caret lands on has to be the line that is wrong.
    equal(lines[outcome.problems[0].line - 1], body[bad - 1], "the reported line is the wrong line");
  }
});

check("a fenced document still parses, and prose after the fence is dropped", () => {
  const text = ["Here you go:", "", "```json", '{ "onyx": "theme" }', "```"].join("\n");
  // Prose *before* the fence is not a fenced block — guessing would be worse.
  assert(!D.parseTheme(text).doc, "a document with a preamble is a syntax error, not a guess");
  const clean = ["", "```jsonc", '{ "onyx": "theme", "name": "Fenced" }', "```", "Enjoy!"].join("\n");
  const outcome = D.parseTheme(clean);
  assert(outcome.doc, `should parse: ${JSON.stringify(outcome.problems)}`);
  equal(outcome.doc.name, "Fenced", "the document inside the fence must survive");
});

describe("the editor's keys");

check("Tab indents a caret and a block, Shift+Tab takes it back", () => {
  /* `Tab` in a textarea moves focus by default, which in a 420-line document is
     the worst possible thing for it to do — and the handler that claimed to
     insert a tab did not handle the key at all. Caret arithmetic is exactly
     what a screenshot cannot check, so it is checked here. */
  const doc = ['{', '  "dark": {', '    "text-hi": "#fff"', "  }", "}"].join("\n");

  // a caret, mid-line
  const caret = IND.indentSelection("abc", 1, 1);
  equal(caret.text, `a${IND.INDENT}bc`, "a caret inserts one indent");
  equal(caret.selectionStart, 1 + IND.INDENT.length, "the caret must follow the text it inserted");

  // a selection spanning three lines indents all three, and stays around them
  const from = doc.indexOf('"dark"');
  const to = doc.indexOf('"#fff"') + 6;
  const block = IND.indentSelection(doc, from, to);
  const lines = block.text.split("\n");
  equal(lines[1], `${IND.INDENT}  "dark": {`, "line 2 must be indented");
  equal(lines[2], `${IND.INDENT}    "text-hi": "#fff"`, "line 3 must be indented");
  equal(lines[0], "{", "a line the selection never touched must not move");
  equal(
    block.text.slice(block.selectionStart, block.selectionEnd).split("\n").length,
    2,
    "the selection must still cover the lines it indented, so Tab can be held",
  );

  // …and Shift+Tab is its inverse
  const back = IND.dedentSelection(block.text, block.selectionStart, block.selectionEnd);
  equal(back.text, doc, "dedent must undo indent exactly");

  // never more than the line has: a dedent may not eat a token
  const flush = IND.dedentSelection('"x": 1', 0, 6);
  equal(flush.text, '"x": 1', "a line with no indentation must be left alone");
  const tabbed = IND.dedentSelection("\t  \"x\": 1", 0, 3);
  equal(tabbed.text, '  "x": 1', "a literal tab counts as one indent");

  // a caret with no selection dedents its own line and keeps its place in it
  const single = IND.dedentSelection('    "x": 1', 8, 8);
  equal(single.text, '  "x": 1', "Shift+Tab with no selection dedents the caret's line");
  equal(single.selectionStart, 6, "the caret must stay where it was in the line");

  // blank lines are not given trailing whitespace: the document must still
  // match its own export
  const blank = IND.indentSelection("a\n\nb", 0, 3);
  equal(blank.text, `${IND.INDENT}a\n\n${IND.INDENT}b`, "a blank line must not be indented");
});

describe("hostile values");

const hostile = [
  ["red; } * { display: none } .x {", "CSS injection through a colour"],
  ["url(https://example.com/a.png)", "a remote image"],
  ["url(javascript:alert(1))", "a javascript: URL"],
  ["#fff /* */ @import 'x'", "an @rule"],
  ["rgb(255 0 0) !important", "an !important"],
  ["var(--not-a-token)", "a token that does not exist"],
  ["expression(alert(1))", "an old IE expression"],
  ["\\0000effff", "a backslash escape"],
  ["rgb(255,0,0)\u0000", "a NUL byte"],
  ["a".repeat(5000), "5 kB of nonsense"],
];

check("a colour token refuses every one of them", () => {
  const spec = T.tokenSpec("text-hi");
  for (const [value, what] of hostile) {
    const out = V.parseValue(spec, value, ctx);
    assert(!out.ok, `${what} was accepted: ${JSON.stringify(value)}`);
  }
});

check("a font stack refuses everything but families", () => {
  const spec = T.tokenSpec("font-ui");
  for (const bad of [
    'url("evil.woff")',
    "Helvetica; } body { color: red",
    "Segoe UI}",
    "@font-face",
    "Font\u0007Name",
    "x".repeat(200),
  ]) {
    assert(!V.parseValue(spec, bad, ctx).ok, `accepted a bad font stack: ${bad}`);
  }
  const good = V.parseValue(spec, 'Inter, "Helvetica Neue", sans-serif', ctx);
  assert(good.ok, `a real stack must be accepted: ${good.message}`);
  equal(good.css, 'Inter, "Helvetica Neue", sans-serif');
});

check("the one image token takes an inline SVG and nothing else", () => {
  const spec = T.tokenSpec("select-arrow");
  const ok = V.parseValue(
    spec,
    `url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg'><path d='M2 4l3 3'/></svg>")`,
    ctx,
  );
  assert(ok.ok, `a plain inline SVG must be accepted: ${ok.message}`);
  for (const bad of [
    `url("https://example.com/a.svg")`,
    `url("data:image/svg+xml,<svg><script>alert(1)</script></svg>")`,
    `url("data:image/svg+xml,<svg><foreignObject/></svg>")`,
    `url("data:text/html,<b>x</b>")`,
    `url("data:image/svg+xml,<svg><image href='http://x/y'/></svg>")`,
  ]) {
    assert(!V.parseValue(spec, bad, ctx).ok, `accepted a dangerous image: ${bad}`);
  }
});

check("numbers are clamped, not trusted", () => {
  const step = V.parseValue(T.tokenSpec("wf-bar-step"), "9999", ctx);
  assert(step.ok && step.css === "40", `bar step should clamp to 40, got ${step.css}`);
  assert(step.note, "a clamp must be reported");
  const radius = V.parseValue(T.tokenSpec("r-panel"), "-40px", ctx);
  assert(radius.ok && radius.css === "0px", `negative radius should clamp to 0, got ${radius.css}`);
  for (const bad of ["NaN", "Infinity", "1e999", "12", "abc"]) {
    assert(!V.parseValue(T.tokenSpec("r-panel"), bad, ctx).ok, `accepted ${bad} as a length`);
  }
});

check("a triplet is three numbers or a token", () => {
  const spec = T.tokenSpec("lane-a-rgb");
  equal(V.parseValue(spec, "300 -4 12", ctx).css, "255 0 12", "must clamp to 0…255");
  assert(V.parseValue(spec, "var(--accent-rgb)", ctx).ok, "a token reference must work");
  assert(!V.parseValue(spec, "#ff0000", ctx).ok, "a hex is not a triplet");
});

describe("diagnostics");

check("an unknown token is an error with a line and a suggestion", () => {
  const text = ['{', '  "onyx": "theme",', '  "dark": {', '    "tect-hi": "#fff"', "  }", "}"].join(
    "\n",
  );
  const outcome = D.parseTheme(text);
  assert(!outcome.doc, "an unknown key must reject the document");
  const problem = outcome.problems.find((p) => p.level === "error");
  equal(problem.line, 4, "wrong line");
  assert(problem.message.includes("tect-hi"), "the message must quote the key");
  assert(problem.suggestion?.includes("text-hi"), `no suggestion: ${problem.suggestion}`);
});

check("a token in the wrong block says which block it belongs in", () => {
  const outcome = D.parseTheme('{ "onyx": "theme", "dark": { "r-panel": "4px" } }');
  const problem = outcome.problems.find((p) => p.level === "error");
  assert(problem.suggestion.includes("base"), `unhelpful: ${problem.suggestion}`);
  const other = D.parseTheme('{ "onyx": "theme", "base": { "text-hi": "#fff" } }');
  const second = other.problems.find((p) => p.level === "error");
  assert(second.suggestion.includes("dark"), "should point at dark/light");
});

check("nothing is silently dropped", () => {
  const outcome = D.parseTheme('{ "onyx": "theme", "colours": { "text-hi": "#fff" } }');
  assert(!outcome.doc, "an unknown section must be an error");
  assert(outcome.problems.some((p) => p.message.includes("colours")), "must name the section");
});

check("suggestions do not fire for things that are not typos", () => {
  assert(D.nearest("text-hi", ["text-hi"]) === "text-hi");
  assert(D.nearest("tect-hi", ["text-hi", "text-mid"]) === "text-hi");
  assert(D.nearest("midi-channel", ["m-hot", "text-hi"]) === undefined, "too far to suggest");
});

check("a newer document version is refused with an explanation", () => {
  const outcome = D.parseTheme('{ "onyx": "theme", "version": 99, "dark": {} }');
  assert(!outcome.doc, "must be refused");
  const problem = outcome.problems.find((p) => p.level === "error");
  assert(problem.message.includes("99"), "must say which version");
  assert(problem.message.includes(String(D.THEME_DOC_VERSION)), "and which version it does read");
});

check("an unversioned document is read as v1, with a warning", () => {
  const outcome = D.parseTheme('{ "onyx": "theme", "dark": { "text-hi": "#fff" } }');
  assert(outcome.doc, "must still be read");
  equal(outcome.doc.version, D.THEME_DOC_VERSION);
  assert(
    outcome.problems.some((p) => p.level === "warning" && p.message.includes("version")),
    "the migration must be announced",
  );
});

describe("atomicity");

check("one bad value rejects the whole document", () => {
  const text = [
    '{ "onyx": "theme", "dark": {',
    '  "text-hi": "#00ff00",',
    '  "ink-900": "not-a-colour"',
    "} }",
  ].join("\n");
  const outcome = D.parseTheme(text);
  equal(outcome.doc, null, "a document with one bad value must not be usable");
  assert(
    outcome.problems.some((p) => p.message.includes("ink-900")),
    "the bad token must be named",
  );
});

describe("one colour parser");

/* THEMING.md tells the user — and therefore the agent they paste it to — that a
   colour may be any of these. Each one has to mean the same thing to the
   validator, to the contrast audit and to the canvas, because they are three
   readers of the same string; when the canvas had its own hex/rgb-only parser,
   `oklch()` and `hsl()` came back **opaque** there and translucent everywhere
   else (see `color.ts`'s header, and `theme.ts`'s `withAlpha`). */
const COLOUR_SYNTAXES = [
  ["#0af", { r: 0, g: 170, b: 255, a: 1 }],
  ["#0af8", { r: 0, g: 170, b: 255, a: 0.533 }],
  ["#1a2b3c", { r: 26, g: 43, b: 60, a: 1 }],
  ["#1a2b3c80", { r: 26, g: 43, b: 60, a: 0.502 }],
  ["rebeccapurple", { r: 102, g: 51, b: 153, a: 1 }],
  ["transparent", { r: 0, g: 0, b: 0, a: 0 }],
  ["rgb(20 40 60)", { r: 20, g: 40, b: 60, a: 1 }],
  ["rgba(20, 40, 60, 0.4)", { r: 20, g: 40, b: 60, a: 0.4 }],
  ["rgb(20 40 60 / 40%)", { r: 20, g: 40, b: 60, a: 0.4 }],
  ["hsl(210 50% 40%)", { r: 51, g: 102, b: 153, a: 1 }],
  ["hsl(210deg 50% 40% / 0.5)", { r: 51, g: 102, b: 153, a: 0.5 }],
  ["hsla(0, 100%, 50%, 0.25)", { r: 255, g: 0, b: 0, a: 0.25 }],
  ["oklch(0.72 0.15 250)", null],
  ["oklch(72% 0.15 250 / 0.35)", null],
];

check("every colour syntax the docs offer reaches the same rgba", () => {
  for (const [text, want] of COLOUR_SYNTAXES) {
    const got = X.parseCssColor(text);
    assert(got, `color.ts cannot read "${text}"`);
    if (!want) continue;
    for (const ch of ["r", "g", "b"]) near(got[ch], want[ch], 1, `${text} · ${ch}`);
    near(got.a, want.a, 0.005, `${text} · alpha`);
  }
});

check("what the editor validates is what the canvas paints", () => {
  const spec = T.tokenSpec("tip-bg");
  assert(spec && spec.type === "color", "tip-bg must be a colour token for this check");
  for (const [text] of COLOUR_SYNTAXES) {
    const result = V.parseValue(spec, text, ctx);
    assert(result.ok, `the validator refuses "${text}": ${result.message}`);
    // `result.css` is what lands on <html>, so it is what the canvas reads back
    // out of `getComputedStyle`. The canvas has to be able to parse *that*.
    const painted = X.parseCssColor(result.css);
    assert(painted, `the canvas cannot read the emitted "${result.css}" (from "${text}")`);
    const validated = V.colorOf(result.css);
    assert(validated, `the audit cannot read the emitted "${result.css}"`);
    for (const ch of ["r", "g", "b", "a"]) {
      near(painted[ch], validated[ch], 0.005, `${text}: the canvas and the audit disagree on ${ch}`);
    }
  }
});

check("the colour grammar exists exactly once", () => {
  // The consolidation, pinned structurally: three implementations is how they
  // came to disagree, and `Math.cbrt` is the fingerprint of the OKLCH one.
  const lib = readdirSync(path.join(root, "src/lib")).filter((f) => f.endsWith(".ts"));
  const owners = lib.filter((f) =>
    readFileSync(path.join(root, "src/lib", f), "utf8").includes("Math.cbrt"),
  );
  equal(owners, ["color.ts"], "the OKLCH conversion must live in color.ts and nowhere else");
  for (const file of ["cssvalue.ts", "theme.ts", "contrast.ts"]) {
    const src = readFileSync(path.join(root, "src/lib", file), "utf8");
    assert(
      /from "\.\/color"/.test(src),
      `${file} must get its colour grammar from color.ts`,
    );
    assert(
      !/parseInt\([^)]*, ?16\)/.test(src),
      `${file} is parsing hex itself again — that is the drift this check exists for`,
    );
  }
});

describe("contrast");

check("the maths agrees with the WCAG worked examples", () => {
  const white = { r: 255, g: 255, b: 255, a: 1 };
  const black = { r: 0, g: 0, b: 0, a: 1 };
  near(X.contrastRatio(white, black), 21, 0.001, "white on black");
  near(X.contrastRatio(white, white), 1, 0.001, "white on white");
  // #767676 on white is the canonical 4.54:1 boundary case
  near(X.contrastRatio({ r: 118, g: 118, b: 118, a: 1 }, white), 4.54, 0.01, "#767676 on white");
  // a half-transparent white over black composites to #808080-ish
  const composited = X.over({ r: 255, g: 255, b: 255, a: 0.5 }, black);
  near(composited.r, 127.5, 0.01, "compositing");
  near(X.contrastRatio(composited, black), 5.32, 0.05, "50% white on black");
});

check("both designed themes pass their own audit", () => {
  for (const theme of ["dark", "light"]) {
    const findings = D.auditTheme(null, theme, DEFAULTS);
    const bad = findings.filter((f) => !f.ok);
    equal(
      bad.map((f) => `${f.label} ${f.ratio.toFixed(2)}`),
      [],
      `the ${theme} theme fails its own legibility guard`,
    );
    assert(findings.length >= 10, "the audit should measure every pair it can resolve");
  }
});

check("an unreadable theme warns and is still applied", () => {
  const text = [
    '{ "onyx": "theme", "dark": {',
    '  "ink-900": "#101010",',
    '  "text-hi": "#151515"',
    "} }",
  ].join("\n");
  const outcome = D.parseTheme(text);
  assert(outcome.doc, "a low-contrast theme must not be refused");
  const warning = outcome.problems.find((p) => p.message.includes("body text"));
  assert(warning, `expected a contrast warning, got ${JSON.stringify(outcome.problems)}`);
  equal(warning.level, "warning", "contrast must warn, never block");
  const failing = outcome.contrast.filter((f) => f.theme === "dark" && !f.ok);
  assert(failing.length > 0, "the finding list must carry the failure");
});

check("the audit is generated from the catalogue, not written beside it", () => {
  /* SPEC §20.2: the catalogue comes from `tokens.css`. An audit with a
     hand-written pair list stops covering a token the moment it is renamed or
     added — silently, because an unresolvable pair is skipped rather than
     reported. So the coverage is recomputed here, from the catalogue, and
     compared with what `contrast.ts` decided to measure. */
  const wanted = T.TOKENS.filter(
    (t) => t.group === "text" && t.type === "color" && !T.DERIVED_TOKENS.has(t.name),
  ).map((t) => t.name);
  assert(wanted.length >= 4, "the catalogue should have a handful of text inks");
  const audited = new Set(C.contrastTokens());
  const missed = wanted.filter((n) => !audited.has(n));
  equal(missed, [], "text inks in the catalogue that the contrast audit never measures");
  for (const name of wanted) {
    assert(C.auditedByCatalogue(name), `${name} must be audited by the catalogue rule`);
  }

  // Both sides of every pair must still *be* tokens: this is what turns a
  // rename into a failed build instead of an audit that quietly shrinks.
  equal(C.CONTRAST_DRIFT, [], "contrast pairs naming tokens that no longer exist");

  // And the derived accent family is excluded on purpose (§20.2) — auditing a
  // computed token would measure the accent picker, not the theme.
  for (const name of T.DERIVED_TOKENS) {
    assert(!C.auditedByCatalogue(name), `${name} is derived and must not be audited as a text ink`);
  }

  // A renamed token has to move the audit with it, in both directions.
  const findings = D.auditTheme(null, "dark", DEFAULTS);
  for (const name of wanted) {
    assert(
      findings.some((f) => f.fg === name),
      `${name} is in the catalogue but no finding measured it`,
    );
  }
});

check("translucent ink is measured over what is behind it", () => {
  // --text-hi is `rgba(255,255,255,0.92)`; measuring it as opaque white would
  // overstate the ratio, which is the bug this composite exists to avoid.
  const vars = D.effectiveVars(null, "dark", DEFAULTS);
  const ink = C.resolveColor("text-hi", (n) => vars.get(n));
  near(ink.a, 0.92, 0.001, "the alpha must survive resolution");
  const glow = C.resolveColor("stage-glow-a", (n) => vars.get(n));
  near(glow.a, 0.045, 0.0001, "rgb(var(--x) / a) must resolve through the triplet");
});

describe("the escape hatch");

check("garbage in the saved theme falls back to the defaults", () => {
  for (const garbage of ['{"dark":', "\u0000\u0000\u0000", "not json at all", '{"dark":{"x":1}}']) {
    const outcome = D.parseTheme(garbage);
    equal(outcome.doc, null, `garbage must not produce a document: ${garbage}`);
    assert(outcome.problems.length > 0, "and must say why");
    // …and the defaults are still available, unchanged
    const vars = D.effectiveVars(null, "dark", DEFAULTS);
    equal(vars.get("ink-900"), "#0a0a0c", "the fallback must be the designed theme");
  }
});

describe("the LLM workflow");

const llm = await readFile(path.join(root, "src/lib/fixtures/llm-theme.jsonc"), "utf8");

check("a theme written the way a model writes one applies end to end", () => {
  const outcome = D.parseTheme(llm, DEFAULTS);
  const errors = outcome.problems.filter((p) => p.level === "error");
  equal(
    errors.map(D.formatProblem),
    [],
    "the fixture must be accepted exactly as a model would write it",
  );
  assert(outcome.doc, "and produce a document");
  equal(outcome.doc.name, "Cold Graphite");

  // it actually changes the look…
  const before = D.effectiveVars(null, "dark", DEFAULTS);
  const after = D.effectiveVars(outcome.doc, "dark", { ...DEFAULTS, ...outcome.doc.appearance });
  let moved = 0;
  for (const [k, v] of after) if (before.get(k) !== v) moved += 1;
  assert(moved > 30, `only ${moved} tokens changed — that is not a new skin`);

  // …the accent came from the document's own appearance block…
  equal(outcome.doc.appearance.accent, "#4f8fbf", "the fixture sets its own accent");
  assert(after.get("accent") !== before.get("accent"), "the accent family must follow it");

  // …and it is still readable.
  const bad = D.auditTheme(outcome.doc, "dark", { ...DEFAULTS, ...outcome.doc.appearance }).filter(
    (f) => !f.ok,
  );
  equal(bad.map((f) => f.label), [], "the fixture should be a legible theme");
});

check("and it survives a round trip through the exporter", () => {
  const outcome = D.parseTheme(llm, DEFAULTS);
  const appearance = { ...DEFAULTS, ...outcome.doc.appearance };
  const text = D.exportTheme({ doc: outcome.doc, appearance });
  const again = D.parseTheme(text, DEFAULTS);
  assert(again.doc, "the re-export must parse");
  const a = D.effectiveVars(outcome.doc, "dark", appearance);
  const b = D.effectiveVars(again.doc, "dark", appearance);
  for (const [k, v] of a) equal(b.get(k), v, `${k} did not survive the round trip`);
});

describe("the agent brief");

check("copy-for-agent carries the rules and the document", () => {
  const text = D.exportTheme();
  const full = D.forAgent(text);
  assert(full.endsWith(text), "the theme must be at the end, where a model expects it");
  for (const rule of ["json", "unknown key", "4.5:1", "oklch()"]) {
    assert(full.toLowerCase().includes(rule.toLowerCase()), `the brief must mention ${rule}`);
  }
});

check("the exported header states the rules the validator enforces", () => {
  const text = D.exportTheme();
  assert(text.startsWith("//"), "the document must lead with its own instructions");
  for (const rule of ["Onyx theme", "keys are fixed", "clamped", "Contrast", "Ctrl/Cmd"]) {
    assert(text.includes(rule), `the header must mention ${rule}`);
  }
});

/* ── the window surface ──────────────────────────────────────────────────── */

/**
 * The *window* has a colour too: the one the window manager paints under the
 * webview, which is what shows around a decorated window's antialiased corners
 * and for the frames before the webview's first paint. Left untold it is the
 * system's, which on macOS is a light grey — a light rim around obsidian.
 * `src-tauri/src/surface.rs` paints it instead, and `src/lib/surface.ts` decides
 * what colour that is, because a theme document can move the base surface
 * anywhere (SPEC §20) and Rust does not parse documents.
 *
 * These checks are the seam. A *constant* would satisfy nothing here: the value
 * is compared against `effectiveVars()` — the same resolution the contrast audit
 * uses — for both designed themes and for a document that moves them, so the
 * window and the webview cannot drift apart.
 */
describe("the window surface");

check("the surface is the resolved theme's own base colour, not a constant", () => {
  const surfaceOf = (doc, theme, appearance) =>
    S.surfaceHex([D.effectiveVars(doc, theme, appearance).get(S.SURFACE_TOKEN)]);

  const dark = surfaceOf(null, "dark", DEFAULTS);
  const light = surfaceOf(null, "light", DEFAULTS);
  equal(dark, "#0a0a0c", "the dark window surface must be the designed obsidian");
  equal(light, "#efebe1", "the light window surface must be the designed alabaster");
  assert(dark !== light, "one colour for both themes is the same defect twice");

  // …and a theme document takes the window with it. This is the case a
  // hardcoded `#0a0a0c` would fail: the app would be graphite and the rim
  // obsidian, on a window the user themed themselves.
  const doc = D.parseTheme(llm, DEFAULTS).doc;
  const appearance = { ...DEFAULTS, ...doc.appearance };
  equal(surfaceOf(doc, "dark", appearance), "#0b0e11", "the document's dark surface must win");
  equal(surfaceOf(doc, "light", appearance), "#e8ecef", "…and its light one");
  assert(surfaceOf(doc, "dark", appearance) !== dark, "the document must move the window surface");
});

check("what the window is painted with is opaque, whatever grammar stated it", () => {
  // A window surface is what everything else is composited over, so an alpha on
  // it has nothing to blend with; the colour survives and the alpha does not.
  equal(S.surfaceHex(["#abc"]), "#aabbcc", "#rgb must expand");
  equal(S.surfaceHex(["  #0A0A0C  "]), "#0a0a0c", "the value arrives as authored, spaces and all");
  equal(S.surfaceHex(["rgb(10, 10, 12)"]), "#0a0a0c", "a browser reports rgb() for a background");
  equal(S.surfaceHex(["#1a2b3ccc"]), "#1a2b3c", "an eight-digit hex keeps its colour, drops alpha");
  const oklch = S.surfaceHex(["oklch(0.72 0.15 250)"]);
  assert(/^#[0-9a-f]{6}$/.test(oklch), `THEMING.md offers oklch(); it must resolve: ${oklch}`);
  const hsl = X.parseCssColor(S.surfaceHex(["hsl(210 50% 40%)"]));
  near(hsl.a, 1, 1e-9, "a surface is opaque even when the token was not");
});

check("nothing paints the window from a value that is not a colour", () => {
  for (const junk of [null, undefined, "", "   ", "banana", "var(--ink-900)", "transparent"]) {
    equal(S.surfaceHex([junk]), null, `${String(junk)} must not become a window surface`);
  }
  /* `transparent` is what a browser reports for "no background here", which says
     nothing about what the window should be — painting it black would be a
     guess. Candidates are tried in order of authority, so the token behind a
     transparent body is what gets used. */
  equal(S.surfaceHex(["transparent", "#0a0a0c"]), "#0a0a0c", "the next candidate must be tried");
  equal(S.surfaceHex(["#efebe1", "#0a0a0c"]), "#efebe1", "what the body paints outranks the token");
  equal(S.surfaceHex([]), null, "no candidates means no opinion — Rust keeps the designed surface");
});

check("Rust paints the window with the token this module names", () => {
  const rust = readFileSync(path.join(root, "src-tauri/src/surface.rs"), "utf8");
  assert(
    rust.includes(`SURFACE_TOKEN: &str = "--${S.SURFACE_TOKEN}"`),
    `src-tauri/src/surface.rs must name --${S.SURFACE_TOKEN}, the token surface.ts reports`,
  );
  /* The two designed surfaces exist three times over: here, as the values
     `effectiveVars()` resolves out of `tokens.css`; in Rust, which parses the
     same sheet at compile time and keeps a built-in pair for the day the parse
     stops working; and in the two Tauri configs, which are the *only* way the
     main window can be born with a colour at all — it exists before any Rust
     runs. Three statements of one colour is a drift waiting to happen, so read
     all three and compare them rather than grepping for a literal. */
  const constant = (name) => {
    const m = new RegExp(`${name}: Color = Color\\(([^)]*)\\)`).exec(rust);
    assert(m, `surface.rs must declare ${name} as a Color literal`);
    const [r, g, b, a] = m[1].split(",").map((part) => Number(part.trim()));
    equal(a, 255, `${name} must be opaque: a window surface has nothing to blend with`);
    return S.surfaceHex([`rgb(${r}, ${g}, ${b})`]);
  };
  const designed = (themeName) =>
    S.surfaceHex([D.effectiveVars(null, themeName, DEFAULTS).get(S.SURFACE_TOKEN)]);
  equal(constant("FALLBACK_DARK"), designed("dark"), "Rust's dark surface is not the sheet's");
  equal(constant("FALLBACK_LIGHT"), designed("light"), "Rust's light surface is not the sheet's");

  for (const rel of ["src-tauri/tauri.conf.json", "src-tauri/tauri.macos.conf.json"]) {
    const conf = JSON.parse(readFileSync(path.join(root, rel), "utf8"));
    const stated = conf.app.windows[0].backgroundColor;
    assert(stated, `${rel} must give the main window a backgroundColor, or it is born grey`);
    equal(
      S.surfaceHex([stated]),
      designed("dark"),
      `${rel} states a surface the default theme does not use`,
    );
  }
  // Every window, and the two the front end never reports for by name.
  for (const rel of ["src-tauri/src/eqwindow.rs", "src-tauri/src/themewindow.rs"]) {
    const src = readFileSync(path.join(root, rel), "utf8");
    assert(/background_color\(crate::surface::for_new_window/.test(src), `${rel} is born grey`);
  }
});

/* ── mock ↔ engine parity ────────────────────────────────────────────────── */

/**
 * The browser preview has its own backend (`src/lib/mock.ts`). Everything above
 * this line proves things about the *front end*; none of it would notice a mock
 * that stored theme documents by different rules than the engine — and a
 * preview that accepts what the app refuses is a preview that verifies fiction.
 *
 * So the real mock is transpiled and driven here, command by command, against
 * the same fixture `settings.rs` is held to.
 */
function installBrowserGlobals() {
  const store = new Map();
  const win = globalThis;
  win.window = win;
  win.opener = null;
  win.closed = false;
  win.location = { href: "http://localhost/index.html" };
  win.screenX = 0;
  win.screenY = 0;
  win.outerWidth = 1440;
  win.outerHeight = 900;
  win.open = () => null;
  win.sessionStorage = {
    getItem: (k) => (store.has(k) ? store.get(k) : null),
    setItem: (k, v) => void store.set(k, String(v)),
    removeItem: (k) => void store.delete(k),
  };
  win.addEventListener = () => {};
  win.removeEventListener = () => {};
}

async function loadMock() {
  const ts = (await import("typescript")).default;
  const out = await mkdtemp(path.join(tmpdir(), "onyx-mock-"));
  for (const name of ["types", "mock"]) {
    const source = await readFile(path.join(root, "src/lib", `${name}.ts`), "utf8");
    const js = ts.transpileModule(source, {
      compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 },
      fileName: `${name}.ts`,
    }).outputText;
    await writeFile(path.join(out, `${name}.mjs`), js.replaceAll('"./types"', '"./types.mjs"'));
  }
  const mod = await import(pathToFileURL(path.join(out, "mock.mjs")).href);
  await rm(out, { recursive: true, force: true });
  return mod;
}

/** Build a case's input exactly as `settings::tests::fixture_input` builds it. */
function fixtureInput(testCase) {
  if (testCase.build) {
    const { prefix = "", unit, times } = testCase.build;
    return prefix + unit.repeat(times);
  }
  return testCase.text ?? null;
}

describe("mock ↔ engine parity");

installBrowserGlobals();
const mock = await loadMock();
const storage = JSON.parse(
  await readFile(path.join(root, "src-tauri/tests/fixtures/theme_doc_contract.json"), "utf8"),
);

check("the fixture the mock is held to is the one Rust reads", () => {
  const rust = readFileSync(path.join(root, "src-tauri/src/settings.rs"), "utf8");
  assert(
    rust.includes("tests/fixtures/theme_doc_contract.json"),
    "settings.rs no longer reads the storage fixture — the engine half is unchecked",
  );
  assert(
    rust.includes("fn the_theme_doc_storage_contract_holds"),
    "the Rust half of the storage contract is gone",
  );
  assert(storage.cases.length >= 12, "the fixture lost cases");
});

for (const testCase of storage.cases) {
  // Every case is run through the real command, and read back out of the real
  // snapshot: a mock that stored the text somewhere the app never sees would
  // pass a unit test of its validator and still be wrong.
  await checkAsync(`set_theme_doc · ${testCase.name}`, async () => {
    const text = fixtureInput(testCase);
    let stored;
    let threw = null;
    try {
      stored = await mock.invoke("set_theme_doc", { text });
    } catch (e) {
      threw = e;
    }
    if (testCase.rejected) {
      assert(threw, "the mock stored a document the engine refuses");
      const snap = await mock.invoke("app_state");
      assert(
        snap.themeDoc !== text,
        "the mock refused the document and kept it anyway",
      );
      return;
    }
    assert(!threw, `the mock refused a document the engine stores: ${threw?.message}`);
    const want = testCase.stored === "=input" ? text : (testCase.stored ?? null);
    equal(stored, want, "the command returned the wrong text");
    const snap = await mock.invoke("app_state");
    equal(snap.themeDoc, want, "the snapshot disagrees with the command");
  });
}

await checkAsync("reset_appearance clears the document and the appearance", async () => {
  // `commands::reset_appearance_in`: this is the escape hatch, and the preview
  // is where its screenshot is taken.
  await mock.invoke("set_theme_doc", { text: '{ "onyx": "theme" }' });
  await mock.invoke("set_appearance", {
    appearance: {
      theme: "light",
      accent: "#4f8fbf",
      uiFont: "neutral",
      numericFont: "sf-mono",
      sizeScale: "large",
    },
  });
  const before = await mock.invoke("app_state");
  assert(before.themeDoc && before.appearance.theme === "light", "could not set up the reset");

  const appearance = await mock.invoke("reset_appearance");
  equal(appearance, DEFAULTS, "reset must return the designed appearance");
  const after = await mock.invoke("app_state");
  equal(after.themeDoc, null, "the document survived a reset");
  equal(after.appearance, DEFAULTS, "the appearance survived a reset");
});

await checkAsync("the snapshot carries the document to every window", async () => {
  // This is the whole cross-window mechanism: apply in the editor, persist,
  // and the main and EQ windows re-skin off `onyx://state`.
  const text = await readFile(path.join(root, "src/lib/fixtures/llm-theme.jsonc"), "utf8");
  const seen = [];
  const stop = mock.listen("onyx://state", (snap) => seen.push(snap.themeDoc));
  await mock.invoke("set_theme_doc", { text });
  stop();
  assert(seen.length > 0, "applying a theme published no snapshot");
  equal(seen.at(-1), text.trim(), "the broadcast document is not the one applied");
  await mock.invoke("reset_appearance");
});

/* ── the 60 Hz path ──────────────────────────────────────────────────────── */

/**
 * `useFrameEffect`, driven for real — SPEC §5's rule that arriving frames never
 * touch React state, and the defect that rule invited.
 *
 * Every canvas in Onyx paints from one shared rAF loop, and the painter is a
 * closure over the render that created it. `useEffect(() => subscribeFrame(paint),
 * [])` therefore captures the *first* render's width, entry id and blind flag and
 * keeps painting with them; the three surfaces that had noticed did three
 * different things about it (an exhaustive dependency array that rebuilt the loop
 * on every prop change, a hand-written `xRef.current = x` mirror per value, and
 * `useStore.getState()` inside the painter). The hook replaces all three, and
 * this drives it with a stand-in React so the two properties that matter can be
 * asserted rather than reviewed: the painter is always the current render's, and
 * it is attached exactly once.
 *
 * The stand-in is fifty lines of hook bookkeeping, not a second implementation of
 * anything in `src/` — `frame.ts` itself is the module under test, transpiled.
 */
async function loadFrameModule() {
  const ts = (await import("typescript")).default;
  const out = await mkdtemp(path.join(tmpdir(), "onyx-frame-"));
  // A minimal `useRef`/`useEffect`, plus a driver: render, re-render with new
  // props, unmount. Deps are compared the way React compares them.
  await writeFile(
    path.join(out, "react.mjs"),
    `const slots = [];
     let cursor = 0;
     let pending = [];
     export function useRef(initial) {
       const i = cursor++;
       if (slots[i] === undefined) slots[i] = { current: initial };
       return slots[i];
     }
     export function useEffect(fn, deps) {
       const i = cursor++;
       const prev = slots[i];
       const same =
         prev && prev.deps && deps && prev.deps.length === deps.length &&
         deps.every((d, k) => d === prev.deps[k]);
       if (!same) pending.push({ i, fn, deps, prev });
     }
     export function render(component, props) {
       cursor = 0;
       component(props);
       const run = pending;
       pending = [];
       for (const e of run) {
         if (e.prev && typeof e.prev.cleanup === "function") e.prev.cleanup();
         slots[e.i] = { deps: e.deps, cleanup: e.fn() };
       }
     }
     export function unmount() {
       for (const slot of slots) if (slot && typeof slot.cleanup === "function") slot.cleanup();
       slots.length = 0;
     }`,
  );
  // What `frame.ts` imports besides React. Named, so a new dependency on the
  // store or the backend shows up here as a missing export rather than silently.
  await writeFile(path.join(out, "api.mjs"), "export const waveformGet = async () => null;\nexport const listen = async () => () => {};\n");
  await writeFile(path.join(out, "align.mjs"), "export const syncFromEngine = () => {};\n");
  await writeFile(path.join(out, "log.mjs"), "export const logError = () => {};\nexport const logWarn = () => {};\n");
  await writeFile(
    path.join(out, "store.mjs"),
    `export const onSnapshot = () => () => {};
     export const useStore = { getState: () => ({ snapshot: null, waveforms: { a: {}, b: {} } }) };`,
  );
  const source = await readFile(path.join(root, "src/lib/frame.ts"), "utf8");
  let js = ts.transpileModule(source, {
    compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 },
    fileName: "frame.ts",
  }).outputText;
  for (const dep of ["api", "align", "log", "store", "types"]) {
    js = js.replaceAll(`"./${dep}"`, `"./${dep}.mjs"`);
  }
  js = js.replaceAll('"react"', '"./react.mjs"');
  await writeFile(path.join(out, "frame.mjs"), js);
  const mods = {
    frame: await import(pathToFileURL(path.join(out, "frame.mjs")).href),
    react: await import(pathToFileURL(path.join(out, "react.mjs")).href),
  };
  await rm(out, { recursive: true, force: true });
  return mods;
}

/** A hand-cranked animation clock, so a "frame" is a statement, not a wait. */
function installClock() {
  const queued = new Map();
  let next = 1;
  let now = 0;
  globalThis.requestAnimationFrame = (cb) => {
    const id = next++;
    queued.set(id, cb);
    return id;
  };
  globalThis.cancelAnimationFrame = (id) => void queued.delete(id);
  return {
    /** one animation frame, 16.7 ms later */
    tick: () => {
      now += 16.7;
      const due = [...queued.values()];
      queued.clear();
      for (const cb of due) cb(now);
    },
    pending: () => queued.size,
  };
}

describe("the 60 Hz path");

const clock = installClock();
const FR = await loadFrameModule();

await checkAsync("a painter sees the render it belongs to, and subscribes once", async () => {
  const painted = [];
  const attachedBefore = FR.frame.frameAttachments();
  // A lane, reduced to the two things the stale closure got wrong: the size it
  // paints at and the track it paints.
  const Lane = (props) => {
    FR.frame.useFrameEffect((frame) => {
      painted.push({ w: props.w, entryId: props.entryId, position: frame?.transport.positionSecs });
    });
  };

  FR.react.render(Lane, { w: 800, entryId: 1 });
  equal(FR.frame.frameSubscribers(), 1, "one mount, one painter");
  equal(FR.frame.frameAttachments() - attachedBefore, 1, "one mount, one attach");
  FR.frame.frameRef.current = { transport: { positionSecs: 12 } };
  clock.tick();
  equal(painted.at(-1), { w: 800, entryId: 1, position: 12 }, "the first frame");

  // The window is dragged narrower and the user loads another track: one React
  // render, no remount. This is the case the `[]` dependency array got wrong —
  // it kept painting 800 px of track 1 forever.
  FR.react.render(Lane, { w: 420, entryId: 2 });
  FR.frame.frameRef.current = { transport: { positionSecs: 13 } };
  clock.tick();
  equal(
    painted.at(-1),
    { w: 420, entryId: 2, position: 13 },
    "the painter is still painting a previous render's props",
  );

  // …and it did not attach a second time, or tear the loop down to do it: a
  // dependency array is the other way to be correct here and it rebuilds the
  // subscription on every prop change, ten times a second while a file decodes.
  equal(FR.frame.frameSubscribers(), 1, "the surface attached more than once");
  equal(
    FR.frame.frameAttachments() - attachedBefore,
    1,
    "a resize and a track change re-attached the painter to the rAF loop",
  );
  equal(painted.length, 2, "one painter, one call per frame");

  // Unmounting detaches, and the loop stops when the last painter goes.
  FR.react.unmount();
  equal(FR.frame.frameSubscribers(), 0, "unmount must detach the painter");
  clock.tick();
  equal(painted.length, 2, "a detached painter was still called");
  equal(clock.pending(), 0, "the rAF loop is still running with nothing to paint");
});

checkSync("no component subscribes to the frame stream by hand", () => {
  /* The rule the hook exists to make keepable (`frame.ts`'s header): one way to
     attach a painter. `subscribeFrame` is still exported — `frame.ts` uses it
     itself, and so does the hook — but a component reaching for it is how the
     stale closure comes back. */
  const offenders = [];
  const walk = (dir) => {
    for (const entry of readdirSync(path.join(root, dir), { withFileTypes: true })) {
      const rel = `${dir}/${entry.name}`;
      if (entry.isDirectory()) {
        walk(rel);
        continue;
      }
      if (!/\.tsx?$/.test(entry.name) || rel === "src/lib/frame.ts") continue;
      const src = readFileSync(path.join(root, rel), "utf8");
      if (/\bsubscribeFrame(Latest)?\s*\(/.test(src)) offenders.push(rel);
    }
  };
  walk("src");
  equal(offenders, [], "these should use useFrameEffect instead");
});

checkSync("no component mirrors a prop by hand for the frame loop", () => {
  /* The other half of the same rule. Before `useFrameEffect` the way to keep a
     painter's view of `blindActive` or `state` fresh was `xRef.current = x` at
     the top of the component — correct, and one line of ceremony per value
     forever, which is how `EqPanel`'s `blindRef` and `WaveformLane`'s `stateRef`
     came to exist. The hook re-points the subscription at every render, so a
     painter reads the prop directly; a new mirror means someone has gone back to
     the old pattern. Refs with a life of their own (`cfgRef`, `dragRef`,
     `alignRef`) are assigned inside handlers, not unconditionally at render, so
     the shape being looked for is the render-time assignment. */
  const offenders = [];
  const walk = (dir) => {
    for (const entry of readdirSync(path.join(root, dir), { withFileTypes: true })) {
      const rel = `${dir}/${entry.name}`;
      if (entry.isDirectory()) {
        walk(rel);
        continue;
      }
      if (!/\.tsx$/.test(entry.name)) continue;
      const src = readFileSync(path.join(root, rel), "utf8");
      // `const xRef = useRef(x);` immediately followed by `xRef.current = x;`
      const mirror = /const (\w+) = useRef\((\w+)\);\n\s*\1\.current = \2;/.exec(src);
      if (mirror) offenders.push(`${rel} (${mirror[1]})`);
    }
  };
  walk("src");
  equal(offenders, [], "read the prop in the painter instead; useFrameEffect keeps it fresh");
});

checkSync("the frame and canvas diagnostics are exposed to the harness", () => {
  /* The stale-closure defect was invisible in the DOM: a canvas painting the
     first window size and the first track looks like a correct picture, so a
     screenshot could not fail on it. `frame.ts` counts attachments and
     `canvas.ts` records what each painter believed the geometry was and how many
     times it has painted; `lib/diag.ts` publishes all three, and
     `scripts/shots.mjs` asserts them across a resize and a track change. If any
     of that stops being reachable the harness goes back to eyeballing PNGs. */
  const frameSrc = readFileSync(path.join(root, "src/lib/frame.ts"), "utf8");
  for (const name of ["frameSubscribers", "frameAttachments"]) {
    assert(new RegExp(`export const ${name}`).test(frameSrc), `frame.ts no longer exports ${name}`);
  }
  const canvasSrc = readFileSync(path.join(root, "src/lib/canvas.ts"), "utf8");
  for (const name of ["paintedSurface", "paintCount"]) {
    assert(new RegExp(`export const ${name}`).test(canvasSrc), `canvas.ts no longer exports ${name}`);
  }
  const diagSrc = readFileSync(path.join(root, "src/lib/diag.ts"), "utf8");
  assert(/__onyxDiag/.test(diagSrc), "lib/diag.ts no longer publishes window.__onyxDiag");
  for (const rel of ["src/main.tsx", "src/eq/main.tsx"]) {
    const entry = readFileSync(path.join(root, rel), "utf8");
    assert(/installDiagnosticsBridge\(\)/.test(entry), `${rel} does not install the diagnostics bridge`);
    // …and only in the preview: a shipped window must expose nothing.
    const guarded = /if \(MOCK\) \{[^}]*installDiagnosticsBridge\(\)/s.test(entry);
    assert(guarded, `${rel} installs the diagnostics bridge outside the MOCK guard`);
  }
});

/* ── the runtime, end to end ─────────────────────────────────────────────── */

/**
 * Everything above proves things about *pure* modules. The claims that matter
 * most to a user, though, are about the running app: an apply is all-or-nothing
 * against a live DOM, a corrupt saved theme comes up as Onyx rather than as a
 * white screen, the reset chord works from a window whose UI is invisible, and
 * a failed persist puts the previous skin back.
 *
 * So the real stack is loaded here — `theme.ts`, `themeio.ts`, `api.ts` with
 * the mock flag on, and `mock.ts` behind it — over a DOM small enough to fit on
 * a screen but real enough to be written to and read back. Nothing is
 * transcribed: these are the modules the app ships, and the assertions are
 * about state (`<html>`'s custom properties, the stored document), not pixels.
 */

/** The parts of a browser `theme.ts` actually touches, and nothing else. */
function installDom() {
  const props = new Map();
  const keys = [];
  const root = {
    dataset: {},
    style: {
      setProperty: (k, v) => void props.set(k, String(v)),
      removeProperty: (k) => void props.delete(k),
      getPropertyValue: (k) => props.get(k) ?? "",
    },
  };
  const local = new Map();
  const win = globalThis;
  win.document = { documentElement: root, addEventListener: () => {}, hasFocus: () => true };
  win.localStorage = {
    getItem: (k) => (local.has(k) ? local.get(k) : null),
    setItem: (k, v) => void local.set(k, String(v)),
    removeItem: (k) => void local.delete(k),
  };
  win.innerWidth = 1440;
  win.innerHeight = 900;
  win.matchMedia = () => ({ matches: false, addEventListener: () => {} });
  /* Custom properties are substituted at computed-value time and otherwise kept
     as authored, so a browser hands the canvas back exactly the string the theme
     wrote — `oklch(0.72 0.15 250)`, not an rgb() the engine helpfully resolved.
     That is the whole reason `theme.ts` needs a colour parser at all, so the
     stand-in reads the properties straight back. */
  win.getComputedStyle = () => ({ getPropertyValue: (k) => props.get(k) ?? "" });
  win.setTimeout = globalThis.setTimeout;
  win.addEventListener = (type, fn) => keys.push({ type, fn });
  win.removeEventListener = () => {};
  return {
    props,
    root,
    /** Fire a key at the capture-phase listeners, the way a webview would. */
    key: (init) => {
      const e = { preventDefault() {}, stopPropagation() {}, ...init };
      for (const k of keys) if (k.type === "keydown") k.fn(e);
    },
    localStorage: local,
  };
}

/**
 * Transpile into the repo's own `node_modules/.cache` rather than `/tmp`, so
 * that `zustand` and `@tauri-apps/api` resolve exactly as they do in the app —
 * the alternative is stubbing them, and a stub is a second implementation.
 */
async function loadRuntime() {
  const ts = (await import("typescript")).default;
  const out = path.join(root, "node_modules/.cache/onyx-check-theme");
  await rm(out, { recursive: true, force: true });
  await mkdir(out, { recursive: true });
  const cssText = await readFile(path.join(root, "src/styles/tokens.css"), "utf8");
  await writeFile(path.join(out, "tokenscss.mjs"), `export default ${JSON.stringify(cssText)};`);
  const names = [
    "types", "log", "store", "api", "mock", "color", "accent", "appearance",
    "tokens", "cssvalue", "jsonc", "contrast", "themedoc", "theme", "themeio",
    "surface",
  ];
  for (const name of names) {
    const source = await readFile(path.join(root, "src/lib", `${name}.ts`), "utf8");
    let js = ts.transpileModule(source, {
      compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 },
      fileName: `${name}.ts`,
    }).outputText;
    js = js.replaceAll('"../styles/tokens.css?raw"', '"./tokenscss.mjs"');
    for (const dep of names) js = js.replaceAll(`"./${dep}"`, `"./${dep}.mjs"`);
    // The two build-time constants Vite would substitute.
    js = js.replaceAll("__ONYX_MOCK__", "true").replaceAll("import.meta.env.DEV", "false");
    await writeFile(path.join(out, `${name}.mjs`), js);
  }
  const load = async (name) => import(pathToFileURL(path.join(out, `${name}.mjs`)).href);
  return { load, cleanup: () => rm(out, { recursive: true, force: true }) };
}

describe("the runtime");

const dom = installDom();
const runtime = await loadRuntime();
const RT = {
  theme: await runtime.load("theme"),
  io: await runtime.load("themeio"),
  api: await runtime.load("api"),
};

/** What `<html>` is actually wearing right now. */
const painted = () => new Map(dom.props);

RT.theme.initAppearance();

await checkAsync("an LLM's theme lands on the DOM and in the backend", async () => {
  const before = painted();
  const result = await RT.io.applyTheme(llm);
  assert(result.applied, `the fixture must apply: ${JSON.stringify(result.outcome.problems)}`);

  const after = painted();
  let moved = 0;
  for (const [k, v] of after) if (before.get(k) !== v) moved += 1;
  assert(moved > 30, `only ${moved} custom properties changed — that is not a new skin`);
  equal(dom.root.dataset.theme, "dark", "the resolved theme must be set on <html>");

  // …and the engine has it, byte for byte, ready for the next launch.
  const snap = await RT.api.appState();
  equal(snap.themeDoc, llm.trim(), "the backend did not store the document");
  equal(snap.appearance.accent, "#4f8fbf", "the document's own accent did not persist");
});

await checkAsync("one bad value changes nothing at all", async () => {
  // Atomic apply (SPEC §20): not "most of the theme", not "the good half".
  const before = painted();
  const broken = llm.replace('"onyx": "theme"', '"onyx": "theme",\n  "dark": { "ink-900": "banana" }');
  const result = await RT.io.applyTheme(broken);
  assert(!result.applied, "a document with a bad value must not apply");
  equal(painted(), before, "the DOM moved for a document that was refused");
  const snap = await RT.api.appState();
  equal(snap.themeDoc, llm.trim(), "the stored document changed for a refused apply");
});

await checkAsync("a document too large to store puts the previous skin back", async () => {
  // The parser's limit is characters and the engine's is bytes, so a document
  // of em dashes can be valid and unstorable at once — which is the only way
  // to reach the rollback path without breaking something on purpose.
  const before = painted();
  // Built off the exported default rather than the fenced fixture: a comment
  // in front of a ``` fence is not a fenced block any more, and this case is
  // about the size limit, not about fences.
  const bulky = `// ${"\u2014".repeat(120 * 1024)}\n${D.exportTheme()}`;
  assert(D.parseTheme(bulky).doc, "the bulky document must be *valid*, only unstorable");
  let threw = null;
  try {
    await RT.io.applyTheme(bulky);
  } catch (e) {
    threw = e;
  }
  assert(threw, "the backend must refuse a document it cannot store");
  equal(painted(), before, "a failed persist left its half-applied theme on screen");
  const snap = await RT.api.appState();
  equal(snap.themeDoc, llm.trim(), "the stored document changed");
});

await checkAsync("the reset chord works with no backend and with one", async () => {
  // The escape hatch (SPEC §20). Bound at capture on `window`, so a focused
  // textarea cannot eat it, and it repaints locally *before* the round trip.
  assert(RT.theme.isResetChord({ ctrlKey: true, altKey: true, shiftKey: true, key: "R" }));
  assert(!RT.theme.isResetChord({ ctrlKey: true, shiftKey: true, key: "R" }), "Alt is required");

  let hook = 0;
  RT.theme.setResetHook(() => {
    hook += 1;
    void RT.api.resetAppearance();
  });
  dom.key({ ctrlKey: true, altKey: true, shiftKey: true, key: "R" });
  equal(hook, 1, "the chord did not reach the persist hook");
  equal(RT.theme.currentThemeText(), null, "the document survived the chord");
  equal(RT.theme.currentAppearance().accent, DEFAULTS.accent, "the accent survived the chord");

  // The local half must not depend on the hook at all: this is the path a
  // window with a wedged backend takes.
  await RT.io.applyTheme(llm);
  RT.theme.setResetHook(null);
  dom.key({ ctrlKey: true, altKey: true, shiftKey: true, key: "R" });
  equal(RT.theme.currentThemeText(), null, "the chord needs a hook to work — it must not");
  const vars = painted();
  equal(vars.get("--accent"), "#e8d9a0", "the champagne accent did not come back");
});

await checkAsync("a corrupt saved theme comes up as Onyx, with a reason", async () => {
  // The white-screen case: `settings.json` holds something the front end cannot
  // read. It must fall back to the designed themes *and* say so, on a line.
  dom.localStorage.set(
    "onyx.appearance",
    JSON.stringify({ ...DEFAULTS, doc: '{\n  "onyx": "theme",\n  "dark": { "tect-hi": "#fff" }\n}' }),
  );
  RT.theme.initAppearance();
  const why = RT.theme.takeThemeDocProblem();
  assert(why, "a corrupt saved theme must be reported");
  assert(/line \d+/.test(why), `the notice must carry a line number: ${why}`);
  assert(why.includes("built-in appearance"), `the notice must say what happened: ${why}`);
  equal(RT.theme.currentThemeDoc(), null, "a corrupt document must not be half-adopted");
  equal(painted().get("--accent"), "#e8d9a0", "the designed appearance must be in force");
  dom.localStorage.delete("onyx.appearance");
});

await checkAsync("an oklch theme paints with real transparency", async () => {
  /* The canvas is the third reader of a theme value, and it used to have its own
     hex/rgb-only parser: `oklch()` and `hsl()` fell through it and came back
     **opaque**, so every gradient that fades to nothing (`fade(token, 0)`) put a
     solid block of colour on the lane instead. THEMING.md offers both syntaxes
     by name, so this was a user's theme painting wrong, silently.

     Asserted on state — the string `ctx.fillStyle` would be given — not on
     pixels. */
  const text = [
    '{ "onyx": "theme", "name": "OKLCH",',
    '  "dark": {',
    '    "eq-node-glow": "oklch(0.72 0.15 250)",',
    '    "solo-band": "hsl(210 50% 40% / 0.6)",',
    '    "lane-a-rgb": "120 180 240",',
    '    "tip-bg": "#1a2b3ccc"',
    "  } }",
  ].join("\n");
  const result = await RT.io.applyTheme(text);
  assert(result.applied, `the fixture must apply: ${JSON.stringify(result.outcome.problems)}`);
  equal(
    painted().get("--eq-node-glow"),
    "oklch(0.72 0.15 250)",
    "an oklch() value must reach the DOM as authored — that is what the canvas reads back",
  );

  const p = RT.theme.paint();
  const want = X.parseCssColor("oklch(0.72 0.15 250)");

  // The transparent end of a travelling gradient has to actually be transparent.
  const zero = X.parseCssColor(p.fade("--eq-node-glow", 0));
  assert(zero, `fade() produced something no canvas could take: ${p.fade("--eq-node-glow", 0)}`);
  near(zero.a, 0, 1e-9, "fade(token, 0) must be fully transparent, whatever syntax the token used");
  const half = X.parseCssColor(p.fade("--eq-node-glow", 0.5));
  near(half.a, 0.5, 1e-9, "fade() must scale the alpha");
  for (const ch of ["r", "g", "b"]) {
    near(half[ch], want[ch], 1, `fade() moved the hue (${ch})`);
  }

  // A token that already carries an alpha keeps it, scaled.
  const soft = X.parseCssColor(p.fade("--solo-band", 0.5));
  near(soft.a, 0.3, 1e-9, "hsl()'s own alpha must survive and multiply");

  // The bar ink: a triplet token, the common case, still exactly as before.
  const ink = p.tint("--lane-a-rgb", 0.46);
  const bars = X.parseCssColor(ink);
  assert(bars, `tint() produced something no canvas could take: ${ink}`);
  near(bars.a, 0.46, 1e-9, "tint() must set the alpha it was asked for");
  for (const [ch, v] of [["r", 120], ["g", 180], ["b", 240]]) {
    near(bars[ch], v, 1, `tint() moved the lane hue (${ch})`);
  }
  /* A triplet token pointed at a *colour* — `tokens.css` does exactly that
     (`--lane-a-rgb: var(--accent-rgb)`), and the validator allows
     `var(--accent)` there too — used to produce `rgba(#e8d9a0, 0.46)`, which is
     not a colour: the canvas kept whatever fill it had last. A browser
     substitutes the var() before the canvas ever sees it, so the substituted
     form is what is checked here.  */
  const viaColour = X.parseCssColor(X.formatRgba(X.scaleAlpha(X.parseCssColor("#e8d9a0"), 0.46)));
  near(viaColour.a, 0.46, 1e-9, "a colour in a triplet token must still take an alpha");

  // …and the ordinary case is unchanged.
  const tip = X.parseCssColor(p.fade("--tip-bg", 1));
  near(tip.a, 0.8, 0.005, "#rrggbbaa must still work");

  RT.theme.resetAppearanceLocally();
  await RT.io.applyTheme(llm);
});

await checkAsync("the window surface reported to the engine is the one on screen", async () => {
  /* The whole seam, running: a theme document is in force, the webview reads
     back what it is *actually* painting, and that colour crosses the IPC
     boundary the engine validates. The pure checks above prove the resolution;
     this proves the wiring, which is where a light rim would come back — a
     `resolvedSurface()` that returned null, or a value the command refuses,
     would leave the window on the system's grey with nothing failing. */
  const color = RT.theme.resolvedSurface();
  equal(color, "#0b0e11", "the surface reported must be the document's, as painted");
  equal(color, painted().get("--ink-900"), "…and must be what <html> is wearing");

  await RT.api.setWindowSurface(color, RT.theme.resolvedTheme());
  const mock = await runtime.load("mock");
  equal(
    mock.reportedWindowSurface(),
    { color: "#0b0e11", theme: "dark" },
    "the backend did not accept the surface the window is painting",
  );

  // The same two rejections as `surface::tests::only_a_hex_colour_crosses…`:
  // not a colour, and `system`, which has no colour of its own.
  for (const [c, t] of [["banana", "dark"], [color, "system"], ["var(--ink-900)", "light"]]) {
    let threw = null;
    try {
      await RT.api.setWindowSurface(c, t);
    } catch (e) {
      threw = e;
    }
    assert(threw, `the engine must refuse (${c}, ${t})`);
  }
  equal(mock.reportedWindowSurface().color, "#0b0e11", "a refused report must change nothing");

  // A reset forgets it, exactly as `reset_appearance_in` calls `surface::forget`:
  // the designed surface is what the window goes back to.
  await RT.api.resetAppearance();
  equal(mock.reportedWindowSurface(), null, "the reset must forget the document's surface");
  RT.theme.resetAppearanceLocally();
  /* The designed surface is not an inline override — it is the stylesheet's own
     declaration, which a browser hands back through `getComputedStyle` and this
     stand-in DOM cannot. So what is asserted here is that the document's
     override is *gone*, and that the value behind it is obsidian. */
  equal(painted().get("--ink-900"), undefined, "the document's surface override must be removed");
  equal(
    S.surfaceHex([D.effectiveVars(null, "dark", DEFAULTS).get(S.SURFACE_TOKEN)]),
    "#0a0a0c",
    "…leaving the designed obsidian, which is what the engine falls back to",
  );
  await RT.io.applyTheme(llm);
});

checkSync("the editor window tracks the document without wearing it", () => {
  // Why the editor is usable after a theme paints every surface one colour.
  RT.theme.applyThemeDoc(D.parseTheme(llm).doc, llm);
  const worn = painted().get("--ink-900");
  assert(worn, "the fixture must state --ink-900 for this check to mean anything");
  RT.theme.ignoreThemeDoc();
  assert(RT.theme.themeDocIgnored(), "the window must know it is not wearing the document");
  assert(painted().get("--ink-900") !== worn, "the editor window is still wearing the theme");
  equal(RT.theme.currentThemeText(), llm, "…but it must still know which document is in force");
  RT.theme.resetAppearanceLocally();
});

// `api.ts` imports the mock lazily, so the transpiled tree has to outlive the
// first command, not the first import.
await runtime.cleanup();

/* ── report ──────────────────────────────────────────────────────────────── */

const total = passed + failures.length;
console.log(`theme contract: ${passed}/${total} checks passed`);
if (failures.length > 0) {
  for (const f of failures) {
    console.error(`\n  ✗ ${f.name}\n    ${String(f.error.message).split("\n").join("\n    ")}`);
  }
  console.error(`\n${failures.length} of ${total} theme checks failed`);
  process.exit(1);
}
console.log(
  `  ${T.TOKENS.length} tokens · ${D.exportTheme().split("\n").length} lines exported · ` +
    `${C.CONTRAST_PAIRS.length} contrast pairs`,
);
/* The mock is browser code and leaves timers behind that nothing here cancels. */
process.exit(0);
