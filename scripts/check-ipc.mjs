#!/usr/bin/env node
/**
 * The IPC command surface, held equal on all three sides (SPEC §3.1).
 *
 * Onyx has two backends — the real one in Rust, and `src/lib/mock.ts`, which
 * the browser preview and all three screenshot harnesses run against — and one
 * mirror of the surface in between them, `src/lib/api.ts`. Two of those three
 * were already pinned to each other: `every_front_end_command_is_registered_and_
 * nothing_extra_is` in `src-tauri/src/lib.rs` reads the `call("…")` sites out of
 * `api.ts` and the `generate_handler![…]` block out of `lib.rs` and holds the
 * two sets equal both ways.
 *
 * The third side was unchecked. The mock throws on an unknown command, but
 * nothing counted its `case` labels against that surface, so a command added to
 * Rust and to `api.ts` without a mock case passed *every* existing gate —
 * `check:eq`, `check:ab`, `check:theme`, the Rust contract test, `tsc`, both
 * builds — and then threw `mock: unknown command "…"` the first time the
 * preview called it. That is the only way UI work gets reviewed in this
 * environment (CONTRIBUTING §2 "Reviewing UI changes with no display"), and
 * mock↔Rust drift of exactly this shape has already shipped a bug to a user
 * (§3.2, the deck-B report). So it is a check, not a note in a document.
 *
 *   src-tauri/src/lib.rs  generate_handler![…]   the commands that exist
 *   src/lib/api.ts        call("…")              the wrappers the UI calls through
 *   src/lib/mock.ts       case "…":              the preview's implementation
 *   SPEC.md §3.1          the table              the contract all three answer to
 *
 * Held equal in every direction: a command missing from any one of the four is
 * a failure naming it, and so is one that only *some* of them still have.
 *
 * The mock is parsed with the TypeScript compiler's own parser, not a regex:
 * the labels have to come from the `switch (cmd)` inside `invoke` and from
 * nowhere else, because a `case "…"` in some other switch would satisfy a grep
 * while dispatching nothing. Every command is then *driven* through the real
 * transpiled mock to prove the label is on the live dispatch path, and the
 * unknown-command sentinel is driven too — a checker whose failure mode is
 * "matched nothing, printed OK" would be worse than no checker.
 *
 *     node scripts/check-ipc.mjs        # npm run check:ipc
 */

import { readFile } from "node:fs/promises";
import path from "node:path";

import { installBrowserGlobals, loadMockModule, repoRoot as root } from "./lib/mock-host.mjs";

/**
 * Commands that deliberately have no `case` in `src/lib/mock.ts`, each with the
 * reason it cannot have one, in the map — not in a comment somewhere else and
 * not in a `continue`.
 *
 * Empty today: every command in the surface is observable from the preview, so
 * every command has a mock case. An entry here must be a registered command
 * *and* must really be absent from the mock, or this check fails on the
 * exemption itself — a stale exemption is how a hole gets quietly re-opened.
 */
const MOCK_EXEMPT = new Map([
  // ["some_command", "why the preview can never reach it"],
]);

/** Every source this check reads, so a moved file fails loudly. */
const LIB_RS = path.join(root, "src-tauri/src/lib.rs");
const API_TS = path.join(root, "src/lib/api.ts");
const MOCK_TS = path.join(root, "src/lib/mock.ts");
const SPEC_MD = path.join(root, "SPEC.md");

/**
 * The surface is 60-odd commands and has only ever grown. A parse that comes
 * back with a handful has found a file whose shape moved, and must fail as a
 * broken check rather than pass as an agreement between two empty sets.
 */
const FLOOR = 40;

const failures = [];
const fail = (message) => {
  /* A tolerated name arrives as `null` from `compare`; everything else is real. */
  if (message != null) failures.push(message);
};

/* ── the commands that exist: generate_handler! ───────────────────────────── */

/**
 * `generate_handler![…]`, read the same way `registered_commands()` in
 * `src-tauri/src/lib.rs`'s own test module reads it, so the two cannot disagree
 * about what "registered" means.
 */
function registeredCommands(source) {
  const open = source.indexOf("generate_handler![");
  if (open < 0) throw new Error(`no generate_handler![ in ${LIB_RS}`);
  const rest = source.slice(open + "generate_handler![".length);
  const close = rest.indexOf("])");
  if (close < 0) throw new Error(`the generate_handler! list in ${LIB_RS} is not closed`);
  return rest
    .slice(0, close)
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.startsWith("commands::"))
    .map((line) => line.slice("commands::".length).replace(/,$/, ""));
}

/* ── the wrappers: api.ts ─────────────────────────────────────────────────── */

/** Every `call("…")` site in `src/lib/api.ts`. */
function apiCommands(source) {
  return [...source.matchAll(/\bcall(?:<[^>]*>)?\(\s*"([a-z][a-z0-9_]*)"/g)].map((m) => m[1]);
}

/* ── the preview's implementation: mock.ts ────────────────────────────────── */

/**
 * The `case` labels of the `switch (cmd)` inside `mock.invoke`, plus whether it
 * still ends in a `default:` that throws.
 *
 * Parsed with the TypeScript compiler, because the thing being asserted is that
 * a label *dispatches* `cmd`: a string in a comment, a `case` in a nested
 * switch, or a label in some unrelated function would all satisfy a regex while
 * leaving the preview to throw. Anything unexpected here raises, so the check
 * cannot degrade into a comparison of two empty sets.
 */
async function mockDispatch(source) {
  const ts = (await import("typescript")).default;
  const sf = ts.createSourceFile("mock.ts", source, ts.ScriptTarget.ES2022, true);

  const invoke = sf.statements.find(
    (s) => ts.isFunctionDeclaration(s) && s.name?.text === "invoke",
  );
  if (!invoke?.body) throw new Error(`src/lib/mock.ts declares no top-level invoke() function`);

  /** The `switch` whose subject is the command name, at any depth inside it. */
  let dispatch = null;
  const walk = (node) => {
    if (
      ts.isSwitchStatement(node) &&
      ts.isIdentifier(node.expression) &&
      node.expression.text === "cmd"
    ) {
      if (dispatch) throw new Error("mock.invoke has more than one switch on `cmd`");
      dispatch = node;
    }
    ts.forEachChild(node, walk);
  };
  walk(invoke.body);
  if (!dispatch) throw new Error("mock.invoke has no switch (cmd) — nothing dispatches a command");

  const commands = [];
  let fallthroughThrows = false;
  for (const clause of dispatch.caseBlock.clauses) {
    if (ts.isDefaultClause(clause)) {
      fallthroughThrows = clause.statements.some((s) => ts.isThrowStatement(s));
      continue;
    }
    if (!ts.isStringLiteral(clause.expression)) {
      throw new Error(
        `a case label in mock.invoke is not a string literal: ${clause.expression.getText(sf)}`,
      );
    }
    commands.push(clause.expression.text);
  }
  return { commands, fallthroughThrows };
}

/* ── the contract: SPEC §3.1 ──────────────────────────────────────────────── */

/**
 * The command names in SPEC §3.1's table, and the count its prose claims.
 *
 * "If §3.1 does not list a command, it does not exist" (CONTRIBUTING §1), so
 * the table is the fourth side of the same surface. Some rows carry two or
 * three names (`playlist_next` / `playlist_prev`), which is why every backticked
 * identifier in the first cell counts and none of the prose around the table
 * does. The stated count is checked against the handler list as well, so the one
 * number SPEC does state about this surface cannot rot.
 */
function specCommands(spec) {
  const start = spec.indexOf("### 3.1 Commands");
  if (start < 0) throw new Error("SPEC.md has no §3.1 Commands section");
  const section = spec.slice(start).split("\n### ")[0];
  const names = [];
  for (const line of section.split("\n")) {
    if (!line.startsWith("|")) continue;
    const cell = line.slice(1).split("|")[0];
    for (const m of cell.matchAll(/`([a-z][a-z0-9_]*)`/g)) names.push(m[1]);
  }
  const claimed = /registered surface\s*—\s*(\d+)\s*commands/.exec(section);
  return { names, claimed: claimed ? Number(claimed[1]) : null };
}

/* ── read all four ───────────────────────────────────────────────────────── */

const [libRs, apiTs, mockTs, specMd] = await Promise.all(
  [LIB_RS, API_TS, MOCK_TS, SPEC_MD].map((file) => readFile(file, "utf8")),
);

const registered = registeredCommands(libRs);
const wrappers = apiCommands(apiTs);
const { commands: mockCases, fallthroughThrows } = await mockDispatch(mockTs);
const { names: spec, claimed } = specCommands(specMd);

const sides = [
  ["generate_handler!", "src-tauri/src/lib.rs", registered],
  ["api.ts wrappers", "src/lib/api.ts", wrappers],
  ["mock cases", "src/lib/mock.ts", mockCases],
  ["SPEC §3.1", "SPEC.md", spec],
];

console.log("the IPC command surface, four ways:");
for (const [label, file, list] of sides) {
  console.log(`  ${String(list.length).padStart(3)} ${label.padEnd(18)} ${file}`);
  if (list.length < FLOOR) {
    fail(`only ${list.length} commands parsed out of ${file} — this check has gone blind`);
  }
  const duplicates = list.filter((name, i) => list.indexOf(name) !== i);
  if (duplicates.length) fail(`${file} lists ${[...new Set(duplicates)].join(", ")} twice`);
}

/* ── the exemptions, before anything is compared ─────────────────────────── */

for (const [name, reason] of MOCK_EXEMPT) {
  if (!reason || reason.length < 10) {
    fail(`the mock exemption for \`${name}\` has no reason recorded next to it`);
  }
  if (!registered.includes(name)) {
    fail(`\`${name}\` is exempted from needing a mock case but is not a registered command`);
  } else if (mockCases.includes(name)) {
    fail(
      `\`${name}\` is exempted from needing a mock case and has one — drop the exemption ` +
        `from scripts/check-ipc.mjs rather than leaving it to cover a future hole`,
    );
  }
}

/* ── hold the four sides equal ────────────────────────────────────────────── */

/**
 * Both directions, always, with the missing side named in the message. A
 * message function may return `null` for a name it deliberately tolerates —
 * that is how `MOCK_EXEMPT` is honoured, and the only way it is.
 */
function compare(a, b, missingFromB, missingFromA) {
  for (const name of new Set(a[2])) if (!b[2].includes(name)) fail(missingFromB(name));
  for (const name of new Set(b[2])) if (!a[2].includes(name)) fail(missingFromA(name));
}

const handler = sides[0];
const api = sides[1];
const mock = sides[2];
const contract = sides[3];

compare(
  handler,
  mock,
  (name) =>
    MOCK_EXEMPT.has(name)
      ? null
      : `\`${name}\` is in generate_handler! but src/lib/mock.ts has no \`case "${name}":\` — ` +
        `the browser preview throws \`mock: unknown command "${name}"\` the first time the UI ` +
        `calls it, and no other check notices (CONTRIBUTING §3.2, §4.1 step 6)`,
  (name) =>
    `src/lib/mock.ts handles \`case "${name}":\` but no such command is registered in ` +
      `generate_handler! — the preview implements something the shipped app does not have, ` +
      `so a review of it proves nothing`,
);

compare(
  handler,
  api,
  (name) => `\`${name}\` is registered but no wrapper in src/lib/api.ts calls it`,
  (name) => `\`${name}\` is called by src/lib/api.ts but is not in generate_handler!`,
);

compare(
  handler,
  contract,
  (name) => `\`${name}\` is registered but SPEC §3.1 does not list it — §3.1 is the contract`,
  (name) => `SPEC §3.1 lists \`${name}\` but nothing registers it`,
);

/* `MOCK_EXEMPT` is honoured inside `compare`, by a message function that
   returns `null` for an exempted name. Nothing is skipped silently: the
   exemption itself was checked above, and it is printed with its reason below. */

if (claimed == null) {
  fail("SPEC §3.1 no longer states the size of the registered surface");
} else if (claimed !== registered.length) {
  fail(`SPEC §3.1 claims ${claimed} commands; generate_handler! registers ${registered.length}`);
}

if (!fallthroughThrows) {
  fail(
    "the switch in mock.invoke no longer throws on an unknown command, so the preview would " +
      "answer a command it does not implement with `undefined` instead of failing",
  );
}

/* ── drive every one of them through the real mock ───────────────────────── */

/* Textual agreement is not the claim. The claim is that the preview *answers*
   each command, so each one is invoked against the transpiled mock and the
   unknown-command refusal must not come back. Arguments are deliberately
   omitted: a case may reject `{}` for a hundred good reasons of its own (that
   is `check:ab`'s and the harnesses' business), but "unknown command" can only
   mean the label is not on the dispatch path. */
installBrowserGlobals();
const engine = await loadMockModule("onyx-ipc-");

const unknown = (message) => /unknown command/.test(message);
const drive = async (cmd) => {
  try {
    await engine.invoke(cmd, {});
    return null;
  } catch (err) {
    return err instanceof Error ? err.message : String(err);
  }
};

const sentinel = await drive("no_such_command_exists");
if (sentinel == null) {
  fail("the mock answered a command that does not exist instead of throwing");
} else if (!unknown(sentinel)) {
  fail(`the mock's refusal of an unknown command does not say so: ${sentinel}`);
}

let answered = 0;
for (const name of registered) {
  if (MOCK_EXEMPT.has(name)) continue;
  const message = await drive(name);
  if (message != null && unknown(message)) {
    fail(`src/lib/mock.ts does not answer \`${name}\`: ${message}`);
  } else {
    answered += 1;
  }
}
console.log(
  `  ${answered}/${registered.length - MOCK_EXEMPT.size} answered by the transpiled mock ` +
    `(unknown-command sentinel: ${unknown(sentinel ?? "") ? "armed" : "MISSING"})`,
);
for (const [name, reason] of MOCK_EXEMPT) console.log(`  exempt: ${name} — ${reason}`);

/* ── verdict ─────────────────────────────────────────────────────────────── */

if (failures.length) {
  console.error(`\nFAIL: the IPC surface disagrees with itself.\n  ${failures.join("\n  ")}`);
  console.error(
    "\nCONTRIBUTING §4.1 lists all six steps for adding a command: SPEC §3.1, commands.rs,\n" +
      "generate_handler!, types.ts, api.ts, and the case in mock.ts — in one commit.",
  );
  process.exit(1);
}
console.log(
  `OK: ${registered.length} commands, registered · wrapped · mocked · specified, in every direction.`,
);
/* The mock's frame clock is a `setInterval` and its level-match measurement a
   `setTimeout`; with a stub `window` nothing cancels them, so say we are done. */
process.exit(0);
