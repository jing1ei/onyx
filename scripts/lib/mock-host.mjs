/**
 * Running `src/lib/mock.ts` — the preview's backend — inside Node.
 *
 * Two checks drive the real mock rather than a transcription of it
 * (`check-ab-parity.mjs` for the A/B rules, `check-ipc.mjs` for the command
 * surface), and both need the same two things: a browser-shaped stand-in for
 * the globals the module touches, and the module itself with its types
 * stripped. That plumbing lives here for the same reason the screenshot
 * harnesses share `shots-base.mjs`: two copies of it would drift, and a
 * checker that loads a *slightly* different mock than the preview does is
 * worth less than no checker.
 */

import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

export const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");

/**
 * The mock is browser code: it publishes itself on `window`, keeps its theme in
 * `sessionStorage` and runs its frame clock on `setInterval`. None of that is
 * the contract, so it gets the thinnest possible stand-in — enough for the
 * module to load and for `invoke` to run, and nothing more.
 */
export function installBrowserGlobals() {
  const store = new Map();
  const win = globalThis;
  win.window = win;
  win.opener = null;
  win.closed = false;
  win.location = { href: "http://localhost/index.html" };
  win.screenX = 0;
  win.screenY = 0;
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

/**
 * Transpile the front end's own sources and import them, types stripped.
 *
 * `mock.ts` imports `./types`; anything else it grows a dependency on should
 * fail loudly here rather than be quietly stubbed.
 */
export async function loadMockModule(prefix = "onyx-mock-") {
  const ts = (await import("typescript")).default;
  const out = await mkdtemp(path.join(tmpdir(), prefix));
  const dir = path.join(repoRoot, "src/lib");
  for (const name of ["types", "mock"]) {
    const source = await readFile(path.join(dir, `${name}.ts`), "utf8");
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
