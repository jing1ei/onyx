/**
 * Visual verification harness: drives the real UI against the mock backend in
 * Chromium and writes shots/*.png. Not shipped; dev tooling only.
 *
 *   npm run build:mock
 *   npx vite preview --outDir dist-mock --port 4173
 *   node scripts/shots.mjs [url]
 *
 * One numbered set, three scripts, no gaps and no collisions:
 *
 *   01–30, 37–44   this file          the app itself
 *   31–36, 45      shots-theme.mjs    the light theme and custom accents (§14/§15)
 *   46–55          shots-themedoc.mjs the theme document (§20)
 *
 * A shot that moves between scripts keeps its number; a new one takes the next
 * free number at the end. Run all three against the same preview to refresh
 * the set.
 *
 * The browser, the window size, the toast-clearing shutter, the console watch
 * and the exit verdict are shared with the other two harnesses — see
 * `scripts/lib/shots-base.mjs`.
 */
import { startHarness, EQ_VIEWPORT } from "./lib/shots-base.mjs";

const { page, url, errors, settle, engine, shotOf, openEq, finish } = await startHarness();
const shot = (name) => shotOf(page, name);

await page.goto(url, { waitUntil: "networkidle" });
await page.waitForSelector(".wave-lane", { timeout: 15000 });
await settle(1400);

/* 1 · main view. The mock boots mid-track and already playing, so leave the
   transport alone — pressing Space here would pause it and silence the FFT. */
await settle(2200);
await shot("01-main-dark");

/* 2 · monitor fold badge — `side`, the one that sounds most broken */
await page.keyboard.press("s");
await settle(600);
await shot("02-monitor-side");
await page.keyboard.press("s");
await settle(300);

/* 2b · A/B switched off: one lane, and — the bug this shot exists for — one
   highlighted playlist row, not two. Deck B keeps its material, so the second
   row must lose both the highlight and its badge, not just dim. */
await page.click('.ab-cluster > .tr-toggle');
await settle(800);
await shot("03-ab-off");
{
  const rows = await page.$$eval(".pl-row", (nodes) =>
    nodes
      .map((n, i) => ({
        i: i + 1,
        playing: n.dataset.playing === "true",
        loaded: n.dataset.loaded === "true",
        deck: n.dataset.deck ?? null,
      }))
      .filter((r) => r.playing || r.loaded || r.deck),
  );
  console.log(`A/B off, marked rows: ${JSON.stringify(rows)}`);
  const playing = rows.filter((r) => r.playing);
  if (playing.length !== 1) errors.push(`A/B off: ${playing.length} playing rows, expected 1`);
  if (rows.some((r) => r.loaded)) errors.push("A/B off: a second row still reads as loaded");
}
await page.click('.ab-cluster > .tr-toggle');
await settle(600);

/* 2c · the A/B deck switch, before and after clicking the other lane. The
   bug the user reported: the lane highlight and the AUDIBLE badge did not
   follow the click. Assert the DOM as well as photograph it — "the badge is
   on lane B" is a fact, not an impression. */
{
  const lanes = () =>
    page.$$eval(".wave-lane", (nodes) =>
      nodes.map((n) => ({
        deck: n.dataset.deck,
        active: n.dataset.active === "true",
        badge: !!n.querySelector(".lane-audible"),
        opacity: Number(getComputedStyle(n).opacity).toFixed(2),
      })),
    );
  const seg = () =>
    page.$$eval(".ab-cluster .seg button", (nodes) =>
      nodes.map((n) => `${n.textContent.trim()}:${n.dataset.on}`),
    );
  const rows = () =>
    page.$$eval(".pl-row", (nodes) =>
      nodes.map((n, i) => (n.dataset.playing === "true" ? `row${i + 1}:${n.dataset.deck}` : null)).filter(Boolean),
    );

  const before = { lanes: await lanes(), seg: await seg(), rows: await rows() };
  await shot("04-ab-deck-a");

  const laneB = (await page.$$(".wave-lane")).at(-1);
  const bb = await laneB.boundingBox();
  // a plain click on lane B: makes B audible *and* seeks, both by design
  await page.mouse.click(bb.x + bb.width * 0.42, bb.y + bb.height * 0.6);
  await settle(700);
  const after = { lanes: await lanes(), seg: await seg(), rows: await rows() };
  await shot("05-ab-deck-b");

  console.log(`deck switch before: ${JSON.stringify(before)}`);
  console.log(`deck switch after:  ${JSON.stringify(after)}`);
  const audible = (st) => st.lanes.filter((l) => l.active && l.badge).map((l) => l.deck);
  if (audible(before).join() !== "a") errors.push(`expected lane A audible first, got ${JSON.stringify(before.lanes)}`);
  if (audible(after).join() !== "b") errors.push(`lane badge did not follow the click: ${JSON.stringify(after.lanes)}`);
  if (!after.seg.includes("B:true")) errors.push(`transport A|B disagrees with the lanes: ${after.seg}`);
  if (after.rows.some((r) => r.endsWith(":a"))) errors.push(`playlist still marks deck A audible: ${after.rows}`);

  // back to A, which also proves the switch is not one-way
  const laneA = (await page.$$(".wave-lane"))[0];
  const ab = await laneA.boundingBox();
  await page.mouse.click(ab.x + ab.width * 0.3, ab.y + ab.height * 0.6);
  await settle(600);
  const back = await lanes();
  if (audible({ lanes: back }).join() !== "a") errors.push(`could not switch back to A: ${JSON.stringify(back)}`);

  // Alt-drag lane B must slide the offset without switching deck (SPEC §11)
  await page.keyboard.down("Alt");
  await page.mouse.move(bb.x + bb.width * 0.5, bb.y + bb.height * 0.5);
  await page.mouse.down();
  await page.mouse.move(bb.x + bb.width * 0.5 + 18, bb.y + bb.height * 0.5, { steps: 12 });
  await page.mouse.up();
  await page.keyboard.up("Alt");
  await settle(500);
  const afterAlt = await lanes();
  const offset = await page.$eval(".off-ms", (n) => n.textContent.trim());
  console.log(`after alt-drag: offset ${offset}, audible ${JSON.stringify(audible({ lanes: afterAlt }))}`);
  if (audible({ lanes: afterAlt }).join() !== "a") {
    errors.push("Alt-drag on lane B switched the audible deck");
  }
  if (/^[+\u2212]?0\.00/.test(offset)) errors.push(`Alt-drag did not move the offset: ${offset}`);
  await page.click(".align-group > button:last-of-type"); // Reset
  await settle(400);
}

/* 2c-ii · the lane modifiers, separated.
   Three gestures share one pointer on the same 40 px of canvas — plain =
   seek/scrub, `Shift` = loop region, `Alt` on lane B = the A/B time offset
   (SPEC §11) — and the last must never also seek. That has to be measured
   against the *engine*, with the transport stopped: a position that moved
   because the track was playing looks exactly like a stray seek, which is the
   false alarm this block exists to rule out. */
{
  const st = async () => {
    const s = await engine("app_state");
    return {
      pos: Math.round(s.transport.positionSecs * 1000) / 1000,
      loop: s.transport.loopRegion,
      deck: s.transport.activeDeck,
      off: s.ab.abOffsetFrames,
      rate: s.transport.engineSampleRate,
    };
  };
  const lane = async (i) => (await page.$$(".wave-lane .lane-canvas"))[i].boundingBox();
  const drag = async (i, from, to, modifier) => {
    const b = await lane(i);
    const y = b.y + b.height / 2;
    if (modifier) await page.keyboard.down(modifier);
    await page.mouse.move(b.x + b.width * from, y);
    await page.mouse.down();
    await page.mouse.move(b.x + b.width * to, y, { steps: 14 });
    await page.mouse.up();
    if (modifier) await page.keyboard.up(modifier);
    await settle(450);
    return b;
  };

  await engine("transport_pause");
  await settle(400);

  const base = await st();
  const b = await drag(1, 0.5, 0.53, "Alt");
  const alt = await st();
  console.log(
    `alt-drag B: pos ${base.pos}→${alt.pos} · deck ${base.deck}→${alt.deck} · ` +
      `offset ${base.off}→${alt.off} frames (${((alt.off / alt.rate) * 1000).toFixed(1)} ms, ` +
      `${(Math.abs((alt.off - base.off) / alt.rate) * 1000 / (b.width * 0.03)).toFixed(0)} ms/px)`,
  );
  if (alt.pos !== base.pos) errors.push(`Alt-drag on lane B seeked: ${base.pos} → ${alt.pos}`);
  if (alt.deck !== base.deck) errors.push("Alt-drag on lane B switched the audible deck");
  if (alt.off === base.off) errors.push("Alt-drag on lane B did not move the offset");
  if (alt.loop) errors.push("Alt-drag on lane B set a loop region");

  // Dragging past the end must stop at the ±30 s the engine accepts (SPEC §11).
  await drag(1, 0.05, 0.95, "Alt");
  const clamped = await st();
  const secs = clamped.off / clamped.rate;
  console.log(`alt-drag to the far edge: ${clamped.off} frames = ${secs.toFixed(3)} s`);
  if (Math.abs(secs) > 30.0001) errors.push(`the A/B offset ran past ±30 s: ${secs}`);
  await page.click(".align-group > button:last-of-type"); // Reset
  await settle(350);

  // Shift = loop region, on either lane, and it must not seek either.
  const beforeShift = await st();
  await drag(1, 0.3, 0.6, "Shift");
  const shift = await st();
  console.log(`shift-drag B: loop ${JSON.stringify(shift.loop)} · pos ${beforeShift.pos}→${shift.pos}`);
  if (!shift.loop) errors.push("Shift-drag on lane B set no loop region");
  if (shift.pos !== beforeShift.pos) errors.push(`Shift-drag seeked: ${beforeShift.pos} → ${shift.pos}`);
  if (shift.off !== beforeShift.off) errors.push("Shift-drag moved the A/B offset");

  // …and plain still seeks, which is the gesture the other two must not become.
  await drag(0, 0.8, 0.8);
  const plain = await st();
  console.log(`plain drag A: pos ${shift.pos}→${plain.pos}`);
  if (plain.pos === shift.pos) errors.push("a plain drag on a lane no longer seeks");

  await engine("set_loop_region", { region: null });
  await engine("transport_play");
  await settle(500);
}

/* 2d · the width ladder (app.css, "Narrow windows"). Two lanes, A/B on, the
   hardest case at every step: 1180 is `01-main`, then each breakpoint tier and
   finally the minimum window from tauri.conf.json. Overflow is asserted, not
   eyeballed — a layout that "fits" by clipping is the failure mode. */
const WIDTHS = [
  ["15-w900", 900, 760],
  ["16-w640", 640, 760],
  ["17-w480", 480, 760],
  ["18-w420-dark", 420, 760],
  ["19-min-window-420x560", 420, 560],
];
for (const [name, w, h] of WIDTHS) {
  await page.setViewportSize({ width: w, height: h });
  await settle(900);
  await shot(name);
  const report = await page.evaluate(() => {
    const over = [];
    const doc = document.documentElement;
    if (doc.scrollWidth > doc.clientWidth) over.push(`document +${doc.scrollWidth - doc.clientWidth}`);
    for (const n of document.querySelectorAll(
      ".titlebar, .tb-center, .wave-stack, .wave-lanes, .wave-meters, .wm-stack, .wm-grid, .playlist, .pl-cols, .pl-row, .ab-rail, .align-group, .nudges, .transport, .ab-cluster, .vol",
    )) {
      const d = n.scrollWidth - n.clientWidth;
      if (d > 1) over.push(`${n.className} +${d}`);
    }
    // hit targets: nothing a finger has to find may be under 20px
    const small = [];
    for (const n of document.querySelectorAll("button")) {
      const r = n.getBoundingClientRect();
      if (r.width === 0 && r.height === 0) continue; // display:none by design
      if (r.width < 20 || r.height < 18) small.push(`${n.className || n.title}: ${Math.round(r.width)}\u00d7${Math.round(r.height)}`);
    }
    // the smallest rendered font on screen, so "legible" is measurable
    let min = 99;
    for (const n of document.querySelectorAll("body *")) {
      if (!n.textContent?.trim() || n.children.length) continue;
      const r = n.getBoundingClientRect();
      if (r.width < 1 || r.height < 1) continue;
      min = Math.min(min, parseFloat(getComputedStyle(n).fontSize));
    }
    const lane = document.querySelector(".lane-canvas");
    // Track selection is one of the three things a narrow window must keep, so
    // the playlist has to show whole rows, not a sliver of one: count the rows
    // that fit entirely inside the scroller.
    const rowsBox = document.querySelector(".pl-rows");
    const rowH = document.querySelector(".pl-row")?.getBoundingClientRect().height ?? 0;
    const wholeRows = rowsBox && rowH ? Math.floor(rowsBox.clientHeight / rowH) : 0;
    // Canvas correctness: every backing store must match its CSS box times the
    // device pixel ratio, or the bars are resampled and stop being crisp.
    const dpr = window.devicePixelRatio || 1;
    const canvasOff = [];
    for (const c of document.querySelectorAll("canvas")) {
      const r = c.getBoundingClientRect();
      if (r.width < 2 || r.height < 2) continue;
      const wantW = Math.round(r.width * dpr);
      const wantH = Math.round(r.height * dpr);
      if (Math.abs(c.width - wantW) > 1 || Math.abs(c.height - wantH) > 1) {
        canvasOff.push(`${c.parentElement?.className}: ${c.width}×${c.height} vs ${wantW}×${wantH}`);
      }
    }
    return {
      over,
      small,
      minFont: min,
      lane: lane ? Math.round(lane.getBoundingClientRect().height) : 0,
      wholeRows,
      rowH: Math.round(rowH),
      canvasOff,
    };
  });
  console.log(
    `${name} (${w}×${h}): overflow ${report.over.length ? JSON.stringify(report.over) : "none"} · ` +
      `small targets ${report.small.length ? JSON.stringify(report.small) : "none"} · ` +
      `min font ${report.minFont}px · lane ${report.lane}px · ` +
      `playlist ${report.wholeRows} whole rows of ${report.rowH}px · ` +
      `canvas ${report.canvasOff.length ? JSON.stringify(report.canvasOff) : "dpr-exact"}`,
  );
  if (report.over.length) errors.push(`${name} overflow: ${JSON.stringify(report.over)}`);
  if (report.small.length) errors.push(`${name} hit targets: ${JSON.stringify(report.small)}`);
  if (report.minFont < 8.5) errors.push(`${name} font too small: ${report.minFont}px`);
  if (report.wholeRows < 2) errors.push(`${name} playlist shows ${report.wholeRows} whole rows`);
  if (report.canvasOff.length) errors.push(`${name} canvas not dpr-exact: ${JSON.stringify(report.canvasOff)}`);
}

/* 2e · blind test in the narrow window. The meter cluster is a strip here and
   its masks are laid out by different rules than the column's (app.css, "Narrow
   windows"); a mask that collapses, or a read-out that survives the reflow, is a
   leak, and a leak makes ABX worthless (SPEC §7). */
await page.setViewportSize({ width: 420, height: 760 });
await settle(800);
await page.click(".blind-btn");
await settle(500);
await page.click('.proto:has-text("ABX")').catch(() => {});
await settle(250);
await page.click('.solid-btn:has-text("Begin")');
await settle(1200);
await shot("20-w420-blind");
{
  const leak = await page.evaluate(() => {
    const strip = document.querySelector(".wave-meters");
    // the mask's own explanation ("0.4 LUFS is visible…") is not a read-out
    const text = [...(strip?.children ?? [])]
      .flatMap((n) => [...(n.children ?? [n])])
      .filter((n) => !n.classList.contains("wm-note"))
      .map((n) => n.innerText ?? "")
      .join(" ")
      .replace(/\s+/g, " ");
    const masks = [...document.querySelectorAll(".wave-meters .masked-panel")].map((n) => {
      const r = n.getBoundingClientRect();
      return `${n.className.split(" ").pop()} ${Math.round(r.width)}×${Math.round(r.height)}`;
    });
    const lanes = document.querySelectorAll(".wave-lane").length;
    // The one lane must be deck A's, *pinned* — painting the audible deck made
    // the picture change the instant you switched slots, and in ABX the X lane
    // was a pixel-for-pixel match of whichever of A/B it was. `data-deck` and
    // the badge carry the same answer in the DOM, where reading it is even
    // easier than off the canvas (SPEC §7).
    const identity = [...document.querySelectorAll(".wave-lane")].map((n) => ({
      deck: n.dataset.deck ?? null,
      badge: n.querySelector(".deck-badge")?.textContent?.trim() ?? null,
      ghost: n.querySelector(".deck-badge")?.dataset.ghost ?? null,
    }));
    const doc = document.documentElement;
    return {
      digits: /[0-9]/.test(text.replace(/LUFS|CORR|LRA|TP|M|S/g, "")),
      text,
      masks,
      lanes,
      identity,
      over: doc.scrollWidth - doc.clientWidth,
    };
  });
  console.log(`420 blind: lanes ${leak.lanes} · masks ${JSON.stringify(leak.masks)} · strip "${leak.text}"`);
  console.log(`420 blind: lane identity ${JSON.stringify(leak.identity)}`);
  if (leak.digits) errors.push(`420 blind: a read-out survived masking: "${leak.text}"`);
  if (leak.identity.some((l) => l.deck !== "a")) {
    errors.push(`420 blind: a lane names a deck other than A: ${JSON.stringify(leak.identity)}`);
  }
  if (leak.identity.some((l) => l.ghost !== "true")) {
    errors.push(`420 blind: the deck badge is not ghosted: ${JSON.stringify(leak.identity)}`);
  }
  if (leak.masks.some((m) => / 0×| ×0/.test(m))) errors.push(`420 blind: a mask collapsed: ${JSON.stringify(leak.masks)}`);
  if (!leak.masks.length) errors.push("420 blind: no masked panels in the strip");
  if (leak.lanes !== 1) errors.push(`420 blind: ${leak.lanes} lanes visible, expected 1`);
  if (leak.over > 0) errors.push(`420 blind: overflow +${leak.over}`);
  // The rail and the meters only come back when the *engine* says the test is
  // over, so abort and wait for that rather than for the panel to shut.
  await page.click('.float-panel.blind .solid-btn.quiet:has-text("Abort")');
  await page.waitForSelector(".ab-rail", { timeout: 8000 });
  await settle(400);
  await page.keyboard.press("Escape");
  await settle(300);
}
await page.setViewportSize({ width: 1180, height: 760 });
await settle(700);

/* 2f · the stale-closure defect, asserted instead of eyeballed. No shot.
   Every canvas in Onyx paints from one shared rAF loop, and each painter is a
   closure over the render that created it. `useEffect(() => subscribeFrame(paint),
   [])` therefore keeps painting with the *first* render's window size and the
   *first* track's material — a correct-looking picture at the wrong scale, and
   the highest-severity finding in the audit precisely because no screenshot can
   fail on it. `useFrameEffect` fixes it; this proves the fix is in force in a
   built preview, from the numbers `lib/diag.ts` publishes:

     · every canvas repaints at the geometry it *actually* has after a resize —
       a painter that stopped tracking geometry shows up as `painted` lagging the
       CSS box, which is the visible half of the defect;
     · every live surface goes on painting across a resize and a track change —
       a stale painter that throws (a prop it captured no longer matching the new
       material) drops out of the loop and its `paints` freezes;
     · and nothing re-attaches to the loop while doing either. That is the churn
       half: the hand-written dependency array this hook replaced tore the rAF
       loop down and rebuilt it on every prop change, ten times a second while a
       file decoded.

   The remaining half — that the painter sees the *props* of the render it belongs
   to — is not visible from outside the module, so it is driven directly against
   a stand-in React in `scripts/check-theme.mjs` ("a painter sees the render it
   belongs to, and subscribes once"). Between the two, reverting `useFrameEffect`
   to a `[]`-dependency subscribe fails `check:theme`, and re-introducing the
   dependency array fails here. */
{
  const installed = await page.evaluate(() => typeof window.__onyxDiag?.report === "function");
  if (!installed) {
    errors.push("window.__onyxDiag is missing: the mock preview was built without lib/diag.ts");
  } else {
    const diag = () => page.evaluate(() => window.__onyxDiag.report());
    const byKey = (r) => new Map(r.canvases.map((c, i) => [`${i}:${c.key}`, c]));
    /** which canvases are frame-driven, decided by watching rather than by class */
    const first = await diag();
    await settle(600);
    const second = await diag();
    const live = [...byKey(second)]
      .filter(([k, c]) => c.paints > (byKey(first).get(k)?.paints ?? 0))
      .map(([k]) => k);
    console.log(
      `diag: dpr ${second.dpr} · ${second.canvases.length} canvases · ` +
        `${live.length} live · ${second.frame.subscribers} painters attached ` +
        `(${second.frame.attachments} attaches since load)`,
    );
    if (!live.length) errors.push("no canvas repainted in 600 ms: the 60 Hz loop is not running");
    if (second.frame.subscribers < live.length) {
      errors.push(`${live.length} canvases repaint but only ${second.frame.subscribers} painters are attached`);
    }

    /** `painted` must agree with the CSS box × dpr, or the painter is behind. */
    const stale = (r) =>
      r.canvases
        .filter((c) => c.css.w >= 2 && c.css.h >= 2 && c.painted)
        .filter(
          (c) =>
            Math.abs(c.painted.w - c.css.w) > 1 ||
            Math.abs(c.painted.h - c.css.h) > 1 ||
            Math.abs(c.painted.dpr - r.dpr) > 0.01,
        )
        .map((c) => `${c.key} painted ${c.painted.w}×${c.painted.h}@${c.painted.dpr} for a ${c.css.w}×${c.css.h} box`);
    const unpainted = (r) => r.canvases.filter((c) => c.css.w >= 2 && c.css.h >= 2 && !c.painted).map((c) => c.key);

    // (a) a resize. 900×700 is a real reflow — `app.css` drops the meter column
    // at 1024 — so every lane and meter canvas changes shape, not just scale.
    const beforeResize = second;
    await page.setViewportSize({ width: 900, height: 700 });
    await settle(900);
    const resized = await diag();
    const resizeStale = stale(resized);
    const resizeDead = [...byKey(resized)].filter(
      ([k, c]) => live.includes(k) && c.paints <= (byKey(beforeResize).get(k)?.paints ?? 0),
    );
    console.log(
      `diag after resize → 900×700: geometry ${resizeStale.length ? JSON.stringify(resizeStale) : "exact"} · ` +
        `unpainted ${JSON.stringify(unpainted(resized))} · ` +
        `attaches ${beforeResize.frame.attachments}→${resized.frame.attachments} · ` +
        `painters ${beforeResize.frame.subscribers}→${resized.frame.subscribers}`,
    );
    if (resizeStale.length) errors.push(`a canvas paints the pre-resize geometry: ${JSON.stringify(resizeStale)}`);
    if (unpainted(resized).length) errors.push(`a visible canvas has never painted: ${JSON.stringify(unpainted(resized))}`);
    if (resizeDead.length) {
      errors.push(`a live surface stopped painting across the resize: ${JSON.stringify(resizeDead.map(([k]) => k))}`);
    }
    if (resized.frame.attachments > beforeResize.frame.attachments) {
      errors.push(
        `resizing re-attached ${resized.frame.attachments - beforeResize.frame.attachments} painter(s) to the ` +
          "rAF loop: a surface is subscribing on render again",
      );
    }

    // (b) a track change: new duration, new material, no remount. This is the
    // half the `[]` dependency array got wrong — the lane kept drawing the
    // timeline of the track it was born with.
    const state = await engine("app_state");
    const other = state.playlist.find((e) => e.id !== state.deckA.entryId);
    if (!other) errors.push("only one playlist entry: cannot test a track change");
    else {
      await engine("playlist_play_entry", { id: other.id });
      await settle(1200);
      const changed = await diag();
      const changedStale = stale(changed);
      const changedDead = [...byKey(changed)].filter(
        ([k, c]) => live.includes(k) && c.paints <= (byKey(resized).get(k)?.paints ?? 0),
      );
      const now = await engine("app_state");
      console.log(
        `diag after track change → entry ${other.id} (${now.deckA.durationSecs?.toFixed?.(1)}s): ` +
          `geometry ${changedStale.length ? JSON.stringify(changedStale) : "exact"} · ` +
          `attaches ${resized.frame.attachments}→${changed.frame.attachments} · ` +
          `painters ${resized.frame.subscribers}→${changed.frame.subscribers}`,
      );
      if (now.deckA.entryId !== other.id) errors.push("the track change did not take");
      if (changedStale.length) errors.push(`a canvas paints stale geometry after a track change: ${JSON.stringify(changedStale)}`);
      if (changedDead.length) {
        errors.push(`a live surface stopped painting across the track change: ${JSON.stringify(changedDead.map(([k]) => k))}`);
      }
      if (changed.frame.attachments > resized.frame.attachments) {
        errors.push(
          `a track change re-attached ${changed.frame.attachments - resized.frame.attachments} painter(s) to the rAF loop`,
        );
      }
      // back to the track and the window the rest of the set is shot at
      await engine("playlist_play_entry", { id: state.deckA.entryId });
      await settle(700);
    }
    await page.setViewportSize({ width: 1180, height: 760 });
    await settle(800);
  }
}

/* 3 · the detached EQ window (SPEC §12).

   The EQ is not a panel in this document any more: it is a second window with
   its own entry point (`eq.html`) — a Tauri `WebviewWindow` in the app, a
   browser popup in this preview. Either way it is a separate document with its
   own JavaScript, so it has to be *driven* and *photographed* as one; `page`
   cannot reach into it. Everything below therefore runs against `eq`, and the
   two facts that cross the boundary — the analyser and the band-solo badge —
   are asserted on `page` while the gesture is held in `eq`. */

/** The FFT is only allowed to run while the EQ window is open. */
const analyserRunning = async () => {
  const m = await engine("meters_get");
  return m.spectrum.some((v) => v > -109);
};
/** The real window's first-run size (`src-tauri/src/eqwindow.rs`). */
const { width: EQ_W, height: EQ_H } = EQ_VIEWPORT;

if (await analyserRunning()) errors.push("the analyser is running with the EQ window closed");

let eq = await openEq();
console.log(`EQ window: ${eq.url().split("/").pop()} · ${JSON.stringify(eq.viewportSize())}`);

/* It must focus an existing window, never make a second one. `eq_window_open`
   is the one path every caller funnels through, so call it and count. */
{
  const before = page.context().pages().length;
  await engine("eq_window_open");
  await engine("eq_window_open");
  await settle(500);
  const after = page.context().pages().length;
  console.log(`re-open while open: ${before} document(s) → ${after}`);
  if (after !== before) errors.push(`opening the EQ twice made ${after - before} extra window(s)`);
  if (!(await engine("eq_window_state")).open) errors.push("the EQ window reports itself closed");
}

const wrap = await eq.$(".eq-canvas-wrap");
const box = await wrap.boundingBox();
const at = (fx, fy) => ({ x: box.x + box.width * fx, y: box.y + box.height * fy });
const bandCount = () => eq.$$eval(".eq-row", (n) => n.length);

// a musical-looking curve: HP, low bell cut, presence lift, air shelf
for (const [fx, fy] of [
  [0.2, 0.62],
  [0.44, 0.35],
  [0.66, 0.6],
  [0.83, 0.4],
]) {
  const p = at(fx, fy);
  await eq.mouse.click(p.x, p.y);
  await eq.waitForTimeout(160);
}
// make one band emphatic by dragging it
{
  const from = at(0.44, 0.35);
  const to = at(0.4, 0.24);
  await eq.mouse.move(from.x, from.y);
  await eq.mouse.down();
  await eq.mouse.move(to.x, to.y, { steps: 14 });
  await eq.mouse.up();
}
await eq.waitForTimeout(1500);
await shotOf(eq, "08-eq-window");

/* SPEC §12: the analyser exists only as the backdrop of this curve, so it must
   be running now and stopped again once the window is gone. */
if (!(await analyserRunning())) errors.push("the analyser is not running with the EQ window open");

/* Every node must be labelled with its note name — this is a music tool. */
{
  const labels = await eq.$$eval(".eq-row .f", (n) => n.map((x) => x.textContent.trim()));
  console.log(`EQ bands: ${labels.length} · ${JSON.stringify(labels)}`);
  if (labels.length !== 4) errors.push(`expected 4 bands, got ${labels.length}`);
  // `440 Hz · A4`, with a real ♯ in the accidentals
  const unnamed = labels.filter((l) => !/·\s*[A-G][♯♭#b]?-?\d/.test(l));
  if (unnamed.length) errors.push(`band labels carry no note name: ${JSON.stringify(unnamed)}`);
}

/* 4 · band-solo sweep in progress (Cmd/Ctrl + drag) — SPEC §12, the reason the
   window exists. The indicator is asserted in *both* documents: the sweep is
   dragged here, but the badge that warns the user has to light in the main
   window, which cannot see this one's JavaScript (it rides `FramePayload`). */
{
  const from = at(0.3, 0.5);
  const to = at(0.58, 0.3);
  await eq.keyboard.down("Control");
  await eq.mouse.move(from.x, from.y);
  await eq.mouse.down();
  await eq.mouse.move(to.x, to.y, { steps: 22 });
  await eq.waitForTimeout(900);
  await shotOf(eq, "09-eq-band-solo");
  const here = await eq.evaluate(() => ({
    sweeping: document.querySelector(".eq-canvas-wrap")?.dataset.sweeping ?? null,
    readout: document.querySelector(".eq-readout")?.textContent?.trim() ?? "",
  }));
  const there = await page.$$eval('.state-badge[data-tone="alarm"]', (n) =>
    n.map((x) => `${x.dataset.on}:${x.querySelector(".sb-v")?.textContent?.trim()}`),
  );
  console.log(`band solo: EQ window ${JSON.stringify(here)} · main-window badge ${JSON.stringify(there)}`);
  if (here.sweeping !== "true" || !here.readout.startsWith("SOLO")) {
    errors.push(`no band-solo indicator in the EQ window: ${JSON.stringify(here)}`);
  }
  if (!there.some((b) => b.startsWith("true:"))) {
    errors.push(`the main window's band-solo badge did not light: ${JSON.stringify(there)}`);
  }
  await shot("11-main-with-eq-open");
  await eq.mouse.up();
  await eq.keyboard.up("Control");
  await settle(600);
  const after = await page.$$eval('.state-badge[data-tone="alarm"]', (n) => n.map((x) => x.dataset.on));
  if (after.some((v) => v === "true")) errors.push("the band-solo badge survived the release");
}

/* Closing must not disturb playback, must stop the FFT, and must leave the
   curve behind: the bands live in the engine, not in that document. */
{
  const bands = await bandCount();
  const posBefore = (await engine("app_state")).transport.positionSecs;
  const playingBefore = (await engine("app_state")).transport.playing;
  await eq.close();
  await settle(900);
  const st = await engine("app_state");
  const open = (await engine("eq_window_state")).open;
  console.log(
    `EQ closed: open=${open} · playing ${playingBefore}→${st.transport.playing} · ` +
      `position ${posBefore.toFixed(2)}→${st.transport.positionSecs.toFixed(2)} · ` +
      `analyser ${(await analyserRunning()) ? "still running" : "stopped"} · ` +
      `${st.eq.bands.length} bands kept`,
  );
  if (open) errors.push("the EQ window reports itself open after closing");
  if (st.transport.playing !== playingBefore) errors.push("closing the EQ window changed playback");
  if (st.transport.positionSecs < posBefore) errors.push("closing the EQ window moved the playhead");
  if (await analyserRunning()) errors.push("the analyser kept running after the EQ window closed");
  if (st.eq.bands.length !== bands) errors.push(`bands lost on close: ${bands} → ${st.eq.bands.length}`);

  // …and they are still there when it comes back.
  eq = await openEq();
  const again = await bandCount();
  console.log(`EQ reopened: ${again} bands`);
  if (again !== bands) errors.push(`bands did not persist across a reopen: ${bands} → ${again}`);
}
await settle(400);

/* The EQ window is resizable down to 620×360 (`src-tauri/src/eqwindow.rs`), so
   every width in that range has to hold. This caught a real one: `.eq-left`
   took the implicit `auto` grid column, which sizes to the widest child — the
   header's one-line gesture legend — so the graph was laid out 854px wide in a
   622px column and drawn underneath the band list. Overlap is the assertion
   that matters; a canvas can overflow its column without overflowing the
   document, so `scrollWidth` alone would never have seen it. */
{
  const measure = () =>
    eq.evaluate(() => {
      const q = (s) => document.querySelector(s);
      const wrap = q(".eq-canvas-wrap");
      const g = wrap.getBoundingClientRect();
      const r = q(".eq-right").getBoundingClientRect();
      const head = q(".eq-head");
      const ro = q(".eq-readout");
      const de = document.documentElement;
      const f = [...document.querySelectorAll(".eq-row .f")];
      const rws = [...document.querySelectorAll(".eq-row")];
      return {
        w: innerWidth,
        graph: Math.round(g.width),
        spill: Math.round(g.right - r.left),
        head: head.scrollWidth - head.clientWidth,
        doc: de.scrollWidth - de.clientWidth,
        clipped: ro.scrollWidth - ro.clientWidth,
        freq: Math.max(...f.map((e) => e.scrollWidth - e.clientWidth)),
        rowSpill: Math.max(...rws.map((e) => e.scrollHeight - e.clientHeight)),
        dpr: q(".eq-canvas-wrap canvas").width === Math.round(g.width * devicePixelRatio),
      };
    });
  const rows = [];
  for (const w of [1280, 1201, 1000, 961, EQ_W, 821, 701, 620]) {
    await eq.setViewportSize({ width: w, height: w === 620 ? 400 : EQ_H });
    await settle(500);
    const m = await measure();
    rows.push(`${m.w}px graph ${m.graph}px spill ${m.spill}px head ${m.head}px doc ${m.doc}px`);
    if (m.spill > 0) errors.push(`EQ ${m.w}px: the graph runs ${m.spill}px under the band list`);
    if (m.head > 0) errors.push(`EQ ${m.w}px: the header overflows by ${m.head}px`);
    if (m.doc > 0) errors.push(`EQ ${m.w}px: the window scrolls sideways by ${m.doc}px`);
    if (!m.dpr) errors.push(`EQ ${m.w}px: the canvas backing store did not follow the resize`);
    // `1.91 kHz · A♯6` is what a band row is for; a row that has ellipsed it
    // away is a row with nothing left to say.
    if (m.freq > 0) errors.push(`EQ ${m.w}px: band rows ellipse the frequency by ${m.freq}px`);
    if (m.rowSpill > 0) errors.push(`EQ ${m.w}px: band rows overflow their height by ${m.rowSpill}px`);
    // The read-out may ellipse when the window is genuinely tight, never while
    // the header still has a spacer's worth of slack to give it.
    if (w >= EQ_W && m.clipped > 0) errors.push(`EQ ${m.w}px: the read-out is clipped with room to spare`);
    if (w === 620) await shotOf(eq, "10-eq-window-min");
  }
  console.log(`EQ widths: ${rows.join(" · ")}`);
  await eq.setViewportSize({ width: EQ_W, height: EQ_H });
  await settle(500);
}
await settle(300);

/* 5 · A/B alignment with a non-zero offset. The nudges alone move deck B by a
   few ms, which on a 4-minute timeline is well under a pixel — so Alt-drag lane
   B far enough that the shift is actually visible against lane A, then trim it
   with the nudges so the read-out is not a round number. */
{
  const laneB = await page.$$(".wave-lane");
  const rect = await laneB[laneB.length - 1].boundingBox();
  const y = rect.y + rect.height / 2;
  const x0 = rect.x + rect.width * 0.5;
  await page.keyboard.down("Alt");
  await page.mouse.move(x0, y);
  await page.mouse.down();
  await page.mouse.move(x0 + 16, y, { steps: 20 });
  await page.mouse.up();
  await page.keyboard.up("Alt");
}
await page.click('.nudges button[title="Deck B 10 ms later"]');
await page.click('.nudges button[title="Deck B 1 ms later"]');
await settle(900);
await shot("06-ab-align-offset");


/* 5b · the badge rail at its realistic worst case: monitor fold + a non-zero
   align offset + level match + a live band-solo audition, all lit at once. The
   sweep is held in the *EQ window* and the shot is of the main one — which is
   exactly the situation the audition badge exists for (SPEC §12): the filter
   you can hear is being dragged somewhere you may not be looking. */
await page.click(".tr-toggle.match");
await page.keyboard.press("o");
await settle(700);
{
  const b2 = await (await eq.$(".eq-canvas-wrap")).boundingBox();
  await eq.keyboard.down("Control");
  await eq.mouse.move(b2.x + b2.width * 0.36, b2.y + b2.height * 0.46);
  await eq.mouse.down();
  await eq.mouse.move(b2.x + b2.width * 0.52, b2.y + b2.height * 0.34, { steps: 18 });
  await settle(900);
  await shot("07-badges-dense");
  const lit = await page.$$eval(".badge-rail .state-badge", (n) =>
    n.filter((x) => x.dataset.on === "true").map((x) => x.querySelector(".sb-k")?.textContent),
  );
  console.log(`badge rail: ${JSON.stringify(lit)}`);
  if (lit.length !== 3) errors.push(`expected 3 lit badges, got ${JSON.stringify(lit)}`);
  await eq.mouse.up();
  await eq.keyboard.up("Control");
}
await page.keyboard.press("o");
await page.click(".tr-toggle.match");
await settle(300);
// Done with the EQ; everything below is main-window only.
await eq.close();
await settle(500);

/* 5a · focus hygiene: a mouse-clicked control must not stay lit. RESET was the
   one the user found — Chromium keeps it focused after a click and then paints
   the focus ring as soon as any key is pressed. Assert the state rather than
   trusting the picture, on the button that showed it and on one sibling. */
for (const sel of [".align-group > button:nth-of-type(2)", ".nudges button:first-of-type"]) {
  const el = await page.$(sel);
  await el.click();
  await page.keyboard.press("Space");
  await settle(200);
  const state = await page.$eval(sel, (n) => ({
    label: n.textContent.trim(),
    focused: document.activeElement === n,
    ring: n.matches(":focus-visible"),
    outline: getComputedStyle(n).outlineStyle,
  }));
  console.log(`click focus ${sel}: ${JSON.stringify(state)}`);
  if (state.focused || state.ring || state.outline !== "none") {
    errors.push(`stuck focus after mouse click: ${sel} ${JSON.stringify(state)}`);
  }
}
// Keyboard users keep their ring. Tab cannot be used to walk there — SPEC §
// keyboard binds it to the deck toggle — so focus the control and press a key
// to put the browser in keyboard modality, which is the state a keyboard-driven
// user is in when the ring has to show.
{
  await page.$eval(".align-group > button:nth-of-type(2)", (n) => n.focus());
  await page.keyboard.press("Shift");
  await settle(120);
  const kb = await page.$eval(".align-group > button:nth-of-type(2)", (n) => {
    const s = getComputedStyle(n);
    return {
      ring: n.matches(":focus-visible"),
      outline: `${s.outlineStyle} ${s.outlineWidth} ${s.outlineColor}`,
    };
  });
  console.log(`keyboard focus ring: ${JSON.stringify(kb)}`);
  if (!kb.ring || kb.outline.startsWith("none")) {
    errors.push(`keyboard focus ring missing on RESET: ${JSON.stringify(kb)}`);
  }
  await page.evaluate(() => document.activeElement?.blur());
}

/* 6 · settings. The panel now carries appearance (§15), the engine source
   (§16) and the MIDI bank (§18) above the loudness cache, and it scrolls, so
   it is photographed a block at a time — appearance, the source, the foot —
   and the selector is the aria label, not the tooltip, which is now a
   sentence. */
await page.click('button[aria-label="Settings"]');
await page.waitForSelector(".float-panel.settings", { timeout: 5000 });
/* Appearance has two faces since SPEC §20 and the code editor is the default
   one; these shots are of the *pickers*, which now live under "Simple".
   `scripts/shots-themedoc.mjs` photographs the other face. */
await page.click('.float-panel.settings .set-tabs button:has-text("Simple")');
await settle(900);
await shot("21-settings-appearance");
{
  // Every appearance control has to be there, and the accent field has to
  // reject a bad hex visibly rather than swallow it (§15).
  const controls = await page.$$eval(".float-panel.settings", ([p]) => ({
    sections: [...p.querySelectorAll(".set-sec-title")].map((n) => n.textContent),
    swatches: p.querySelectorAll(".set-swatch:not(.big)").length,
    selects: p.querySelectorAll(".select").length,
    hex: !!p.querySelector(".set-hex"),
  }));
  console.log(`settings: ${JSON.stringify(controls)}`);
  if (controls.swatches < 4 || !controls.hex) errors.push("appearance controls missing");

  await page.fill(".set-hex", "not-a-colour");
  await page.keyboard.press("Enter");
  await settle(300);
  const rejected = await page.$$eval(".float-panel.settings", ([p]) => ({
    bad: p.querySelector(".set-hex")?.dataset.bad === "true",
    said: p.querySelector(".set-error")?.textContent?.trim() ?? "",
    accent: getComputedStyle(document.documentElement).getPropertyValue("--accent").trim(),
  }));
  console.log(`bad hex: ${JSON.stringify(rejected)}`);
  if (!rejected.bad || !rejected.said) errors.push("a bad hex was accepted silently");
  await shot("22-settings-accent-rejected");
  // Back to the default by way of its preset, not by typing `--accent` back in:
  // that variable holds the *derived* tone, not the accent that was chosen.
  await page.click(".set-swatches .set-swatch:first-child");
  await settle(400);
}
/* The engine source (§16), scrolled to the top of its own block: the panel is
   taller than the window, and a shot taken at the very bottom is the *cache*
   shot with the source rail half in frame — two numbers for one picture. */
await page.$eval(".float-panel.settings", (p) => {
  const sec = [...p.querySelectorAll(".set-sec")].find(
    (s) => s.querySelector(".set-sec-title")?.textContent === "Engine source",
  );
  p.scrollTo(0, (sec?.offsetTop ?? 0) - 8);
});
await settle(500);
await shot("23-settings-engine-source");
{
  // The path this sandbox can actually walk (§16): an API with nothing on it.
  // Choosing it must say so and leave the running stream alone, not tear the
  // output down and hand back silence.
  const before = await page.evaluate(() => window.__onyxMockHost.invoke("audio_source", {}));
  await page.selectOption('[data-set="host"]', "jack");
  await settle(700);
  const empty = await page.$$eval(".float-panel.settings", ([p]) => {
    const row = [...p.querySelectorAll(".field")].find((f) =>
      f.textContent.startsWith("Output device"),
    );
    return {
      said: row?.querySelector(".sub")?.textContent?.trim() ?? "",
      disabled: row?.querySelector("select")?.disabled ?? false,
    };
  });
  const after = await page.evaluate(() => window.__onyxMockHost.invoke("audio_source", {}));
  console.log(`empty host: ${JSON.stringify(empty)} · device kept ${after.source?.deviceName}`);
  if (!/no output devices/i.test(empty.said)) errors.push("an empty host said nothing");
  if (after.source?.deviceName !== before.source?.deviceName) {
    errors.push("an empty host tore down the running stream");
  }
  await shot("24-settings-no-devices");
  await page.selectOption('[data-set="host"]', "coreaudio");
  await settle(600);
}
// and the foot of the panel: the MIDI bank and the loudness cache (§8/§18)
await page.$eval(".float-panel.settings", (p) => p.scrollTo(0, p.scrollHeight));
await settle(500);
await shot("25-settings-cache");
{
  // No audio device in this sandbox is the interesting path (§16): the panel
  // must say so rather than render three empty pickers.
  const source = await page.evaluate(() => window.__onyxMockHost.invoke("audio_source", {}));
  console.log(
    `audio source: ${source.hosts.length} hosts, ${source.devices.length} devices, ` +
      `${source.source?.bufferFrames} frames, ${source.source?.latencyMs?.toFixed(1)} ms`,
  );
}
/* 6b · §15 again, and the constraint that actually bites: `large` must not
   break the 420 px floor. The zoom is capped against the layout minimum, so
   at 420 px it has to refuse to grow — and the settings panel, now the tallest
   thing in the app, has to stay inside the window rather than run off it. */
await page.setViewportSize({ width: 420, height: 560 });
await settle(800);
await page.click('.float-panel.settings .seg button:has-text("Large")');
await settle(800);
{
  const fit = await page.evaluate(() => {
    const doc = document.documentElement;
    const p = document.querySelector(".float-panel.settings");
    const r = p.getBoundingClientRect();
    return {
      zoom: getComputedStyle(doc).getPropertyValue("--zoom").trim(),
      overflowX: doc.scrollWidth - doc.clientWidth,
      panel: [Math.round(r.width), Math.round(r.height)],
      spill: [Math.round(r.right - window.innerWidth), Math.round(r.bottom - window.innerHeight)],
      scrolls: p.scrollHeight > p.clientHeight,
      win: [window.innerWidth, window.innerHeight],
    };
  });
  console.log(`large at 420: ${JSON.stringify(fit)}`);
  if (fit.overflowX > 1) errors.push(`large at 420 overflows by ${fit.overflowX}px`);
  if (fit.spill[0] > 1 || fit.spill[1] > 1) errors.push(`the settings panel spills ${fit.spill}`);
  if (!fit.scrolls) errors.push("the settings panel does not scroll where it cannot fit");
}
await shot("26-settings-large-420");
// back to the defaults and to the window every other shot is taken in
await page.click('.float-panel.settings .ghost-btn:has-text("Reset")');
await settle(500);
await page.setViewportSize({ width: 1180, height: 760 });
await settle(700);

// the hex field still has focus, and Escape belongs to whatever is focused
await page.evaluate(() => document.activeElement?.blur());
await page.keyboard.press("Escape");
await settle(400);

/* 7 · an ABX trial */
await page.click('.tr-toggle:has-text("Blind")');
await settle(600);
await page.click('.proto:has-text("ABX")');
await settle(300);
await page.click('.xfade-opts button:has-text("12")').catch(() => {});
await settle(200);
await page.click('.solid-btn:has-text("Begin")');
await settle(1200);
await shot("12-abx-trial");

/* 8 · the reveal — answer every trial, then look at the summary */
for (let i = 0; i < 24; i += 1) {
  const done = await page.$(".blind-verdict");
  if (done) break;
  const btn = await page.$(".blind-actions .solid-btn:not([disabled])");
  if (!btn) break;
  await btn.click();
  await page.waitForTimeout(260);
}
await page.waitForSelector(".blind-verdict", { timeout: 8000 });
await settle(900);
await shot("13-abx-reveal");

/* 10 · the shortcut overlay. Worth its own shot since the bindings moved from
   `event.key` to `event.code` with legends resolved from the Keyboard Map API:
   a positional row that renders blank, or as `undefined`, is a regression this
   is the only place to see. */
await page.keyboard.press("Escape");
await settle(400);
await page.keyboard.press("Shift+Slash");
await page.waitForSelector(".shortcut-card", { timeout: 5000 });
await settle(600);
await shot("14-shortcuts");
{
  // Legends are text, so read them back rather than only looking at a picture.
  const rows = await page.$$eval(".shortcut-row", (nodes) =>
    nodes.map((n) => [
      n.querySelector(".keys")?.textContent ?? "",
      n.querySelector(".act")?.textContent ?? "",
    ]),
  );
  const broken = rows.filter(([keys]) => !keys.trim() || /undefined|null|\bDigit|\bKey[A-Z]\b/.test(keys));
  console.log(`shortcut overlay: ${rows.length} rows, ${broken.length} unreadable`);
  if (broken.length) errors.push(`shortcut legends: ${JSON.stringify(broken)}`);
}
await page.keyboard.press("Escape");

/* 14 · SPEC §18/§19: a zip opened as a playlist, the archive named on the
   playlist head, and the `.mid` inside it reading as a General MIDI rendering
   rather than as a recording. Last, because it replaces the playlist the
   earlier steps were built on. */
await settle(500);
await engine("open_files", {
  paths: ["/Users/mix/Deliveries/Nightglass masters.zip"],
  replace: true,
});
await settle(1200);
{
  const head = await page.$eval(".pl-head .pl-tag", (n) => n.textContent.trim()).catch(() => "");
  const rows = await page.$$eval(".pl-row", (nodes) =>
    nodes.map((n) => ({
      title: n.querySelector(".pl-title")?.firstChild?.textContent?.trim() ?? "",
      tags: [...n.querySelectorAll(".pl-tag")].map((t) => t.textContent.trim()),
    })),
  );
  console.log(`archive playlist: head "${head}" · ${JSON.stringify(rows)}`);
  if (!/\.zip$/i.test(head)) errors.push(`the playlist head does not name the archive: "${head}"`);
  if (rows.length === 0) errors.push("the archive produced no rows");
  if (!rows.some((r) => r.tags.some((t) => t.startsWith("MIDI")))) {
    errors.push("no MIDI row in the archive playlist");
  }
}
// the archive as a playlist: the zip on the head, its members as rows
await shot("27-archive-playlist");

/* 14a · and the MIDI member playing, which is its own subject: the title bar
   has to read `MIDI · GM · <bank> · <rate> · stereo` — the bank being the thing
   the user can change — rather than pass a rendering off as a recording. */
{
  const id = await page.evaluate(
    () =>
      window.__onyxMockHost
        .invoke("app_state", {})
        .then((s) => s.playlist.find((e) => e.synthBank)?.id ?? null),
  );
  if (id == null) errors.push("no MIDI entry to play in the archive playlist");
  else {
    await engine("playlist_play_entry", { id });
    await settle(1400);
    const badge = await page.$eval(".tb-badge", (n) => n.textContent.trim()).catch(() => "");
    console.log(`title bar badge: ${badge}`);
    if (!/^MIDI · GM · .+/.test(badge)) errors.push(`MIDI badge reads "${badge}"`);
    await shot("28-midi-track");
  }
}

/* 14b · the same playlist with a file off the disk added to it. Now the origin
   distinguishes rows, so every archive row has to name its archive — one zip
   on its own says so once, in the head, and does not repeat itself down the
   column. */
await engine("open_files", { paths: ["/Users/mix/Masters/06 Ashfall.mp3"], replace: false });
await settle(1000);
{
  const rows = await page.$$eval(".pl-row", (nodes) =>
    nodes.map((n) => [...n.querySelectorAll(".pl-tag")].map((t) => t.textContent.trim())),
  );
  const marked = rows.filter((tags) => tags.some((t) => /\.zip$/i.test(t))).length;
  console.log(`mixed playlist: ${rows.length} rows, ${marked} marked with an archive`);
  if (marked !== rows.length - 1) errors.push(`archive marks on ${marked} of ${rows.length - 1}`);
}
await shot("29-archive-mixed");

/* 15 · and the archive with nothing playable in it, which has to say so in
   words rather than appear to do nothing. The toast is the subject of this
   shot, so it is not dismissed first. */
await page
  .evaluate(() =>
    window.__onyxMockHost
      .invoke("open_files", { paths: ["/Users/mix/Deliveries/Artwork and notes.zip"], replace: false })
      .catch(() => null),
  )
  .catch(() => null);
await settle(700);
{
  const toasts = await page.$$eval(".toast", (nodes) => nodes.map((n) => n.textContent.trim()));
  console.log(`audio-free archive: ${JSON.stringify(toasts)}`);
  if (!toasts.some((t) => /contains no audio files/i.test(t))) {
    errors.push("an audio-free archive said nothing");
  }
  await shotOf(page, "30-archive-no-audio", { keepToasts: true });
}

/* 16 · SPEC §2.8 — the four routes onto deck B, each one asserted against the
   engine rather than admired as a picture. The bug this section exists for was
   reported as "deck b is not assignable, only a": deck B *was* assignable, but
   only through a right-click menu nothing hinted at, so for every user who had
   not been told, it was not. Each route below is checked for the two things
   that make it real — the material lands on deck B, and A/B turns itself on. */
{
  const abState = async () => {
    const s = await engine("app_state");
    return {
      ab: s.ab.enabled,
      b: s.deckB.loaded ? s.deckB.entryId : null,
      a: s.deckA.loaded ? s.deckA.entryId : null,
      ids: s.playlist.map((e) => e.id),
    };
  };
  /** Back to three tracks, deck A playing, deck B empty, A/B off. */
  const reset = async () => {
    await engine("open_files", {
      paths: [
        "/Users/mix/Masters/01 Nightglass.wav",
        "/Users/mix/Masters/02 Ember Room.flac",
        "/Users/mix/Masters/03 Slow Amber.aiff",
      ],
      replace: true,
    });
    await engine("ab_set_enabled", { value: false });
    // earlier steps left an alignment offset on B; it is not this section's subject
    await engine("set_ab_offset", { frames: 0 });
    await settle(700);
    return abState();
  };
  const laneOf = async (deck) => {
    const lane = await page.$(`.wave-lane[data-deck="${deck}"]`);
    if (!lane) throw new Error(`there is no lane ${deck.toUpperCase()} on screen`);
    return lane;
  };

  let base = await reset();
  console.log(`assignment start: ${JSON.stringify(base)}`);
  if (base.b !== null) errors.push("deck B is not empty at the start of the assignment checks");

  /* 16a · the empty lane. Turn A/B on with nothing on B: the lane must say how
     to fill itself, not draw a blank rectangle the user reads as a dead deck. */
  await engine("ab_set_enabled", { value: true });
  await settle(700);
  {
    const hint = await page
      .$eval('.wave-lane[data-deck="b"] .lane-empty', (n) => n.textContent.replace(/\s+/g, " ").trim())
      .catch(() => "");
    console.log(`empty deck B lane: "${hint}"`);
    if (!/Deck B is empty/i.test(hint)) errors.push(`the empty B lane says "${hint}"`);
    for (const route of [/Drag a track/i, /playlist row/i, /\u21E7B/]) {
      if (!route.test(hint)) errors.push(`the empty B lane does not mention ${route}`);
    }
    await shot("37-ab-lane-b-empty");
  }
  await engine("ab_set_enabled", { value: false });
  await settle(500);

  /* 16b · drag a playlist row onto lane B. Photographed mid-air, because the
     highlight is the whole point: a lane that only reacts after the drop is a
     gesture you have to already know about. Playwright drives the real mouse,
     so this is Chromium's own HTML5 drag, not a synthesised event. */
  {
    await engine("ab_set_enabled", { value: true }); // lane B has to exist to aim at
    await settle(600);
    const row = (await page.$$(".pl-row"))[1];
    const rowId = base.ids[1];
    const rb = await row.boundingBox();
    const laneB = await laneOf("b");
    const lb = await laneB.boundingBox();
    await page.mouse.move(rb.x + 140, rb.y + rb.height / 2);
    await page.mouse.down();
    await page.mouse.move(rb.x + 150, rb.y + rb.height / 2 - 12, { steps: 5 });
    await page.mouse.move(lb.x + lb.width / 2, lb.y + lb.height / 2, { steps: 22 });
    await settle(350);
    const mid = await page.$$eval(".wave-lane", (nodes) =>
      nodes.map((n) => ({
        deck: n.dataset.deck,
        target: n.dataset.dropTarget === "true",
        hint: getComputedStyle(n.querySelector(".lane-drop-hint")).opacity,
      })),
    );
    console.log(`mid-drag over lane B: ${JSON.stringify(mid)}`);
    const lit = mid.filter((l) => l.target);
    if (lit.length !== 1 || lit[0].deck !== "b") {
      errors.push(`the drag did not light lane B alone: ${JSON.stringify(mid)}`);
    }
    if (Number(lit[0]?.hint ?? 0) < 0.9) errors.push("lane B lit up but said nothing about what a drop does");
    await shot("38-assign-drag-hover-b");

    await page.mouse.up();
    await settle(900);
    const after = await abState();
    console.log(`after dropping row 2 on lane B: ${JSON.stringify(after)}`);
    if (after.b !== rowId) errors.push(`drag to lane B put ${after.b} on the deck, expected ${rowId}`);
    if (!after.ab) errors.push("assigning deck B by drag left A/B off");
    const stuck = await page.$$eval(".wave-lane", (n) => n.some((x) => x.dataset.dropTarget === "true"));
    if (stuck) errors.push("a lane stayed lit as a drop target after the drop");
    await shot("39-assign-drag-dropped-b");
  }

  /* 16c · the row chips. Two claims: the chip for the deck a row sits on is
     always visible (the assignment is legible without hovering anything), and
     one click on the other chip moves the row to that deck. */
  base = await reset();
  {
    const chips = await page.$$eval(".pl-row", (nodes) =>
      nodes.slice(0, 3).map((n, i) => ({
        row: i + 1,
        deck: n.dataset.deck ?? null,
        selected: n.dataset.selected === "true",
        chips: [...n.querySelectorAll(".deck-chip")].map((c) => ({
          d: c.dataset.deck,
          on: c.dataset.on === "true",
          o: Number(getComputedStyle(c).opacity).toFixed(2),
          w: Math.round(c.getBoundingClientRect().width),
        })),
      })),
    );
    console.log(`row chips: ${JSON.stringify(chips)}`);
    for (const r of chips) {
      if (r.chips.length !== 2) errors.push(`row ${r.row} has ${r.chips.length} deck chips`);
      // hit-testable at all times, so touch and keyboard can reach them
      if (r.chips.some((c) => c.w < 12)) errors.push(`row ${r.row}: a deck chip has no box`);
      const onChip = r.chips.find((c) => c.on);
      if (r.deck && Number(onChip?.o) < 0.99) {
        errors.push(`row ${r.row} is on deck ${r.deck} but its chip is invisible (${onChip?.o})`);
      }
      // the chip for the row that is *selected* must be reachable without a mouse
      if (r.selected && r.chips.some((c) => Number(c.o) < 0.99)) {
        errors.push(`row ${r.row} is selected but its chips are hover-only`);
      }
    }

    const rowId = base.ids[2];
    await page.hover(".pl-row:nth-child(3)");
    await settle(250);
    await page.click('.pl-row:nth-child(3) .deck-chip[data-deck="b"]');
    await settle(900);
    const after = await abState();
    console.log(`after tapping the B chip on row 3: ${JSON.stringify(after)}`);
    if (after.b !== rowId) errors.push(`the B chip put ${after.b} on deck B, expected ${rowId}`);
    if (!after.ab) errors.push("the B chip assigned deck B but left A/B off");
    if (after.a !== base.a) errors.push("assigning deck B disturbed deck A");
    await shot("40-assign-chip-b");
  }

  /* 16d · ⇧B on the selected row — and, in the same breath, that plain `B` is
     a *different* thing: it moves which deck is audible and assigns nothing.
     Confusing the two is what makes assignment look broken. */
  base = await reset();
  {
    await page.click(".pl-row:nth-child(2)"); // one tap = play = select (SPEC §2.3)
    await settle(700);
    const selected = base.ids[1];
    const before = await abState();
    await page.keyboard.press("Shift+B");
    await settle(900);
    const after = await abState();
    console.log(`⇧B on row 2: ${JSON.stringify(before)} → ${JSON.stringify(after)}`);
    if (after.b !== selected) errors.push(`⇧B put ${after.b} on deck B, expected ${selected}`);
    if (!after.ab) errors.push("⇧B assigned deck B but left A/B off");
    await shot("41-assign-shift-b");

    // plain A / B: audible only, and it has to *look* like it
    await page.keyboard.press("KeyB");
    await settle(600);
    const audible = await page.$$eval(".wave-lane", (nodes) =>
      nodes.map((n) => ({
        deck: n.dataset.deck,
        active: n.dataset.active === "true",
        says: n.querySelector(".lane-audible, .lane-silent")?.textContent?.trim() ?? "",
      })),
    );
    const listening = await abState();
    console.log(`plain B: ${JSON.stringify(audible)} · decks ${JSON.stringify(listening)}`);
    if (listening.b !== after.b || listening.a !== after.a) errors.push("plain B moved material between decks");
    const laneB = audible.find((l) => l.deck === "b");
    if (!laneB?.active || laneB.says !== "audible") errors.push(`lane B does not read as audible: ${JSON.stringify(laneB)}`);
    if (audible.find((l) => l.deck === "a")?.says !== "silent") {
      errors.push("lane A does not say it has gone silent");
    }
    await shot("42-ab-audible-b");
    await page.keyboard.press("KeyA");
    await settle(400);
  }

  /* 16e · a file dropped straight onto a lane belongs to that lane, not to the
     window-level "drop = append" of SPEC §2.2. Synthesised here: Playwright
     cannot originate a drag from the desktop, and in the packaged app this
     path is Tauri's native `onDragDropEvent` (hit-tested in `App.tsx`), which
     a browser cannot exercise at all. What this does prove is that the lane
     claims the drop and the window handler does not fire. */
  base = await reset();
  {
    await engine("ab_set_enabled", { value: true }); // the empty B lane is the target
    await settle(600);
    const laneB = await laneOf("b");
    // The drag is held first so the highlight can be read after React has
    // rendered it, then let go: one `evaluate` for both would sample the
    // attribute in the same tick that set it.
    await laneB.evaluate((lane) => {
      const dt = new DataTransfer();
      dt.items.add(new File([new Uint8Array(64)], "Late night bounce.wav", { type: "audio/wav" }));
      window.__shotDt = dt;
      // real coordinates: the window-level handler hit-tests them to decide
      // whether to raise its own "drop to append" overlay
      const r = lane.getBoundingClientRect();
      const at = { clientX: r.x + r.width / 2, clientY: r.y + r.height / 2 };
      window.__shotAt = at;
      for (const type of ["dragenter", "dragover"]) {
        lane.dispatchEvent(
          new DragEvent(type, { bubbles: true, cancelable: true, dataTransfer: dt, ...at }),
        );
      }
    });
    await settle(300);
    const lit = await laneB.evaluate((lane) => lane.dataset.dropTarget === "true");
    // and the window-wide "drop to append" overlay must stand down: over a
    // lane the gesture means something else, and two affordances at once is a
    // question, not an answer
    const overlay = await page.$(".drop-overlay");
    if (overlay) errors.push("the window append overlay covered the lane's own drop target");
    await shot("43-assign-file-drag-over-lane-b");
    const dropped = await laneB.evaluate((lane) => {
      const dt = window.__shotDt;
      const drop = new DragEvent("drop", {
        bubbles: true,
        cancelable: true,
        dataTransfer: dt,
        ...window.__shotAt,
      });
      // The window handler is the SPEC §2.2 append. It listens in the bubble
      // phase, which is what `stopPropagation` on the lane is supposed to cut off.
      let reachedWindow = false;
      const spy = () => (reachedWindow = true);
      window.addEventListener("drop", spy);
      lane.dispatchEvent(drop);
      window.removeEventListener("drop", spy);
      delete window.__shotDt;
      delete window.__shotAt;
      return { defaultPrevented: drop.defaultPrevented, reachedWindow };
    });
    await settle(1400);
    const after = await abState();
    console.log(`file dropped on lane B: lit ${lit} · ${JSON.stringify(dropped)} → ${JSON.stringify(after)}`);
    if (!lit) errors.push("a file dragged over lane B did not light it");
    if (!dropped.defaultPrevented) errors.push("lane B did not claim the file drop");
    if (dropped.reachedWindow) errors.push("the window-level append handler also saw the lane's drop");
    if (after.ids.length !== base.ids.length + 1) errors.push("the dropped file was not added to the playlist");
    const added = after.ids.find((id) => !base.ids.includes(id));
    if (after.b !== added) errors.push(`the dropped file went to ${after.b}, expected the new row ${added}`);
    if (!after.ab) errors.push("a file dropped on lane B left A/B off");
    await shot("44-assign-file-drop-lane-b");
  }
}

/* One verdict, shared with the other two harnesses: print what was written,
   print every problem (this used to print the first 20 and then exit 0, so a
   failing run was indistinguishable from a clean one), and exit non-zero if
   there were any. */
await finish();
