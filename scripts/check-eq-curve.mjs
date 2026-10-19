/**
 * The EQ curve contract, front-end half.
 *
 * SPEC §12: the curve the EQ panel draws must be the response the audio path
 * actually has. It is computed twice — once in Rust (`dsp::eq::curve_db`, which
 * the engine designs its biquads from) and once in TypeScript
 * (`src/lib/eq.ts::compositeResponse`, so dragging a node costs no IPC). Two
 * implementations of one contract, in two languages, is exactly the kind of
 * thing that drifts silently: the Rust shelves were corrected to RBJ shelf-Q
 * semantics without anything checking that the drawn curve still agreed.
 *
 * `crates/onyx-core/tests/eq_curve_contract.rs` pins the engine's answer for a
 * matrix of shapes, corners, Qs, gains, slopes and sample rates into
 * `crates/onyx-core/tests/fixtures/eq_curve_reference.json`. This script
 * transpiles the *real* `src/lib/eq.ts` (no transcription, no copy of the
 * formulas) and asserts the TypeScript reproduces that fixture.
 *
 *     node scripts/check-eq-curve.mjs        # npm run check:eq
 *
 * Exits non-zero, loudly and with the worst offending case, if they diverge.
 */

import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const FIXTURE = path.join(root, "crates/onyx-core/tests/fixtures/eq_curve_reference.json");

/** Transpile the front end's own sources and import them, types stripped. */
async function loadEqModule() {
  const ts = (await import("typescript")).default;
  const out = await mkdtemp(path.join(tmpdir(), "onyx-eq-"));
  for (const name of ["types", "eq"]) {
    const source = await readFile(path.join(root, "src/lib", `${name}.ts`), "utf8");
    const js = ts.transpileModule(source, {
      compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 },
      fileName: `${name}.ts`,
    }).outputText;
    await writeFile(path.join(out, `${name}.mjs`), js.replaceAll('"./types"', '"./types.mjs"'));
  }
  const mod = await import(pathToFileURL(path.join(out, "eq.mjs")).href);
  await rm(out, { recursive: true, force: true });
  return mod;
}

const fixture = JSON.parse(await readFile(FIXTURE, "utf8"));
const { compositeResponse } = await loadEqModule();

const probes = Float64Array.from(fixture.probeHz);
const tolerance = fixture.toleranceDb;
const out = new Float32Array(probes.length);

let worst = 0;
let worstAt = null;
const worstByKind = new Map();

for (const testCase of fixture.cases) {
  compositeResponse(testCase.config, probes, testCase.sampleRate, out);
  for (let i = 0; i < probes.length; i += 1) {
    const rust = testCase.curveDb[i];
    const drawn = out[i];
    if (!Number.isFinite(rust) && !Number.isFinite(drawn)) continue;
    const error = Math.abs(rust - drawn);
    const kind = testCase.config.bands[0]?.kind ?? "chain";
    if (error > (worstByKind.get(kind) ?? -1)) worstByKind.set(kind, error);
    if (error > worst) {
      worst = error;
      worstAt = { name: testCase.name, hz: probes[i], rust, drawn };
    }
  }
}

const cases = fixture.cases.length;
const points = cases * probes.length;
console.log(
  `EQ curve contract: ${cases} cases x ${probes.length} probes = ` +
    `${points} points, src/lib/eq.ts vs onyx-core::dsp::eq::curve_db`,
);
for (const [kind, error] of [...worstByKind].sort((a, b) => b[1] - a[1])) {
  console.log(`  ${kind.padEnd(10)} worst ${error.toExponential(3)} dB`);
}
console.log(`  worst overall ${worst.toExponential(3)} dB (tolerance ${tolerance} dB)`);

if (worst > tolerance) {
  console.error(
    `\nFAIL: the drawn curve disagrees with the engine by ${worst.toFixed(4)} dB.\n` +
      `  case      ${worstAt.name}\n` +
      `  at        ${worstAt.hz.toFixed(2)} Hz\n` +
      `  engine    ${worstAt.rust.toFixed(6)} dB\n` +
      `  drawn     ${worstAt.drawn.toFixed(6)} dB\n` +
      `SPEC §12 requires these to be the same filter. Fix src/lib/eq.ts, or, if\n` +
      `the engine changed on purpose, regenerate the fixture with\n` +
      `  ONYX_UPDATE_EQ_FIXTURE=1 cargo test -p onyx-core --test eq_curve_contract`,
  );
  process.exit(1);
}
console.log("OK: the drawn curve is the filter the engine runs.");
