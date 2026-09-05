/**
 * Theme verification harness — SPEC §14/§15. Dev tooling; not shipped.
 *
 *   npm run build:mock
 *   npx vite preview --outDir dist-mock --port 4173
 *   node scripts/shots-theme.mjs [url] [--tag dark-before]
 *
 * Three jobs:
 *
 *  1. photograph every theme this work adds — light, light at the 420 px floor,
 *     the light EQ *window* (a second document: it has to be driven as one),
 *     a custom accent in both themes, and the deck-assignment affordances of
 *     SPEC §2.8 on paper. Shots 31–36 and 45 of the one numbered set;
 *     `scripts/shots.mjs` owns 01–30 and 37–44 and `shots-themedoc.mjs` 46–55,
 *     including the dark main view, which is why this script measures dark
 *     rather than photographing it;
 *
 *  2. drive the settings panel and prove the choice reaches the engine, the
 *     token layer, a reload with the front end's cache wiped, and the EQ
 *     window (the last section — the seams, not the pictures);
 *
 *  3. prove the dark theme did not drift. The mock is alive — a moving
 *     playhead, decaying meters, an FFT — so a shot of it is never twice the
 *     same. `--tag` writes one deliberately frozen dark frame (paused, seeked
 *     to a fixed position, meters run down) under `shots/diff/`, so the same
 *     script run against the old build and the new one produces two frames that
 *     can be compared pixel for pixel.
 *
 * The theme comes from the URL (`?theme=…&accent=…&scale=…`), which
 * `applyPreviewOverride()` reads in mock builds only.
 *
 * The browser, the window size, the toast-clearing shutter, the console watch,
 * the EQ window and the exit verdict are shared with the other two harnesses —
 * see `scripts/lib/shots-base.mjs`.
 */
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { startHarness } from "./lib/shots-base.mjs";

const args = process.argv.slice(2);
const tagIx = args.indexOf("--tag");
const tag = tagIx >= 0 ? args[tagIx + 1] : null;

const { out, url, page, context, errors, settle, engine, shotOf, openEq, finish } =
  await startHarness({ argv: args });

const diffDir = join(out, "diff");

/* Only when there is something to put in it. An ordinary run left an empty
   `shots/diff/` behind, which reads as a directory somebody forgot to fill
   rather than as the two-build comparison it is for. */
if (tag) mkdirSync(diffDir, { recursive: true });

/** Load the app with an appearance forced from the URL. */
const load = async (query) => {
  await page.goto(`${url}/${query}`, { waitUntil: "networkidle" });
  await page.waitForSelector(".wave-lane", { timeout: 15000 });
  await settle(1800);
};

/** What the token layer actually resolved to — the theme, in numbers. */
const report = async (target = page) =>
  target.evaluate(() => {
    const cs = getComputedStyle(document.documentElement);
    const v = (t) => cs.getPropertyValue(t).trim();
    return {
      theme: document.documentElement.dataset.theme,
      pref: document.documentElement.dataset.themePref,
      scheme: cs.colorScheme,
      bg: v("--ink-900"),
      accent: v("--accent"),
      hi: v("--accent-hi"),
      press: v("--accent-press"),
      deep: v("--accent-deep"),
      deckB: v("--deck-b"),
      text: v("--text-hi"),
      wfCore: v("--wf-core-a"),
      wfBlend: v("--wf-blend"),
      scrim: v("--wf-scrim"),
      eqCurve: v("--eq-curve"),
      missing: ["--wf-mid", "--mt-safe", "--eq-curve", "--lane-a-rgb", "--tip-bg"].filter(
        (t) => !v(t),
      ),
    };
  });

/**
 * A canvas is the one surface that cannot inherit a colour, so read the pixels
 * back: mean luminance of the waveform lane and of the EQ graph. A light theme
 * whose canvases were left at dark's values shows up here as a lane that is
 * barely darker than the paper behind it.
 */
const canvasInk = (target, selector) =>
  target.evaluate((sel) => {
    const c = document.querySelector(sel);
    if (!c) return null;
    const ctx = c.getContext("2d");
    const { data } = ctx.getImageData(0, 0, c.width, c.height);
    let sum = 0;
    let litSum = 0;
    let min = 255;
    let max = 0;
    let lit = 0;
    for (let i = 0; i < data.length; i += 4) {
      const a = data[i + 3] / 255;
      const raw = 0.2126 * data[i] + 0.7152 * data[i + 1] + 0.0722 * data[i + 2];
      const l = raw * a;
      sum += l;
      if (a > 0.3) {
        litSum += raw;
        lit += 1;
      }
      if (a > 0.5) {
        if (l < min) min = l;
        if (l > max) max = l;
      }
    }
    const px = data.length / 4;
    return {
      mean: +(sum / px).toFixed(1),
      /* Mean luminance of the *ink* only. The EQ graph covers ~2 % of its
         canvas, so a whole-surface mean barely moves between themes even when
         every stroke changed colour; the ink's own luminance swings from
         champagne to bronze and is the honest way to tell a repainted canvas
         from a stale one. */
      litMean: lit ? +(litSum / lit).toFixed(1) : 0,
      min: Math.round(min),
      max: Math.round(max),
      coverage: +((lit / px) * 100).toFixed(1),
    };
  }, selector);

/**
 * The lane as the eye sees it: the bar layer *and* the overlay that scrims the
 * un-played half, composited off-screen and measured in two places. Reading the
 * base canvas alone hides the failure this is here to catch — a scrim tuned on
 * obsidian washes a bronze bar to within a few levels of the paper, and the
 * played half stays perfectly crisp while the rest of the lane turns to mist.
 * Returns mean luminance of a played slice and of an un-played one.
 */
const laneHalves = (target) =>
  target.evaluate(() => {
    const wrap = document.querySelector(".lane-canvas");
    if (!wrap) return null;
    const [base, over] = wrap.querySelectorAll("canvas");
    const c = document.createElement("canvas");
    c.width = base.width;
    c.height = base.height;
    const ctx = c.getContext("2d");
    // the field the lane is drawn on, so alpha composites the way it looks
    ctx.fillStyle = getComputedStyle(document.documentElement)
      .getPropertyValue("--ink-900")
      .trim();
    ctx.fillRect(0, 0, c.width, c.height);
    ctx.drawImage(base, 0, 0);
    ctx.drawImage(over, 0, 0);
    const slice = (x0, x1) => {
      const { data } = ctx.getImageData(
        Math.round(c.width * x0),
        0,
        Math.max(1, Math.round(c.width * (x1 - x0))),
        c.height,
      );
      let sum = 0;
      let ex = 0;
      const field = ctx.getImageData(0, 0, 1, 1).data;
      const fl = 0.2126 * field[0] + 0.7152 * field[1] + 0.0722 * field[2];
      for (let i = 0; i < data.length; i += 4) {
        const l = 0.2126 * data[i] + 0.7152 * data[i + 1] + 0.0722 * data[i + 2];
        sum += l;
        const d = Math.abs(l - fl);
        if (d > ex) ex = d;
      }
      return { mean: +(sum / (data.length / 4)).toFixed(1), extreme: Math.round(ex) };
    };
    const field = ctx.getImageData(0, 0, 1, 1).data;
    return {
      field: Math.round(0.2126 * field[0] + 0.7152 * field[1] + 0.0722 * field[2]),
      played: slice(0.02, 0.12),
      unplayed: slice(0.55, 0.95),
    };
  });

/* ═══════════════════════════════════════════════════════════════════════════
   1 · the frozen dark frame, for the before/after diff
   ═════════════════════════════════════════════════════════════════════════ */

if (tag) {
  // No query string on the "before" build (it has no override to read), so ask
  // for dark explicitly only when the build understands it.
  await load("?theme=dark");
  await engine("transport_pause");
  await engine("transport_seek", { secs: 42 });
  // let the meters run all the way down, so nothing in the frame is in motion
  await settle(2600);
  await shotOf(page, `main-${tag}`, { dir: diffDir });
  console.log(`frozen dark frame: ${JSON.stringify(await report())}`);
  await engine("transport_play");
  /* `--diff-only` is how the *old* build is photographed: everything below this
     point drives APIs that only exist after this work (a theme to switch to, an
     accent to override), so a build that predates them can do the frozen frame
     and nothing else. */
  if (args.includes("--diff-only")) await finish();
}

/* ═══════════════════════════════════════════════════════════════════════════
   2 · dark, measured rather than re-photographed
   ═════════════════════════════════════════════════════════════════════════ */

/* `shots/01-main-dark.png` is the dark main view of the set; a second one taken
   here would be the same picture with a different number, so this pass only
   reads the numbers light is compared against below. */
await load("?theme=dark");
await settle(2200);
const dark = await report();
const darkLane = await canvasInk(page, ".lane-canvas canvas");
const darkHalves = await laneHalves(page);
console.log(`dark:  ${JSON.stringify(dark)}`);
console.log(`dark lane canvas: ${JSON.stringify(darkLane)}`);
console.log(`dark lane, composited: ${JSON.stringify(darkHalves)}`);
if (dark.theme !== "dark") errors.push(`dark shot resolved to ${dark.theme}`);
if (dark.accent !== "#e8d9a0") errors.push(`the dark accent drifted: ${dark.accent}`);
if (dark.bg !== "#0a0a0c") errors.push(`the dark background drifted: ${dark.bg}`);
if (dark.missing.length) errors.push(`dark: tokens missing ${JSON.stringify(dark.missing)}`);

/* ═══════════════════════════════════════════════════════════════════════════
   3 · light
   ═════════════════════════════════════════════════════════════════════════ */

await load("?theme=light");
await settle(2200);
await shotOf(page, "31-theme-light-main");
const light = await report();
const lightLane = await canvasInk(page, ".lane-canvas canvas");
const lightHalves = await laneHalves(page);
console.log(`light: ${JSON.stringify(light)}`);
console.log(`light lane canvas: ${JSON.stringify(lightLane)}`);
console.log(`light lane, composited: ${JSON.stringify(lightHalves)}`);
if (light.theme !== "light") errors.push(`light shot resolved to ${light.theme}`);
if (light.scheme !== "light") errors.push(`light: color-scheme is ${light.scheme}`);
if (light.accent === dark.accent) errors.push("light re-used the dark accent");
if (light.wfBlend !== "multiply") errors.push(`light waveform blend is ${light.wfBlend}`);
if (light.missing.length) errors.push(`light: tokens missing ${JSON.stringify(light.missing)}`);

/* The point of the light canvas work, measured. Contrast against the field the
   lane sits on, in both halves of the lane: the played half is easy, the
   un-played half is where a scrim tuned on obsidian gives the game away. Light
   is held to dark's separation, not to a fraction of it. */
{
  const sep = (h) => ({
    played: Math.abs(h.played.mean - h.field),
    unplayed: Math.abs(h.unplayed.mean - h.field),
    peak: h.unplayed.extreme,
  });
  const d = sep(darkHalves);
  const l = sep(lightHalves);
  console.log(`lane separation from its field  dark ${JSON.stringify(d)}  light ${JSON.stringify(l)}`);
  if (l.played < d.played * 0.85) {
    errors.push(`the light waveform's played half is washed out: ${l.played} vs dark ${d.played}`);
  }
  if (l.unplayed < d.unplayed * 0.85) {
    errors.push(`the light waveform's un-played half is washed out: ${l.unplayed} vs dark ${d.unplayed}`);
  }
  // and the tallest bar in the un-played half has to be unmistakable, not a hint
  if (l.peak < 40) errors.push(`the light un-played bars barely register: peak ${l.peak}/255`);
}

/* 3a-ii · the deck-assignment affordances (SPEC §2.8) in light.
   The row chips and the lane drop highlight are new surfaces, and both are
   drawn in a deck accent over a *paper* field rather than obsidian: `--deck-b`
   is a different colour in each theme for exactly this reason. Photograph them
   here, and measure the one property that decides whether they work at all —
   that the lit chip is actually distinguishable from the row behind it. */
{
  const rgb = (s) => (s.match(/[\d.]+/g) ?? []).slice(0, 3).map(Number);
  const lum = ([r, g, b]) => 0.2126 * r + 0.7152 * g + 0.0722 * b;

  // put row 3 on deck B through the chip, exactly as a user would
  await page.hover(".pl-row:nth-child(3)");
  await settle(250);
  await page.click('.pl-row:nth-child(3) .deck-chip[data-deck="b"]');
  await settle(900);
  await page.hover(".pl-row:nth-child(2)"); // show the un-lit pair as well
  await settle(300);

  const chips = await page.$$eval(".pl-row .deck-chip", (nodes) =>
    nodes.map((n) => ({
      deck: n.dataset.deck,
      on: n.dataset.on === "true",
      fg: getComputedStyle(n).color,
      bg: getComputedStyle(n).backgroundColor,
      row: getComputedStyle(n.closest(".pl-row")).backgroundColor,
      o: Number(getComputedStyle(n).opacity).toFixed(2),
    })),
  );
  const litB = chips.find((c) => c.on && c.deck === "b");
  console.log(`light deck chips: ${JSON.stringify(chips.filter((c) => c.on || Number(c.o) > 0))}`);
  if (!litB) errors.push("light: no lit B chip after assigning through it");
  else {
    const ink = Math.abs(lum(rgb(litB.fg)) - lum(rgb(litB.bg)));
    console.log(`light lit B chip: ${litB.bg} on ${litB.row}, ink separation ${ink.toFixed(1)}/255`);
    if (ink < 60) errors.push(`light: the lit B chip's label barely shows on it (${ink.toFixed(1)})`);
  }

  // and the lane drop highlight, held mid-drag
  const laneB = await page.$('.wave-lane[data-deck="b"]');
  if (!laneB) errors.push("light: no lane B to drop onto");
  else {
    const row = (await page.$$(".pl-row"))[1];
    const rb = await row.boundingBox();
    const lb = await laneB.boundingBox();
    await page.mouse.move(rb.x + 140, rb.y + rb.height / 2);
    await page.mouse.down();
    await page.mouse.move(rb.x + 150, rb.y + rb.height / 2 - 12, { steps: 5 });
    await page.mouse.move(lb.x + lb.width / 2, lb.y + lb.height / 2, { steps: 20 });
    await settle(350);
    const lit = await laneB.evaluate((n) => ({
      target: n.dataset.dropTarget === "true",
      hint: getComputedStyle(n.querySelector(".lane-drop-hint")).opacity,
      colour: getComputedStyle(n.querySelector(".lane-drop-hint")).color,
    }));
    console.log(`light drop target on lane B: ${JSON.stringify(lit)}`);
    if (!lit.target || Number(lit.hint) < 0.9) {
      errors.push(`light: lane B did not offer itself as a drop target (${JSON.stringify(lit)})`);
    }
    await shotOf(page, "45-theme-light-assign");
    await page.mouse.up();
    await settle(700);
  }
}

/* 3b · the light EQ window: a second webview, themed by itself */
{
  const eq = await openEq();
  const eqReport = await report(eq);
  console.log(`light EQ window: ${JSON.stringify(eqReport)}`);
  if (eqReport.theme !== "light") {
    errors.push(`the EQ window did not follow the theme: ${eqReport.theme}`);
  }
  const wrap = await eq.$(".eq-canvas-wrap");
  const box = await wrap.boundingBox();
  // a curve worth photographing: HP, a cut, a presence lift, an air shelf
  for (const [fx, fy] of [
    [0.2, 0.62],
    [0.44, 0.34],
    [0.66, 0.6],
    [0.83, 0.4],
  ]) {
    await eq.mouse.click(box.x + box.width * fx, box.y + box.height * fy);
    await eq.waitForTimeout(170);
  }
  await eq.waitForTimeout(1600);
  await shotOf(eq, "32-theme-light-eq-window");
  console.log(`light EQ canvas: ${JSON.stringify(await canvasInk(eq, ".eq-canvas-wrap canvas"))}`);

  /* The bug this whole exercise is about: a canvas that keeps the palette it
     was first painted with. Switch the theme *while the window is open* and
     read the pixels back — both windows, both canvas kinds. */
  const before = {
    main: await canvasInk(page, ".lane-canvas canvas"),
    eq: await canvasInk(eq, ".eq-canvas-wrap canvas"),
  };
  await page.evaluate(() => window.__onyxTheme.applyAppearance({ theme: "dark" }));
  await settle(1200);
  await eq.waitForTimeout(1200);
  const after = {
    main: await canvasInk(page, ".lane-canvas canvas"),
    eq: await canvasInk(eq, ".eq-canvas-wrap canvas"),
    mainTheme: (await report()).theme,
    eqTheme: (await report(eq)).theme,
  };
  console.log(`live switch light→dark: before ${JSON.stringify(before)} after ${JSON.stringify(after)}`);
  if (after.mainTheme !== "dark") errors.push("the main window ignored a live theme change");
  if (after.eqTheme !== "dark") errors.push("the EQ window ignored a live theme change");
  // the waveform's static layer is the one that goes stale
  if (Math.abs(after.main.mean - before.main.mean) < 4) {
    errors.push(`the waveform canvas is stale after a theme change: ${JSON.stringify([before.main, after.main])}`);
  }
  // the EQ redraws every frame, so what has to change is the colour of its ink
  if (Math.abs(after.eq.litMean - before.eq.litMean) < 25) {
    errors.push(`the EQ canvas is stale after a theme change: ${JSON.stringify([before.eq, after.eq])}`);
  }
  await eq.close();
  await settle(500);
}

/* 3c · the window floor (420 × 560, tauri.conf.json), in *both* themes.
   Light is the one photographed (shot 33; dark at 420 is shot 18 in
   `shots.mjs`), but the measurement runs in both, because a layout that only
   fits in one theme fits by luck: the two themes differ in font weight and in
   border thickness, and the transport row is where that shows.

   The transport read-out is checked explicitly and at its worst case. At 420 px
   the transport is a two-row grid (app.css, "≤599: the transport splits") with
   the elapsed/total time in a `minmax(0, 1fr)` column between the buttons, so a
   read-out that could break across lines would push the row's height and shove
   the playlist up — and the read-out only reaches its full width on an
   hour-long file, which is exactly the case a run against the mock's short
   demo tracks never shows. So the time is set to `1:59:59 / 2:34:56` and
   measured: one line, no overflow, clear of the loop button, and a transport
   the same height as before. `white-space: nowrap` on `.tr-time` is what keeps
   that true; this is the assertion that notices if it goes. */
for (const theme of ["light", "dark"]) {
  await load(`?theme=${theme}`);
  await page.setViewportSize({ width: 420, height: 560 });
  await settle(1600);
  if (theme === "light") await shotOf(page, "33-theme-light-420");
  const over = await page.evaluate(() => {
    const doc = document.documentElement;
    const list = [];
    if (doc.scrollWidth > doc.clientWidth) list.push(`document +${doc.scrollWidth - doc.clientWidth}`);
    for (const n of document.querySelectorAll(".titlebar, .wave-stack, .wave-meters, .playlist, .transport, .ab-rail")) {
      const d = n.scrollWidth - n.clientWidth;
      if (d > 1) list.push(`${n.className} +${d}`);
    }
    return list;
  });
  console.log(`${theme} 420×560 overflow: ${over.length ? JSON.stringify(over) : "none"}`);
  if (over.length) errors.push(`${theme} 420: ${JSON.stringify(over)}`);

  const time = await page.evaluate(() => {
    const box = document.querySelector(".tr-time");
    const pos = box.querySelector(".tr-pos");
    const dur = box.querySelector(".tr-dur");
    const loop = document.querySelector(".transport > .tr-toggle");
    const row = document.querySelector(".transport");
    // A wrapped inline box reports one client rect per line, so the line count
    // is read from the layout rather than guessed from a width comparison.
    const measure = () => ({
      text: `${pos.textContent} / ${dur.textContent}`,
      lines: box.getClientRects().length,
      width: Math.round(box.getBoundingClientRect().width),
      height: Math.round(box.getBoundingClientRect().height),
      overflow: box.scrollWidth - box.clientWidth,
      rowOverflow: row.scrollWidth - row.clientWidth,
      rowHeight: Math.round(row.getBoundingClientRect().height),
      clearsLoop: Math.round(loop.getBoundingClientRect().left - box.getBoundingClientRect().right),
    });
    const now = measure();
    // The frame loop rewrites these on the next tick, so this is a measurement,
    // not a change to the app's state.
    pos.textContent = "1:59:59";
    dur.textContent = "2:34:56";
    const worst = measure();
    /* And the *capability*, which is the thing that actually regresses. An
       hour-long read-out still fits at 420 px with room to spare, so it would
       keep passing the moment `white-space: nowrap` was deleted; a read-out
       wider than the column it sits in would not. It is not a layout the app can
       produce — no file is 111 hours long — it asks the one question the
       property answers: *can* this box break across lines? If it can, the
       transport grows, because the row is `auto`-height below 600 px. */
    pos.textContent = "111:59:59";
    dur.textContent = "222:34:56";
    const cannotWrap = { ...measure(), ws: getComputedStyle(box).whiteSpace };
    pos.textContent = now.text.split(" / ")[0];
    dur.textContent = now.text.split(" / ")[1];
    return { now, worst, cannotWrap };
  });
  console.log(`${theme} 420 transport time: now ${JSON.stringify(time.now)}`);
  console.log(`${theme} 420 transport time: worst ${JSON.stringify(time.worst)}`);
  console.log(`${theme} 420 transport time: cannot wrap ${JSON.stringify(time.cannotWrap)}`);
  for (const [when, m] of Object.entries(time)) {
    if (m.lines !== 1) errors.push(`${theme} 420: the time read-out wrapped to ${m.lines} lines (${when}, "${m.text}")`);
    // Only the two read-outs the app can really print are held to fitting; the
    // 111-hour one is there to be too wide.
    if (when === "cannotWrap") continue;
    if (m.overflow > 1) errors.push(`${theme} 420: the time read-out overflows by ${m.overflow}px (${when})`);
    if (m.rowOverflow > 1) errors.push(`${theme} 420: the transport overflows by ${m.rowOverflow}px (${when})`);
    if (m.clearsLoop < 0) errors.push(`${theme} 420: the time read-out runs into the loop button by ${-m.clearsLoop}px (${when})`);
  }
  for (const [when, m] of [
    ["an hour-long read-out", time.worst],
    ["a read-out too wide for its column", time.cannotWrap],
  ]) {
    if (m.rowHeight !== time.now.rowHeight) {
      errors.push(
        `${theme} 420: ${when} changed the transport height ` +
          `${time.now.rowHeight} → ${m.rowHeight}px`,
      );
    }
  }
  if (!/nowrap|pre/.test(time.cannotWrap.ws)) {
    errors.push(`${theme} 420: .tr-time may wrap (white-space: ${time.cannotWrap.ws})`);
  }
}
await page.setViewportSize({ width: 1180, height: 760 });

/* 3d · the listeners `initAppearance()` installs come off again.
   `theme.ts` binds `resize`, `storage` and the capture-phase reset chord on
   `window` at module scope, in every window. The `resize` one used to be an
   anonymous function nobody kept a handle to, so every webview that came and
   went — the EQ window, the editor, a preview popup, a hot update — left one
   attached to a document that would never be painted again, holding this
   module's closure and its stale `current` appearance.

   Counted rather than reasoned about, through CDP's own listener table (the
   same one DevTools' "Event Listeners" pane shows), because a listener leak is
   invisible from inside the page: `removeEventListener` is not observable and a
   leaked one is not either. Three facts: a second `initAppearance()` does not
   double up (the `installed` latch), `teardownAppearance()` hands all three
   back, and a later `initAppearance()` installs a fresh set. */
{
  await load("?theme=dark");
  const cdp = await context.newCDPSession(page);
  const OWNED = ["resize", "storage", "keydown"];
  /** How many listeners `window` itself carries, by event type. */
  const onWindow = async () => {
    const { result } = await cdp.send("Runtime.evaluate", { expression: "window" });
    const { listeners } = await cdp.send("DOMDebugger.getEventListeners", {
      objectId: result.objectId,
      depth: 0,
    });
    await cdp.send("Runtime.releaseObject", { objectId: result.objectId });
    const counts = {};
    for (const l of listeners) counts[l.type] = (counts[l.type] ?? 0) + 1;
    return counts;
  };
  const theme = (call) => page.evaluate((c) => window.__onyxTheme[c](), call);
  /** Take `--zoom` away, fire a resize, and report whether it came back. */
  const zoomOnResize = () =>
    page.evaluate(() => {
      const root = document.documentElement;
      const before = root.style.getPropertyValue("--zoom");
      root.style.removeProperty("--zoom");
      window.dispatchEvent(new Event("resize"));
      return { before, after: root.style.getPropertyValue("--zoom") };
    });

  const base = await onWindow();
  await theme("initAppearance");
  const again = await onWindow();
  await theme("teardownAppearance");
  const torn = await onWindow();
  /* Functionally, not just numerically: a torn-down module must not still be
     applying zoom from its own `current`. `--zoom` is what `onResize` writes, so
     take it away and see whether a resize puts it back. */
  const deaf = await zoomOnResize();
  await theme("initAppearance");
  const fresh = await onWindow();
  const say = (c) => OWNED.map((t) => `${t}:${c[t] ?? 0}`).join(" ");
  console.log(`window listeners  installed [${say(base)}]  init twice [${say(again)}]`);
  console.log(`window listeners  torn down [${say(torn)}]  re-inited  [${say(fresh)}]`);
  for (const type of OWNED) {
    const had = base[type] ?? 0;
    if (had < 1) {
      errors.push(`theme.ts installed no ${type} listener at all`);
      continue;
    }
    if ((again[type] ?? 0) !== had) {
      errors.push(`a second initAppearance() changed the ${type} count ${had} → ${again[type]}`);
    }
    if ((torn[type] ?? 0) !== had - 1) {
      errors.push(
        `teardownAppearance() left ${torn[type] ?? 0} ${type} listeners, expected ${had - 1}`,
      );
    }
    if ((fresh[type] ?? 0) !== had) {
      errors.push(`initAppearance() after a teardown installed ${fresh[type] ?? 0} ${type}`);
    }
  }
  // The re-installed set has to *work*, not just be counted: a torn-down module
  // that comes back deaf is the same bug wearing a different hat.
  const live = await zoomOnResize();
  console.log(
    `resize after a teardown: ${JSON.stringify(deaf)}  after a re-init: ${JSON.stringify(live)}`,
  );
  if (deaf.after !== "") {
    errors.push(`a resize still reached theme.ts after teardownAppearance(): ${deaf.after}`);
  }
  if (live.after === "" || live.after !== live.before) {
    errors.push(`the re-installed resize handler did not restore --zoom: ${JSON.stringify(live)}`);
  }
  await cdp.detach();
}

/* ═══════════════════════════════════════════════════════════════════════════
   4 · a custom accent, derived in both themes
   ═════════════════════════════════════════════════════════════════════════ */

const CUSTOM = "%235b8def"; // a cold blue: nothing about it is champagne
for (const [name, theme] of [
  ["34-accent-custom-dark", "dark"],
  ["35-accent-custom-light", "light"],
]) {
  await load(`?theme=${theme}&accent=${CUSTOM}`);
  await settle(2000);
  await shotOf(page, name);
  const r = await report();
  console.log(`custom accent (${theme}): ${JSON.stringify(r)}`);
  const family = [r.accent, r.hi, r.press, r.deep];
  if (new Set(family).size !== 4) errors.push(`${theme}: accent variants collapsed: ${family}`);
  if (family.includes("#e8d9a0")) errors.push(`${theme}: a champagne value survived a custom accent`);
  // deck B must stay separable from deck A, which *is* the accent
  if (r.deckB.toLowerCase() === r.accent.toLowerCase()) {
    errors.push(`${theme}: deck B collided with the accent`);
  }
}

/* ═══════════════════════════════════════════════════════════════════════════
   5 · `system`, reacting without a restart
   ═════════════════════════════════════════════════════════════════════════ */

{
  await context.clearCookies();
  await page.emulateMedia({ colorScheme: "dark" });
  await load("?theme=system");
  const first = await report();
  await page.emulateMedia({ colorScheme: "light" });
  await settle(900);
  const second = await report();
  await page.emulateMedia({ colorScheme: "dark" });
  await settle(900);
  const third = await report();
  console.log(
    `system: OS dark → ${first.theme}, OS light → ${second.theme}, OS dark again → ${third.theme}`,
  );
  if (first.theme !== "dark" || second.theme !== "light" || third.theme !== "dark") {
    errors.push(`\`system\` did not follow the OS: ${[first.theme, second.theme, third.theme]}`);
  }
  if (second.pref !== "system") errors.push(`the preference was lost: ${second.pref}`);
  await page.emulateMedia({ colorScheme: null });
}

/* ═══════════════════════════════════════════════════════════════════════════
   6 · the seam: settings panel → theme module → engine → reload → EQ window

   Every step above forced an appearance from the URL, which is exactly the half
   the shipped app does *not* have. This section drives the surface a user
   drives — the settings panel — and then proves the choice outlives the window:

     panel click → `applyAppearance()` (the DOM)      · the theming seam
                 → `set_appearance` (the engine)      · the IPC seam
                 → reload with the front-end cache wiped
                 → `snapshot.appearance` re-themes it · the persistence seam
                 → the EQ window opens already light  · the second-window seam

   The preview's back end keeps the appearance the way `settings.json` does; what
   it cannot stand in for is a real relaunch of a real Tauri app, which is called
   out in the report rather than implied here.
   ═════════════════════════════════════════════════════════════════════════ */

{
  // Start from the shipped defaults: no URL override, no cache, nothing the
  // engine remembers from the sections above.
  await page.goto(url, { waitUntil: "domcontentloaded" });
  await page.evaluate(() => {
    window.localStorage.clear();
    window.sessionStorage.clear();
  });
  await load("");
  const fresh = await report();
  console.log(`fresh preview: ${JSON.stringify({ theme: fresh.theme, accent: fresh.accent })}`);
  if (fresh.theme !== "dark") errors.push(`a fresh preview came up ${fresh.theme}`);

  // ── the panel ──────────────────────────────────────────────────────────
  await page.click('button[aria-label="Settings"]');
  await page.waitForSelector(".float-panel.settings", { timeout: 5000 });
  /* Appearance has two faces since SPEC §20, and the theme document is the
     default one. The six pickers this section drives — theme, accent, size,
     read-out font — are under "Simple"; the other face is driven end to end by
     `scripts/shots-themedoc.mjs`. Selecting the tab here rather than assuming
     it is what keeps this script measuring the *pickers*' seam to the engine
     and not the document's. */
  await page.click('.float-panel.settings .set-tabs button:text-is("Simple")');
  await settle(600);

  const themeButton = (label) =>
    page.click(`.float-panel.settings .field:has(.k:text-is("Theme")) .seg button:text-is("${label}")`);
  const scaleButton = (label) =>
    page.click(`.float-panel.settings .field:has(.k:text-is("Size")) .seg button:text-is("${label}")`);

  await themeButton("Light");
  await settle(700);
  await page.click('.set-swatches .set-swatch[aria-label="Verdigris"]');
  await settle(600);
  await scaleButton("Compact");
  await settle(600);
  await page.selectOption('[data-set="num-font"]', "menlo");
  await settle(700);

  /** What the engine was told, and what the DOM is showing, side by side. */
  const both = async (target = page) => {
    const dom = await target.evaluate(() => {
      const r = document.documentElement;
      const cs = getComputedStyle(r);
      return {
        theme: r.dataset.theme,
        pref: r.dataset.themePref,
        scale: r.dataset.scale,
        numFont: r.dataset.numFont,
        accentVar: cs.getPropertyValue("--accent").trim(),
      };
    });
    const engineSide = await engine("app_state");
    return { dom, engine: engineSide.appearance };
  };

  const chosen = await both();
  console.log(`panel → DOM/engine: ${JSON.stringify(chosen)}`);
  const want = { theme: "light", accent: "#3f8f8a", numericFont: "menlo", sizeScale: "compact" };
  for (const [k, v] of Object.entries(want)) {
    if (chosen.engine?.[k] !== v) {
      errors.push(`the panel's ${k} did not reach the engine: ${chosen.engine?.[k]} ≠ ${v}`);
    }
  }
  if (chosen.dom.theme !== "light") errors.push(`the DOM did not follow the panel: ${chosen.dom.theme}`);
  if (chosen.dom.scale !== "compact") errors.push(`the size scale did not reach the DOM: ${chosen.dom.scale}`);
  if (chosen.dom.numFont !== "menlo") errors.push(`the read-out font did not reach the DOM: ${chosen.dom.numFont}`);
  // The accent the panel sent is the *chosen* one; `--accent` holds the tone
  // derived for this theme, so what matters is that it stopped being champagne.
  if (/e8d9a0|c9a227/i.test(chosen.dom.accentVar)) {
    errors.push(`the accent did not reach the token layer: ${chosen.dom.accentVar}`);
  }
  await page.keyboard.press("Escape");
  await settle(400);

  // ── the reload, with the front end's own memory taken away ─────────────
  await page.evaluate(() => window.localStorage.clear());
  await page.reload({ waitUntil: "networkidle" });
  await page.waitForSelector(".wave-lane", { timeout: 15000 });
  await settle(2000);
  const reloaded = await both();
  console.log(`after a reload with the cache wiped: ${JSON.stringify(reloaded)}`);
  if (reloaded.dom.theme !== "light") {
    errors.push(`the appearance did not survive a reload: ${reloaded.dom.theme}`);
  }
  if (reloaded.dom.scale !== "compact" || reloaded.dom.numFont !== "menlo") {
    errors.push(`only half the appearance survived: ${JSON.stringify(reloaded.dom)}`);
  }
  if (reloaded.engine?.accent !== want.accent) {
    errors.push(`the engine forgot the accent: ${reloaded.engine?.accent}`);
  }

  // ── and the second window, which never saw the panel ───────────────────
  const eq = await openEq();
  const eqSide = await report(eq);
  console.log(`EQ window after the reload: ${JSON.stringify({ theme: eqSide.theme, accent: eqSide.accent })}`);
  if (eqSide.theme !== "light") {
    errors.push(`the EQ window did not inherit the persisted theme: ${eqSide.theme}`);
  }
  if (eqSide.accent === fresh.accent) {
    errors.push(`the EQ window kept the default accent: ${eqSide.accent}`);
  }
  await eq.close();
  await settle(400);
  await shotOf(page, "36-appearance-persisted-light");
}

await finish();
