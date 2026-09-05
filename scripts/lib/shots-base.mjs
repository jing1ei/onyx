/**
 * The plumbing every screenshot harness needs, in one place.
 *
 * `scripts/shots.mjs`, `scripts/shots-theme.mjs` and `scripts/shots-themedoc.mjs`
 * are three different *arguments* about the app, but they open the same browser
 * against the same mock preview, watch the same consoles, dismiss the same
 * toast and photograph the same two windows. Kept as three copies, that
 * plumbing drifted: the toast-clear delay was 260 ms in one file, 300 ms in
 * another and 250 ms in the third, `shotOf` took its arguments in two different
 * orders, and one harness collected assertion failures and then exited 0, so a
 * broken run looked like a clean one. None of those differences meant anything;
 * they were just three people editing three copies.
 *
 * So the plumbing lives here and the harnesses keep only their arguments. The
 * numbers below are the reconciled ones, each tied to something in the app
 * rather than to whichever file happened to be edited last.
 *
 * Not shipped: dev tooling, run from `package.json` (`npm run shots`,
 * `shots:theme`, `shots:themedoc`).
 */
import { chromium } from "playwright";
import { existsSync, mkdirSync, statSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join, resolve } from "node:path";

/** Repo root, from this file's own location (`scripts/lib/` → `..`/`..`). */
export const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");

/** Where every harness writes; `.gitignore`d, regenerable, never committed. */
export const SHOTS_DIR = join(ROOT, "shots");

/**
 * The app's real default window — `tauri.conf.json` `app.windows[0]`
 * (1180×760, min 420×560). Shots taken any wider flatter the layout and hide
 * the spacing the app actually ships with.
 */
export const MAIN_VIEWPORT = { width: 1180, height: 760 };

/** The EQ window's first-run size — `src-tauri/src/eqwindow.rs` (SPEC §12). */
export const EQ_VIEWPORT = { width: 940, height: 560 };

/**
 * How long to wait after retiring the toasts before the shutter opens.
 *
 * Dismissal itself is synchronous (`Toasts.tsx` removes the node on click, with
 * no exit transition), so the wait is not for the toast to leave — it is for a
 * toast that *arrived* in the same tick as the click, which is mid-`toast-in`
 * and would otherwise be photographed half-drawn. `toast-in` is 260 ms
 * (`src/styles/app.css`, and the same in `eq.css`), so this is that animation
 * plus a couple of frames' margin. The three harnesses used 250, 260 and
 * 300 ms; 300 is the only one of the three that actually clears the animation
 * it is waiting on, so 300 it is.
 */
export const TOAST_CLEAR_MS = 300;

/**
 * A PNG this small is a blank or half-painted frame, not a screenshot. Every
 * shot in every harness is checked, including the ones that keep their toast.
 */
export const MIN_SHOT_BYTES = 5000;

/** A local `vite preview` of `dist-mock`, the documented way to run these. */
export const DEFAULT_URL = "http://localhost:4173";

/**
 * The preview URL: first http(s) argument, else `ONYX_SHOTS_URL`, else local.
 *
 * Positional rather than first-argument-only because `shots-theme.mjs` also
 * takes `--tag <name>`; and local-by-default because this once defaulted to a
 * throwaway sandbox deploy URL, which stopped resolving the moment that sandbox
 * went away and made the harnesses look broken.
 */
export function shotsUrl(argv = process.argv.slice(2)) {
  return argv.find((a) => /^https?:\/\//.test(a)) ?? process.env.ONYX_SHOTS_URL ?? DEFAULT_URL;
}

/**
 * Launch Chromium, open the app's window, and hand back the tools the
 * harnesses share.
 *
 * @param {object} [options]
 * @param {string[]} [options.argv]     argv tail, for the URL (default: this process's)
 * @param {string}   [options.url]      explicit URL, overriding argv/env
 * @param {object}   [options.viewport] main-window viewport (default: MAIN_VIEWPORT)
 * @param {string}   [options.dir]      where shots land (default: SHOTS_DIR)
 */
export async function startHarness({ argv, url, viewport, dir } = {}) {
  const out = dir ?? SHOTS_DIR;
  mkdirSync(out, { recursive: true });

  const target = url ?? shotsUrl(argv);

  /** Everything that went wrong, printed by `finish()` and decides the exit code. */
  const errors = [];
  /** Every file written, printed by `finish()`. */
  const wrote = [];

  const browser = await chromium.launch();
  const context = await browser.newContext({
    viewport: viewport ?? MAIN_VIEWPORT,
    deviceScaleFactor: 2,
  });

  /** Adopt a document's console: anything it complains about is a failure. */
  const watch = (target_, who) => {
    target_.on("console", (m) => {
      // `willReadFrequently` is a harness reading canvases back, not the app
      if (m.text().includes("willReadFrequently")) return;
      if (m.type() === "error" || m.type() === "warning") {
        errors.push(`${who} ${m.type()}: ${m.text()}`);
      }
    });
    target_.on("pageerror", (e) => errors.push(`${who} pageerror: ${e.message}`));
  };

  const page = await context.newPage();
  watch(page, "main");

  const settle = (ms = 700) => page.waitForTimeout(ms);

  /** The engine's own answer, read through the mock host in a document. */
  const engine = (cmd, params = {}, on = page) =>
    on.evaluate(([c, a]) => window.__onyxMockHost.invoke(c, a), [cmd, params]);

  /**
   * Photograph any document, not just the main one: since SPEC §12 the EQ is a
   * window of its own (a Tauri `WebviewWindow` in the app, a browser popup
   * here) and so is the theme editor since §20 — `page` cannot see inside
   * either.
   *
   * The mock announces itself with a toast ("no audio device attached"); it
   * auto-dismisses after a few seconds, but it should never be sitting over the
   * A/B rail in a reference shot, so open toasts are retired first — unless the
   * toast *is* the subject (`keepToasts`).
   */
  const shotOf = async (on, name, { dir: into = out, keepToasts = false } = {}) => {
    if (!keepToasts) {
      await on.evaluate(() => {
        document.querySelectorAll(".toast").forEach((t) => t.click());
      });
      await on.waitForTimeout(TOAST_CLEAR_MS);
    }
    mkdirSync(into, { recursive: true });
    const p = join(into, `${name}.png`);
    await on.screenshot({ path: p });
    if (!existsSync(p) || statSync(p).size < MIN_SHOT_BYTES) throw new Error(`bad screenshot ${p}`);
    wrote.push(`${p} (${(statSync(p).size / 1024).toFixed(0)} kB)`);
    return p;
  };

  /**
   * Adopt a second document: run `open` (the gesture that spawns it), wait for
   * the popup, size it like the real window, and wait for the selector that
   * means it has rendered.
   */
  const openPopup = async ({ open, who, selector, viewport: size, from = page, ready = 1500 }) => {
    const appearing = from.waitForEvent("popup", { timeout: 10000 });
    await open();
    const win = await appearing;
    watch(win, who);
    // Playwright would otherwise hand the popup the main window's viewport.
    if (size) await win.setViewportSize(size);
    if (selector) await win.waitForSelector(selector, { timeout: 15000 });
    if (ready) await win.waitForTimeout(ready);
    return win;
  };

  /** Open the EQ window (SPEC §12 `E`) and adopt it. */
  const openEq = ({ from = page, who = "eq" } = {}) =>
    openPopup({
      from,
      who,
      open: () => from.keyboard.press("e"),
      selector: ".eq-canvas-wrap",
      viewport: EQ_VIEWPORT,
    });

  /** Print what was written and what went wrong, then leave with a verdict. */
  const finish = async ({ close = true } = {}) => {
    console.log(`\n${wrote.join("\n")}`);
    console.log(
      errors.length
        ? `\nPROBLEMS (${errors.length}):\n${errors.join("\n")}`
        : "\nCONSOLE: clean",
    );
    if (close) await browser.close();
    process.exit(errors.length ? 1 : 0);
  };

  return {
    root: ROOT,
    out,
    url: target,
    browser,
    context,
    page,
    errors,
    wrote,
    watch,
    settle,
    engine,
    shotOf,
    openPopup,
    openEq,
    finish,
  };
}
