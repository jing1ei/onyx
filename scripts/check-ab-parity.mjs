/**
 * The A/B assignment contract, front-end half (SPEC §2.8).
 *
 * Onyx has two implementations of its own backend: the real one in Rust, and
 * `src/lib/mock.ts`, which the browser preview and the screenshot harness run
 * against. They drifted on one rule — *assigning a track to deck B turns A/B
 * on* — and that drift is what hid a shipped bug. In the preview, assigning to
 * B "worked": the mock loaded the deck but left A/B off, so lane B was never
 * drawn, and every screenshot-based verification of deck B passed. On the real
 * macOS build a user reported "deck b is not assignable, only a".
 *
 * So the rule lives in a checked-in fixture that both sides are held to:
 *
 *   src-tauri/tests/fixtures/ab_assign_contract.json
 *     ├─ Rust: src-tauri/tests/ab_assign_contract.rs   (cargo test --workspace)
 *     └─ TS:   this script                             (npm run check:ab)
 *
 * This drives the *real* `src/lib/mock.ts` — transpiled, not transcribed — end
 * to end for every case: open a file, set A/B to the "before" state, invoke
 * `ab_assign`, and read A/B back out of the snapshot the mock publishes. A copy
 * of the rule in the checker would prove nothing.
 *
 * Two more rules joined it, for the same reason and by the same route:
 *
 *   · **removing a track that is on a deck.** `commands::playlist_remove` clears
 *     the deck and leaves A/B *on* (`loader::clear_deck` never touches it —
 *     turning the comparison off is `ab_set_enabled`'s job and nothing else's,
 *     and lane B draws its own "empty" state), and it refuses the removal
 *     outright while a blind test is running. The rules are read out of the Rust
 *     source here rather than copied into it, so a change on that side fails
 *     this check instead of quietly disagreeing with it.
 *   · **the blind-test guard.** `AppState::blind_guard` refuses eleven commands
 *     mid-test with one sentence; the mock refused none of them, so the preview
 *     let you do things the shipped app rejects.
 *
 *     node scripts/check-ab-parity.mjs        # npm run check:ab
 */

import { mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { pathToFileURL } from "node:url";

/* The browser stand-in and the transpile step are shared with
   `check-ipc.mjs`, so the two checkers cannot end up driving slightly
   different mocks — the same reason the screenshot harnesses share
   `shots-base.mjs`. */
import { installBrowserGlobals, loadMockModule, repoRoot as root } from "./lib/mock-host.mjs";

const FIXTURE = path.join(root, "src-tauri/tests/fixtures/ab_assign_contract.json");

/* ── reading the other implementation ────────────────────────────────────── */

/**
 * A Rust function's body, by brace matching. Crude on purpose: the alternative
 * is a second copy of the rule in this file, and a copy is what this whole
 * script exists to prevent.
 */
function rustFn(source, signature) {
  const at = source.indexOf(signature);
  if (at < 0) return null;
  let i = source.indexOf("{", at);
  if (i < 0) return null;
  let depth = 0;
  for (let j = i; j < source.length; j += 1) {
    if (source[j] === "{") depth += 1;
    else if (source[j] === "}") {
      depth -= 1;
      if (depth === 0) return source.slice(i + 1, j);
    }
  }
  return null;
}

/** Every subject `AppState::blind_guard` is called with, on the Rust side. */
function rustGuardSubjects(sources) {
  const subjects = new Set();
  for (const src of sources) {
    for (const m of src.matchAll(/blind_guard\(\s*"([^"]+)"\s*\)/g)) subjects.add(m[1]);
    // …including the one that is built per deck.
    for (const m of src.matchAll(/blind_guard\(&format!\("([^"]*)\{\}"[^)]*deck_label/g)) {
      for (const label of ["A", "B"]) subjects.add(`${m[1]}${label}`);
    }
  }
  return [...subjects];
}

/** One case, run against a mock engine with no state carried in from the last. */
async function runCase(mock, testCase) {
  // Two tracks, so deck A already holds something: assigning B has to be what
  // turns A/B on, not merely the first load doing it.
  await mock.invoke("open_files", { paths: ["parity-1.wav", "parity-2.wav"], replace: true });
  const opened = await mock.invoke("app_state");
  const rows = opened.playlist;
  if (rows.length < 2) throw new Error(`the mock opened ${rows.length} rows, expected 2`);

  await mock.invoke("ab_set_enabled", { value: testCase.abEnabledBefore });
  const before = await mock.invoke("app_state");
  if (before.ab.enabled !== testCase.abEnabledBefore) {
    throw new Error(`could not put the mock into A/B ${testCase.abEnabledBefore}`);
  }

  const row = rows[rows.length - 1];
  const after = await mock.invoke("ab_assign", { deck: testCase.deck, id: row.id });
  const deck = testCase.deck === "b" ? after.deckB : after.deckA;
  return {
    abEnabled: after.ab.enabled,
    // A rule about a flag is worth nothing if the material never arrived.
    loadedEntryId: deck?.loaded ? deck.entryId : null,
    wantEntryId: row.id,
    // The transport mirror the UI actually reads (`recomputeDerived`).
    transportAb: after.transport.abEnabled,
  };
}

installBrowserGlobals();
const fixture = JSON.parse(await readFile(FIXTURE, "utf8"));
const mock = await loadMockModule();

console.log(
  `A/B assignment contract: ${fixture.cases.length} cases, ` +
    `src/lib/mock.ts vs onyx_lib::abrules::ab_enabled_after_assign`,
);

const failures = [];
for (const testCase of fixture.cases) {
  let got;
  try {
    got = await runCase(mock, testCase);
  } catch (err) {
    failures.push(`${testCase.name}: the mock threw — ${err.message}`);
    continue;
  }
  const problems = [];
  if (got.abEnabled !== testCase.abEnabledAfter) {
    problems.push(`A/B is ${got.abEnabled}, contract says ${testCase.abEnabledAfter}`);
  }
  if (got.transportAb !== testCase.abEnabledAfter) {
    problems.push(`transport.abEnabled is ${got.transportAb}, contract says ${testCase.abEnabledAfter}`);
  }
  if (got.loadedEntryId !== got.wantEntryId) {
    problems.push(`deck ${testCase.deck.toUpperCase()} holds ${got.loadedEntryId}, expected ${got.wantEntryId}`);
  }
  const mark = problems.length ? "FAIL" : "ok  ";
  console.log(
    `  ${mark} deck ${testCase.deck.toUpperCase()} · A/B ${String(testCase.abEnabledBefore).padEnd(5)} ` +
      `→ ${String(got.abEnabled).padEnd(5)} · ${testCase.name}`,
  );
  for (const p of problems) failures.push(`${testCase.name}: ${p}`);
}

/* The fixture is the contract, so a case list that quietly shrank to the one
   combination someone remembered is itself a failure. Rust asserts the same
   coverage; both sides check it so neither can be the only guard. */
const seen = new Set(fixture.cases.map((c) => `${c.deck}:${c.abEnabledBefore}`));
for (const deck of ["a", "b"]) {
  for (const before of [false, true]) {
    if (!seen.has(`${deck}:${before}`)) {
      failures.push(`the contract says nothing about assigning deck ${deck.toUpperCase()} with A/B ${before}`);
    }
  }
}

/* The whole point of this file is that a *route* to deck B cannot be verified
   in the preview if the preview's backend disagrees with the engine, so make
   sure the fixture is the one Rust reads. */
const rustTests = await readdir(path.join(root, "src-tauri/tests"));
if (!rustTests.includes("ab_assign_contract.rs")) {
  failures.push("src-tauri/tests/ab_assign_contract.rs is gone — the Rust half of this contract is unchecked");
}

/* ── removing a track that is on a deck ──────────────────────────────────── */

const loaderRs = await readFile(path.join(root, "src-tauri/src/loader.rs"), "utf8");
const commandsRs = await readFile(path.join(root, "src-tauri/src/commands.rs"), "utf8");
const stateRs = await readFile(path.join(root, "src-tauri/src/state.rs"), "utf8");

const clearDeck = rustFn(loaderRs, "pub fn clear_deck(");
const removeCmd = rustFn(commandsRs, "pub async fn playlist_remove(");
if (!clearDeck || !removeCmd) {
  failures.push("could not find clear_deck / playlist_remove in the Rust source — this check is blind");
}

/* What Rust does, read off Rust. `clear_deck` clears the deck, the playlist
   assignment and (if one is running) the blind test; it never writes
   `ab.enabled`, so a deck-B removal leaves the comparison switched on and lane B
   draws its own empty state. If that ever changes, this is where it is caught —
   before the mock and the engine disagree in a preview. */
const rustDisablesAb = clearDeck != null && /ab\s*\.\s*lock\(\)|ab\.enabled/.test(clearDeck);
const rustGuardsRemoval =
  removeCmd != null && /blind_guard\("Removing a track that is on a deck"\)/.test(removeCmd);

if (rustDisablesAb) {
  failures.push(
    "loader::clear_deck now touches ab.enabled — the mock does not; align src/lib/mock.ts " +
      "(case \"playlist_remove\") with it and update this check",
  );
}

async function twoDecks(mock) {
  await mock.invoke("open_files", { paths: ["parity-1.wav", "parity-2.wav"], replace: true });
  const rows = (await mock.invoke("app_state")).playlist;
  if (rows.length < 2) throw new Error(`the mock opened ${rows.length} rows, expected 2`);
  await mock.invoke("ab_assign", { deck: "a", id: rows[0].id });
  const snap = await mock.invoke("ab_assign", { deck: "b", id: rows[1].id });
  if (!snap.ab.enabled) throw new Error("assigning deck B did not turn A/B on");
  return { rows, snap };
}

console.log("\nremoving a deck-B track: src/lib/mock.ts vs commands::playlist_remove");
try {
  const { rows } = await twoDecks(mock);
  const after = await mock.invoke("playlist_remove", { id: rows[1].id });

  const problems = [];
  if (after.deckB.entryId !== null || after.deckB.loaded) {
    problems.push(`deck B still holds ${after.deckB.entryId} after its row was removed`);
  }
  if (after.playlist.some((e) => e.id === rows[1].id)) problems.push("the row is still in the playlist");
  if (after.deckA.entryId !== rows[0].id) problems.push("removing deck B's row disturbed deck A");
  // The rule under audit: A/B stays on, because that is what the engine does.
  if (after.ab.enabled !== !rustDisablesAb) {
    problems.push(
      `A/B is ${after.ab.enabled} after the removal; Rust's clear_deck ${
        rustDisablesAb ? "disables it" : "leaves it alone"
      }`,
    );
  }
  if (after.transport.abEnabled !== after.ab.enabled) {
    problems.push("transport.abEnabled disagrees with ab.enabled — the UI reads the mirror");
  }
  console.log(
    `  ${problems.length ? "FAIL" : "ok  "} deck B cleared · A/B ${after.ab.enabled} · ` +
      `${after.playlist.length} rows left`,
  );
  for (const p of problems) failures.push(`playlist_remove: ${p}`);
} catch (err) {
  failures.push(`playlist_remove: the mock threw — ${err.message}`);
}

/* ── the blind-test guard ────────────────────────────────────────────────── */

/** `AppState::blind_guard`'s sentence, taken from the Rust source. */
const refusalFormat = /"(\{what\} is not allowed[^"]*)"/.exec(stateRs)?.[1] ?? null;
if (!refusalFormat) {
  failures.push("could not find the blind-guard refusal in src-tauri/src/state.rs");
}
const subjects = rustGuardSubjects([commandsRs, loaderRs]);
console.log(`\nthe blind-test guard: ${subjects.length} refusals, mock vs AppState::blind_guard`);

if (refusalFormat) {
  const want = refusalFormat.replace("{what}", "Removing a track that is on a deck");
  const got = mock.blindRefusal(true, "Removing a track that is on a deck");
  if (got !== want) {
    failures.push(`the mock's refusal reads differently:\n    mock: ${got}\n    rust: ${want}`);
  }
  if (mock.blindRefusal(false, "Anything") !== null) {
    failures.push("the mock refuses when no test is running");
  }
}

/* Every subject Rust refuses has to be a subject the mock refuses: the preview
   must not permit an action the shipped app rejects. Checked against the source
   because the mock is one `switch`, and a missing `case` is invisible from
   outside it. */
const mockSource = await readFile(path.join(root, "src/lib/mock.ts"), "utf8");
for (const subject of subjects) {
  const literal = subject.replace(/deck [AB]$/, "deck ${deckLabel(");
  if (!mockSource.includes(`"${subject}"`) && !mockSource.includes(`\`${literal}`)) {
    failures.push(`the mock never refuses "${subject}" — Rust does`);
  }
}

/* …and it is not enough for the string to be there. Drive a real test. */
try {
  const { rows } = await twoDecks(mock);
  await mock.invoke("blind_start", { trials: 4, mode: "abx" });
  const refused = async (cmd, args) => {
    try {
      await mock.invoke(cmd, args);
      return null;
    } catch (err) {
      return err.message;
    }
  };
  const cases = [
    ["playlist_remove", { id: rows[1].id }, true, "removing the track that is on deck B"],
    ["playlist_clear", {}, true, "clearing the playlist"],
    ["ab_assign", { deck: "b", id: rows[0].id }, true, "assigning a deck"],
    ["ab_set_enabled", { value: false }, true, "switching A/B off"],
    ["ab_select", { deck: "b" }, true, "naming the audible deck"],
    ["ab_toggle_deck", {}, true, "toggling the audible deck"],
    ["set_level_match", { enabled: true }, true, "level matching"],
    ["set_ab_offset", { frames: 480 }, true, "changing the offset"],
    ["set_deck_invert", { deck: "b", invert: true }, true, "inverting a deck"],
    ["open_files", { paths: ["x.wav"], replace: true }, true, "opening files over the test"],
    // Allowed mid-test, on both sides: appending does not touch either deck, and
    // the transport is the listener's own control.
    ["open_files", { paths: ["y.wav"], replace: false }, false, "appending a file"],
    ["transport_toggle", {}, false, "play/pause"],
  ];
  for (const [cmd, args, mustRefuse, what] of cases) {
    const message = await refused(cmd, args);
    const didRefuse = message != null && /blind test/.test(message);
    if (mustRefuse && !didRefuse) {
      failures.push(`the mock allowed ${what} during a blind test; the engine refuses it`);
    }
    if (!mustRefuse && didRefuse) {
      failures.push(`the mock refused ${what} during a blind test; the engine allows it`);
    }
    console.log(`  ${mustRefuse === didRefuse ? "ok  " : "FAIL"} ${mustRefuse ? "refuses" : "allows "} ${what}`);
  }
  // A row on no deck is removable mid-test, exactly as in Rust: the guard is
  // about the material under comparison, not about the list.
  const spare = (await mock.invoke("app_state")).playlist.find(
    (e) => e.id !== rows[0].id && e.id !== rows[1].id,
  );
  if (spare) {
    const message = await refused("playlist_remove", { id: spare.id });
    if (message) failures.push(`removing a row that is on no deck was refused: ${message}`);
    else console.log("  ok   allows removing a row that is on no deck");
  }
  await mock.invoke("blind_abort", {});
} catch (err) {
  failures.push(`the blind-guard drive threw — ${err.message}`);
}

/* ── the alignment drag (SPEC §11) ───────────────────────────────────────── */

/* Alt-drag on lane B slides the A/B offset, and a drag that cannot be released
   is a stuck comparison: `alignRef.dragging` blocks every later engine offset
   sync, so the offset the UI shows stops being the offset the engine has. The
   two things that keep that from happening are structural, so they are checked
   structurally rather than photographed.

   An audit read this file and reported the alignment drag as *lacking* pointer
   capture because the one `setPointerCapture` call is "in a different handler".
   It is not: `onPointerDown` is the single handler for all three drag modes and
   captures before it branches. That is easy to break by mistake — move the
   capture below the `mode: "align"` branch, or give the align gesture a handler
   of its own without one, and releasing over the transport bar leaves the drag
   armed — so the *order* is what is asserted, not the mere presence of the call.
   Anything that makes the audit's reading true now fails here. */
const stack = await readFile(path.join(root, "src/components/WaveformStack.tsx"), "utf8");
const down = /const onPointerDown = useCallback\([\s\S]*?\n  \);/.exec(stack)?.[0] ?? "";
if (!down) {
  failures.push("WaveformStack has no onPointerDown callback to check for pointer capture");
} else {
  const capture = down.indexOf("setPointerCapture(");
  const align = down.indexOf('mode: "align"');
  if (capture < 0) {
    failures.push(
      "WaveformStack's onPointerDown no longer captures the pointer: releasing an Alt-drag over " +
        "the transport bar leaves alignRef.dragging true and every later offset sync blocked",
    );
  } else if (align < 0) {
    failures.push('WaveformStack no longer starts an "align" drag on Alt-drag (SPEC §11)');
  } else if (capture > align) {
    failures.push(
      "WaveformStack captures the pointer only after the align branch has returned, so the " +
        "Alt-drag runs uncaptured and cannot be released outside the lane",
    );
  }
}
for (const event of ["pointerup", "pointercancel", "blur"]) {
  if (!stack.includes(`"${event}"`)) {
    failures.push(`WaveformStack has no window "${event}" release path for a drag`);
  }
}
if (!/e\.buttons === 0/.test(stack)) {
  failures.push("WaveformStack does not notice a drag whose pointerup went to another window");
}
if (!/relatedTarget/.test(stack)) {
  failures.push("the lane's dragleave does not check relatedTarget, so crossing into a child unlights it");
}
for (const event of ["dragend", "drop"]) {
  if (!new RegExp(`addEventListener\\("${event}"`).test(stack)) {
    failures.push(`a drag that ends outside a lane never clears the highlight (no window "${event}")`);
  }
}

/* ── the alignment writes themselves ─────────────────────────────────────── */

/**
 * `src/lib/align.ts`, driven for real.
 *
 * The module coalesces offset writes to one animation frame *and* to one write
 * at a time. The second half is the one an audit cannot see by reading: on a
 * machine where a `set_ab_offset` round trip outlasts a frame — any machine,
 * while the engine is re-decoding a deck — two frames' writes would otherwise
 * be on the wire together, the engine may ack them in either order, and
 * `alignRef.confirmed` would end up holding a superseded offset. A rejection
 * then rolls the lane back to a value the user never dragged to.
 *
 * So the frame clock and the IPC boundary are both put under this checker's
 * control — a *drag* is what has to be reproduced, and a drag is a sequence of
 * events at pointer rate against writes that have not come back yet. Only those
 * two seams are stubbed: `requestAnimationFrame` becomes a queue this file
 * ticks, and `api.setAbOffset` becomes a recorder that hands back promises it
 * resolves on command. Everything else is the module the app ships.
 */
async function loadAlign() {
  const ts = (await import("typescript")).default;
  const out = await mkdtemp(path.join(tmpdir(), "onyx-align-"));

  /** Every write, in the order it went out, and the pending resolvers. */
  const wire = { writes: [], live: 0, maxLive: 0, settle: [] };
  const apiStub = `
    export const setAbOffset = (frames) => globalThis.__onyxWire.write(frames);
    export const errorMessage = (e) => String(e && e.message ? e.message : e);
  `;
  /* `align.ts` reaches the store for exactly one thing: the toast a refused
     offset raises. */
  const storeStub = `
    export const useStore = { getState: () => ({ pushToast: (level, text) =>
      void globalThis.__onyxWire.toasts.push([level, text]) }) };
  `;
  await writeFile(path.join(out, "api.mjs"), apiStub);
  await writeFile(path.join(out, "store.mjs"), storeStub);
  const source = await readFile(path.join(root, "src/lib/align.ts"), "utf8");
  const js = ts
    .transpileModule(source, {
      compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 },
      fileName: "align.ts",
    })
    .outputText.replaceAll('"./api"', '"./api.mjs"')
    .replaceAll('"./store"', '"./store.mjs"');
  await writeFile(path.join(out, "align.mjs"), js);

  /** The frame clock: nothing runs until this file says so. */
  const frames = new Map();
  let nextFrame = 1;
  globalThis.requestAnimationFrame = (fn) => {
    const id = nextFrame++;
    frames.set(id, fn);
    return id;
  };
  globalThis.cancelAnimationFrame = (id) => void frames.delete(id);
  globalThis.__onyxWire = {
    toasts: [],
    write(frames_) {
      wire.writes.push(frames_);
      wire.live += 1;
      wire.maxLive = Math.max(wire.maxLive, wire.live);
      return new Promise((resolve, reject) => {
        wire.settle.push({
          frames: frames_,
          ok: () => {
            wire.live -= 1;
            resolve(undefined);
          },
          bad: (why) => {
            wire.live -= 1;
            reject(new Error(why));
          },
        });
      });
    },
  };
  /** Run every frame queued *now* — a frame queued by one of them waits. */
  const tick = async () => {
    const due = [...frames.entries()];
    frames.clear();
    for (const [, fn] of due) fn();
    // let the promise chain inside `flush` run
    await new Promise((r) => setTimeout(r, 0));
  };
  /** Ack (or refuse) the oldest write still on the wire. */
  const ack = async (how = "ok", why = "refused") => {
    const next = wire.settle.shift();
    if (!next) throw new Error("nothing is on the wire to ack");
    if (how === "ok") next.ok();
    else next.bad(why);
    await new Promise((r) => setTimeout(r, 0));
    return next.frames;
  };
  const mod = await import(pathToFileURL(path.join(out, "align.mjs")).href);
  await rm(out, { recursive: true, force: true });
  return { align: mod, wire, tick, ack, toasts: globalThis.__onyxWire.toasts };
}

console.log("\nthe A/B offset drag: src/lib/align.ts against a recording backend");
try {
  const { align, wire, tick, ack, toasts } = await loadAlign();
  const RATE = 48_000;
  const problems = [];

  // A drag: three positions inside one frame. One write, carrying the last.
  align.setOffset(120, RATE);
  align.setOffset(240, RATE);
  align.setOffset(360, RATE);
  if (wire.writes.length !== 0) problems.push("a write went out before the frame did");
  await tick();
  if (JSON.stringify(wire.writes) !== JSON.stringify([360])) {
    problems.push(`a frame's worth of drag wrote ${JSON.stringify(wire.writes)}, expected [360]`);
  }

  // The drag continues while that write is still on the wire. Later frames must
  // not overtake it, however many of them pass.
  align.setOffset(480, RATE);
  await tick();
  align.setOffset(600, RATE);
  await tick();
  await tick();
  if (wire.writes.length !== 1) {
    problems.push(`writes overlapped: ${JSON.stringify(wire.writes)} with one still unacked`);
  }
  if (align.alignWriteState().inflight !== 1) {
    problems.push(`alignWriteState reports ${align.alignWriteState().inflight} writes in flight, expected 1`);
  }

  // The first ack releases exactly one more write, and it carries the value the
  // drag has reached — not the two it passed through.
  await ack();
  await tick();
  if (JSON.stringify(wire.writes) !== JSON.stringify([360, 600])) {
    problems.push(`after the ack the wire holds ${JSON.stringify(wire.writes)}, expected [360, 600]`);
  }
  await ack();
  await tick();
  await tick();
  if (wire.writes.length !== 2) {
    problems.push(`the drag kept writing after it ended: ${JSON.stringify(wire.writes)}`);
  }
  if (wire.maxLive > 1) problems.push(`${wire.maxLive} writes were on the wire at once`);
  if (align.alignRef.confirmed !== 600 || align.alignRef.frames !== 600) {
    problems.push(
      `the drag ended at ${align.alignRef.frames} / confirmed ${align.alignRef.confirmed}, expected 600`,
    );
  }
  if (align.alignWriteState().queued) problems.push("a write is still queued after the drag ended");

  // A refusal rolls the lane back to what the engine last confirmed, and says so.
  align.setOffset(900, RATE);
  await tick();
  await ack("bad", "offset out of range");
  if (align.alignRef.frames !== 600) {
    problems.push(`a refused offset left the lane at ${align.alignRef.frames}, expected 600`);
  }
  if (!toasts.some(([level, text]) => level === "error" && /Alignment refused/.test(text))) {
    problems.push("a refused offset raised no error toast");
  }

  // An engine-reported offset (auto-align) cancels the drag's queued write
  // rather than letting it overwrite the value that just arrived.
  align.setOffset(1_200, RATE);
  align.adoptOffset(-480);
  await tick();
  await tick();
  if (wire.writes.length !== 3) {
    problems.push(`an adopted offset did not cancel the queued write: ${JSON.stringify(wire.writes)}`);
  }
  if (align.alignRef.frames !== -480 || align.alignRef.confirmed !== -480) {
    problems.push(`adoptOffset left the lane at ${align.alignRef.frames}, expected -480`);
  }

  console.log(
    `  ${problems.length ? "FAIL" : "ok  "} writes ${JSON.stringify(wire.writes)} · ` +
      `max concurrent ${wire.maxLive} · ended at ${align.alignRef.confirmed}`,
  );
  for (const p of problems) failures.push(`align: ${p}`);
} catch (err) {
  failures.push(`the offset drag drive threw — ${err.stack ?? err.message}`);
}

if (failures.length) {
  console.error(`\nFAIL: the mock backend disagrees with the engine.\n  ${failures.join("\n  ")}`);
  console.error(
    "\nSPEC §2.8 requires assigning deck B to turn A/B on. Fix src/lib/mock.ts\n" +
      "(case \"ab_assign\") or src-tauri/src/abrules.rs — but never the fixture alone.",
  );
  process.exit(1);
}
console.log(
  "OK: the preview's backend assigns decks, clears them and refuses mid-test the way the engine does.",
);
/* The mock schedules browser timers (the level-match measurement lands "a beat
   later"); with a stub `window` nothing cancels them, so say we are done. */
process.exit(0);
