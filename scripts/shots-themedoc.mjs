/**
 * The theme-document workflow, driven end to end — SPEC §20. Dev tooling.
 *
 *   npm run build:mock
 *   npx vite preview --outDir dist-mock --port 4173
 *   node scripts/shots-themedoc.mjs [url]
 *
 * Shots 46–55 of the one numbered set (`scripts/shots.mjs` owns 01–30 and
 * 37–44, `shots-theme.mjs` owns 31–36 and 45).
 *
 * This is the *user's* workflow, not a unit test with a camera: open the
 * editor, paste a theme a language model would write, apply it, watch the main
 * window and the EQ window change, make a typo and read the error, paste
 * something illegible and read the warning, then wreck the app on purpose and
 * recover it with the keyboard.
 *
 * Every claim is asserted against **state** — the custom properties `<html>` is
 * actually wearing, the document the backend actually stored, the DOM of the
 * problem list — and the screenshots are evidence for a human, not the test.
 * A picture cannot tell you that `--ink-900` is the one the document asked for.
 *
 * What this cannot prove is listed at the bottom of THEMING.md: the preview's
 * "windows" are browser popups, so window decorations, the native menu item and
 * OS font resolution are out of reach here.
 *
 * The browser, the window size, the toast-clearing shutter, the console watch,
 * popup adoption and the exit verdict are shared with the other two harnesses —
 * see `scripts/lib/shots-base.mjs`.
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { startHarness } from "./lib/shots-base.mjs";

const {
  root,
  url,
  page,
  context,
  errors,
  watch,
  settle,
  engine,
  shotOf,
  openPopup,
  openEq,
  finish,
} = await startHarness();

const LLM = readFileSync(join(root, "src/lib/fixtures/llm-theme.jsonc"), "utf8");

const note = (m) => console.log(`  · ${m}`);
const fail = (m) => {
  errors.push(m);
  console.log(`  ✗ ${m}`);
};

/** The token layer of whichever document is asking. */
const tokens = (target) =>
  target.evaluate(() => {
    const root = document.documentElement;
    const cs = getComputedStyle(root);
    const v = (t) => cs.getPropertyValue(t).trim();
    return {
      theme: root.dataset.theme,
      uiFont: root.dataset.uiFont,
      numFont: root.dataset.numFont,
      bg: v("--ink-900"),
      panel: v("--surface-panel"),
      accent: v("--accent"),
      text: v("--text-hi"),
      rPanel: v("--r-panel"),
      barStep: v("--wf-bar-step"),
    };
  });

/**
 * Contrast of the body text over the app's own background, as the eye gets it.
 * "Still legible" has to be a number, or it is an opinion about a screenshot.
 */
const legibility = (target) =>
  target.evaluate(() => {
    const cs = getComputedStyle(document.documentElement);
    const parse = (s) => {
      const n = (s.match(/[\d.]+/g) ?? []).map(Number);
      return { r: n[0] ?? 0, g: n[1] ?? 0, b: n[2] ?? 0, a: n[3] ?? 1 };
    };
    const hex = (s) => {
      const h = s.trim().replace("#", "");
      const w = h.length === 3 ? [...h].map((c) => c + c) : [h.slice(0, 2), h.slice(2, 4), h.slice(4, 6)];
      return { r: parseInt(w[0], 16), g: parseInt(w[1], 16), b: parseInt(w[2], 16), a: 1 };
    };
    const colour = (s) => (s.trim().startsWith("#") ? hex(s) : parse(s));
    const lin = (c) => {
      const x = c / 255;
      return x <= 0.03928 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4;
    };
    const lum = (c) => 0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b);
    const over = (f, b) => ({
      r: f.r * f.a + b.r * (1 - f.a),
      g: f.g * f.a + b.g * (1 - f.a),
      b: f.b * f.a + b.b * (1 - f.a),
      a: 1,
    });
    const bg = colour(cs.getPropertyValue("--ink-900"));
    const ink = over(colour(cs.getPropertyValue("--text-hi")), bg);
    const [hi, lo] = [lum(ink), lum(bg)].sort((a, b) => b - a);
    return +(((hi + 0.05) / (lo + 0.05)).toFixed(2));
  });

/** Type into the code box the way a paste does: whole value, one change. */
const paste = async (win, text) => {
  await win.fill(".te-code", text);
  // the editor validates on a 220 ms debounce
  await win.waitForTimeout(700);
};

const problems = (win) =>
  win.$$eval(".te-problems li", (nodes) =>
    nodes.map((n) => ({
      level: n.dataset.level,
      line: n.querySelector(".te-line")?.textContent,
      message: n.querySelector(".te-msg")?.textContent ?? "",
      hint: n.querySelector(".te-hint")?.textContent ?? "",
    })),
  );

/* ═══════════════════════════════════════════════════════════════════════════
   46 · the editor in the settings panel, which is where the feature starts
   ═════════════════════════════════════════════════════════════════════════ */

console.log("\n46 · the code editor is the default face of Appearance");
await page.goto(url, { waitUntil: "networkidle" });
await page.waitForSelector(".wave-lane", { timeout: 15000 });
await settle(1500);
await page.click('button[aria-label="Settings"]');
await page.waitForSelector(".float-panel.settings", { timeout: 5000 });
await settle(700);
{
  const face = await page.evaluate(() => {
    const p = document.querySelector(".float-panel.settings");
    const tabs = [...p.querySelectorAll(".set-tabs button")].map((b) => ({
      label: b.textContent.trim(),
      on: b.dataset.on === "true",
    }));
    const box = p.querySelector(".te-code");
    return {
      tabs,
      hasEditor: !!p.querySelector(".theme-editor"),
      chars: box ? box.value.length : 0,
      buttons: [...p.querySelectorAll(".theme-editor button")].map((b) => b.textContent.trim()),
      simpleControlsVisible: !!p.querySelector(".set-swatch"),
    };
  });
  note(`tabs ${JSON.stringify(face.tabs)} · ${face.chars} characters in the box`);
  note(`actions: ${face.buttons.join(" / ")}`);
  if (!face.hasEditor) fail("the settings panel does not show the theme editor");
  if (face.tabs[0]?.on !== true) fail("the code editor is not the default tab");
  if (face.chars < 2000) fail(`the box came up with ${face.chars} characters, expected a document`);
  if (face.simpleControlsVisible) fail("the simple pickers are showing under the code tab");
  for (const want of ["Copy default", "Copy current", "Copy for agent", "Validate", "Apply", "Revert"]) {
    if (!face.buttons.includes(want)) fail(`the editor has no ${want} action`);
  }
}
await shotOf(page, "46-themedoc-settings-code");

// …and the pickers are still there, one click away, still working.
await page.click('.set-tabs button:has-text("Simple")');
await settle(400);
{
  const simple = await page.$$eval(".float-panel.settings", ([p]) => ({
    swatches: p.querySelectorAll(".set-swatch:not(.big)").length,
    selects: p.querySelectorAll(".select").length,
    hex: !!p.querySelector(".set-hex"),
    editor: !!p.querySelector(".theme-editor"),
  }));
  note(`simple tab: ${JSON.stringify(simple)}`);
  if (simple.swatches < 6 || !simple.hex || simple.selects < 2) {
    fail("the simple pickers lost controls when they were demoted to a tab");
  }
  if (simple.editor) fail("both faces are rendering at once");
}
await page.click('.set-tabs button:has-text("Theme code")');
await settle(300);

/* ═══════════════════════════════════════════════════════════════════════════
   47 · the editor window
   ═════════════════════════════════════════════════════════════════════════ */

console.log("\n47 · the editor in its own window");
const editor = await openPopup({
  who: "theme",
  open: () => page.click('.float-panel.settings .ghost-btn:has-text("Open window")'),
  selector: ".theme-editor",
  viewport: { width: 720, height: 840 },
  ready: 1200,
});
await shotOf(editor, "47-themedoc-editor-window");
{
  const ignores = await editor.evaluate(() => window.__onyxTheme.themeDocIgnored());
  note(`the editor window ignores the document it is editing: ${ignores}`);
  if (!ignores) fail("the editor window would wear the theme it is editing");
}

/* ── 47b · an input method mid-composition, which Tab used to eat ──────────
   Not a picture: a defect. The editor's `Tab` handler called `preventDefault()`
   and rewrote the box's value without asking whether an IME was composing, so
   picking a Pinyin candidate with `Tab` — the ordinary way to do it — tore the
   composition down and dropped the pending characters. The same omission was in
   the global map (`lib/keys.ts`), the accent field, the EQ window's keys and the
   editor window's `Escape`.

   Driven as an IME actually behaves: `compositionstart`, a phonetic buffer typed
   into the field with `isComposing` set, the keys the candidate window claims,
   then `compositionend` with the committed characters. The last step presses
   `Tab` with no composition in flight, because a guard that simply disabled the
   indent would pass every assertion above it. */

console.log("\n47b · Pinyin: Tab and ⌘Enter belong to the candidate window");
{
  const ime = await editor.evaluate(async () => {
    const el = document.querySelector(".te-code");
    el.focus();
    const wait = () => new Promise((r) => setTimeout(r, 80));
    /** Type into the controlled textarea the way the browser does. */
    const type = (value, composing) => {
      el.value = value;
      el.dispatchEvent(new InputEvent("input", { bubbles: true, isComposing: composing }));
    };
    /** Press a key; the answer is whether the app claimed it. */
    const press = (init) => {
      const e = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init });
      el.dispatchEvent(e);
      return e.defaultPrevented;
    };

    const base = '{\n  "onyx": "theme",\n  "name": "IME"\n}';
    type(base, false);
    await wait();

    // A composition begins and the phonetic buffer goes into the field.
    el.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true }));
    type(`${base}nihao`, true);
    await wait();
    const pending = el.value;
    // Tab picks a candidate; ⌘/Ctrl+Enter commits one. Neither is ours.
    const tabClaimed = press({ key: "Tab", code: "Tab", isComposing: true });
    const applyClaimed = press({ key: "Enter", code: "Enter", ctrlKey: true, isComposing: true });
    await wait();
    const afterKeys = el.value;

    // The IME commits, and the composition is over.
    el.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: "你好" }));
    type(`${base}你好`, false);
    await wait();
    const committed = el.value;

    // With nothing composing, Tab is the editor's own indent again.
    el.setSelectionRange(0, 0);
    const tabIndents = press({ key: "Tab", code: "Tab" });
    await wait();
    return { pending, tabClaimed, applyClaimed, afterKeys, committed, tabIndents, indented: el.value };
  });
  note(`while composing: Tab claimed ${ime.tabClaimed}, ⌘Enter claimed ${ime.applyClaimed}`);
  note(`buffer kept: ${JSON.stringify(ime.afterKeys.slice(-8))} · committed ${JSON.stringify(ime.committed.slice(-4))}`);
  if (ime.tabClaimed) fail("Tab was swallowed while an input method was composing");
  if (ime.applyClaimed) fail("⌘Enter applied the document while an input method was composing");
  if (ime.afterKeys !== ime.pending) fail("the pending composition was rewritten by a key handler");
  if (!ime.committed.includes("你好")) fail("the committed characters did not survive");
  if (!ime.tabIndents) fail("Tab no longer indents when nothing is composing");
  if (ime.indented === ime.committed) fail("the IME guard disabled the indent altogether");
}

/* ── and the global map, in the main window: SPEC §4's keys are not the
   candidate window's. `Space` is the one that hurts most — it is play/pause
   here and "commit" in half the input methods on earth. */
{
  const state = await engine("app_state");
  const before = state.transport.playing;
  const claimed = await page.evaluate(() => {
    const e = new KeyboardEvent("keydown", {
      key: " ",
      code: "Space",
      bubbles: true,
      cancelable: true,
      isComposing: true,
    });
    window.dispatchEvent(e);
    return e.defaultPrevented;
  });
  await settle(400);
  const during = (await engine("app_state")).transport.playing;
  // The same key with no composition in flight has to work, or this proves
  // nothing about the guard.
  const claimedIdle = await page.evaluate(() => {
    const e = new KeyboardEvent("keydown", {
      key: " ",
      code: "Space",
      bubbles: true,
      cancelable: true,
    });
    window.dispatchEvent(e);
    return e.defaultPrevented;
  });
  await settle(400);
  const after = (await engine("app_state")).transport.playing;
  note(`Space: playing ${before} → composing ${during} → idle ${after} (claimed ${claimed}/${claimedIdle})`);
  if (claimed) fail("the global map claimed Space while an input method was composing");
  if (during !== before) fail("Space toggled the transport while an input method was composing");
  if (!claimedIdle || after === during) fail("Space no longer toggles the transport");
  // Put the transport back where the rest of the run expects it.
  if (after !== before) {
    await page.evaluate(() =>
      window.dispatchEvent(new KeyboardEvent("keydown", { key: " ", code: "Space", bubbles: true, cancelable: true })),
    );
    await settle(400);
  }
}

/* ═══════════════════════════════════════════════════════════════════════════
   48–49 · a model's theme, applied to every window
   ═════════════════════════════════════════════════════════════════════════ */

console.log("\n48 · Cold Graphite, applied from the editor window");
const before = await tokens(page);
note(`main window before: ${JSON.stringify(before)}`);

await paste(editor, LLM);
{
  const state = await editor.evaluate(() => ({
    errors: document.querySelectorAll('.te-problems li[data-level="error"]').length,
    ok: document.querySelector(".te-ok")?.textContent ?? null,
    applyDisabled: document.querySelector(".primary-btn").disabled,
  }));
  note(`validation: ${JSON.stringify(state)}`);
  if (state.errors > 0) fail("the fixture does not validate in the real editor");
  if (state.applyDisabled) fail("Apply is disabled for a valid document");
}
await editor.click('.primary-btn:has-text("Apply")');
await editor.waitForTimeout(1200);
await settle(1200);

{
  const after = await tokens(page);
  const stored = await engine("app_state");
  note(`main window after:  ${JSON.stringify(after)}`);
  if (after.bg !== "#0b0e11") fail(`the document's background did not land: ${after.bg}`);
  if (after.accent === before.accent) fail("the accent did not move");
  if (after.uiFont !== "neutral") fail(`the document's UI font did not land: ${after.uiFont}`);
  if (after.rPanel !== "12px") fail(`a base token did not land: --r-panel is ${after.rPanel}`);
  if (after.barStep.trim() !== "6") fail(`waveform geometry did not land: ${after.barStep}`);
  if (!stored.themeDoc || !stored.themeDoc.includes("Cold Graphite")) {
    fail("the engine did not store the document");
  }
  if (stored.appearance.accent !== "#4f8fbf") {
    fail(`the document's appearance block did not persist: ${stored.appearance.accent}`);
  }

  const ratio = await legibility(page);
  note(`body text over the app background: ${ratio}:1`);
  if (ratio < 4.5) fail(`the applied theme is not legible: ${ratio}:1`);

  // usable, not only visible: the transport still responds to the app's keys
  await page.keyboard.press("Escape");
  await settle(300);
  const played = await page.evaluate(async () => {
    const was = (await window.__onyxMockHost.invoke("app_state")).transport.playing;
    return was;
  });
  await page.keyboard.press("Space");
  await settle(500);
  const now = (await engine("app_state")).transport.playing;
  if (now === played) fail("the app stopped responding to the keyboard under the new theme");
  await page.keyboard.press("Space");
  await settle(400);
}
await shotOf(page, "48-themedoc-applied-main");

// The editor window is *not* wearing it — that is the whole safety argument.
{
  const editorTokens = await tokens(editor);
  note(`editor window meanwhile: bg ${editorTokens.bg}, accent ${editorTokens.accent}`);
  if (editorTokens.bg === "#0b0e11") fail("the editor window is wearing the document it is editing");
  if (editorTokens.accent === before.accent) {
    fail("the editor window did not follow the document's *appearance* (it should)");
  }
}

console.log("\n49 · the EQ window, which never saw the editor");
{
  const eq = await openEq();
  const eqTokens = await tokens(eq);
  note(`EQ window: ${JSON.stringify(eqTokens)}`);
  if (eqTokens.bg !== "#0b0e11") fail(`the EQ window did not re-skin: ${eqTokens.bg}`);
  if (eqTokens.accent !== (await tokens(page)).accent) fail("the two windows disagree about the accent");
  await shotOf(eq, "49-themedoc-applied-eq");
  await eq.close();
  await settle(500);
}

/* ═══════════════════════════════════════════════════════════════════════════
   50 · a typo, with a line number and a suggestion
   ═════════════════════════════════════════════════════════════════════════ */

console.log("\n50 · an error a user can act on");
await paste(
  editor,
  ['{', '  "onyx": "theme",', '  "name": "Typo",', '  "dark": {', '    "tect-hi": "#ffffff",', '    "ink-900": "#101014"', "  }", "}"].join("\n"),
);
{
  const list = await problems(editor);
  note(`problems: ${JSON.stringify(list)}`);
  const first = list.find((p) => p.level === "error");
  if (!first) fail("a misspelled key did not produce an error");
  if (first && first.line !== "5") fail(`the error points at line ${first.line}, expected 5`);
  if (first && !first.message.includes("tect-hi")) fail("the error does not quote the key");
  if (first && !first.hint.includes("text-hi")) fail(`no "did you mean": ${first?.hint}`);
  const disabled = await editor.evaluate(() => document.querySelector(".primary-btn").disabled);
  if (!disabled) fail("Apply is offered for a document with an error");
  // …and nothing on screen moved while the box held a broken document
  const still = await tokens(page);
  if (still.bg !== "#0b0e11") fail(`the main window changed while an error was being typed: ${still.bg}`);
}
await shotOf(editor, "50-themedoc-error-line");

/* ═══════════════════════════════════════════════════════════════════════════
   51 · the contrast warning: measured, and never a block
   ═════════════════════════════════════════════════════════════════════════ */

console.log("\n51 · a legible warning about an illegible theme");
await paste(
  editor,
  [
    "{",
    '  "onyx": "theme",',
    '  "name": "Charcoal on Charcoal",',
    '  "dark": {',
    '    "ink-900": "#101010",',
    '    "text-hi": "#1b1b1b",',
    '    "text-mid": "#181818"',
    "  }",
    "}",
  ].join("\n"),
);
{
  const warn = await editor.evaluate(() => {
    const box = document.querySelector(".te-contrast");
    return {
      shown: !!box,
      head: box?.querySelector(".te-contrast-head")?.textContent?.trim() ?? "",
      rows: [...(box?.querySelectorAll("li") ?? [])].map((li) => li.textContent.replace(/\s+/g, " ")),
      applyDisabled: document.querySelector(".primary-btn").disabled,
      errors: document.querySelectorAll('.te-problems li[data-level="error"]').length,
    };
  });
  note(`contrast box: ${JSON.stringify(warn)}`);
  if (!warn.shown) fail("an illegible theme produced no contrast warning");
  if (warn.rows.length === 0) fail("the contrast warning names no failing pair");
  if (warn.errors > 0) fail("contrast was treated as an error");
  if (warn.applyDisabled) fail("contrast blocked the apply — it must warn, not block");
}
await shotOf(editor, "51-themedoc-contrast-warning");

/* ═══════════════════════════════════════════════════════════════════════════
   52–53 · wreck it, then recover with the keyboard
   ═════════════════════════════════════════════════════════════════════════ */

console.log("\n52 · a theme that makes the app unreadable, applied on purpose");
await editor.click('.primary-btn:has-text("Apply")');
await editor.waitForTimeout(1000);
await settle(1000);
{
  const wrecked = await tokens(page);
  const ratio = await legibility(page);
  note(`main window: bg ${wrecked.bg}, text ${wrecked.text}, ${ratio}:1`);
  if (wrecked.bg !== "#101010") fail("the illegible theme was not applied — it must be allowed");
  if (ratio > 2) fail(`this shot is supposed to be unreadable, and it is ${ratio}:1`);
}
await shotOf(page, "52-themedoc-unreadable");

console.log("\n53 · Ctrl+Alt+Shift+R, from the window nobody can read");
await page.bringToFront();
await page.keyboard.down("Control");
await page.keyboard.down("Alt");
await page.keyboard.down("Shift");
await page.keyboard.press("KeyR");
await page.keyboard.up("Shift");
await page.keyboard.up("Alt");
await page.keyboard.up("Control");
await settle(1200);
{
  const recovered = await tokens(page);
  const stored = await engine("app_state");
  const ratio = await legibility(page);
  note(`after the chord: ${JSON.stringify(recovered)} · ${ratio}:1`);
  note(`the engine's document is now ${JSON.stringify(stored.themeDoc)}`);
  if (recovered.bg !== "#0a0a0c") fail(`the designed background did not come back: ${recovered.bg}`);
  if (recovered.accent !== "#e8d9a0") fail(`the champagne accent did not come back: ${recovered.accent}`);
  if (stored.themeDoc !== null) fail("the reset did not clear the stored document");
  if (stored.appearance.accent !== "#c9a227") fail("the reset did not clear the appearance");
  if (ratio < 4.5) fail(`the app is still not legible after a reset: ${ratio}:1`);

  // The reset is an *appearance* reset: nothing else may have moved.
  if (!stored.playlist.length) fail("the reset cost the playlist");
  if (!stored.deckA.loaded) fail("the reset cost the loaded deck");
}
await shotOf(page, "53-themedoc-reset-recovered");

// and the editor window survived all of it
{
  const alive = await editor.evaluate(() => !!document.querySelector(".te-code"));
  if (!alive) fail("the editor window did not survive the round trip");
  await editor.close();
}

/* ═══════════════════════════════════════════════════════════════════════════
   54–55 · the compact editor at the 420 px floor, in both themes

   The editor in the settings panel is the same component in a third of the
   width (SPEC §4 gives the window a 420 px floor, §20.10 says the compact form
   has to hold there). The document below is valid *and* illegible on purpose,
   so the panel is carrying its tallest possible stack — toolbar, code box,
   status chips, a warning list and the contrast box — while it is measured.
   ═════════════════════════════════════════════════════════════════════════ */

const ILLEGIBLE = [
  "{",
  '  "onyx": "theme",',
  '  "name": "Charcoal on Charcoal",',
  '  "dark": {',
  '    "ink-900": "#101010",',
  '    "text-hi": "#1b1b1b",',
  '    "text-mid": "#181818"',
  "  }",
  "}",
].join("\n");

const NARROW = [
  ["dark", "54-themedoc-settings-420-dark"],
  ["light", "55-themedoc-settings-420-light"],
];

console.log("\n54–55 · the compact editor at 420 px, dark and light");
for (const [theme, name] of NARROW) {
  const narrow = await context.newPage();
  watch(narrow, `420 ${theme}`);
  await narrow.setViewportSize({ width: 420, height: 560 });
  await narrow.goto(`${url}/?theme=${theme}`, { waitUntil: "networkidle" });
  await narrow.waitForSelector(".wave-lane", { timeout: 15000 });
  await narrow.waitForTimeout(1200);
  await narrow.click('button[aria-label="Settings"]');
  await narrow.waitForSelector(".float-panel.settings .theme-editor", { timeout: 5000 });
  await paste(narrow, ILLEGIBLE);

  const fit = await narrow.evaluate(() => {
    const panel = document.querySelector(".float-panel.settings");
    const rows = (sel) => {
      const centres = [...document.querySelectorAll(sel)]
        .map((n) => n.getBoundingClientRect())
        .filter((b) => b.width > 0.5)
        .map((b) => b.top + b.height / 2)
        .sort((a, b) => a - b);
      let n = 0;
      let last = -1e9;
      for (const c of centres) {
        if (c - last > 6) n += 1;
        last = c;
      }
      return n;
    };
    const box = panel.getBoundingClientRect();
    // Anything inside the editor that is wider than the editor itself is a
    // spiller: a label that refused to ellipse, a button row that did not wrap.
    const editorBox = document.querySelector(".theme-editor").getBoundingClientRect();
    const spillers = [...document.querySelectorAll(".theme-editor *")]
      .filter((n) => n.getBoundingClientRect().right > editorBox.right + 0.5)
      .map((n) => n.className || n.tagName);
    return {
      theme: document.documentElement.dataset.theme,
      panelWidth: Math.round(box.width),
      editorWidth: Math.round(editorBox.width),
      barRows: rows(".te-bar button"),
      warnings: document.querySelectorAll('.te-problems li[data-level="warning"]').length,
      contrastRows: document.querySelectorAll(".te-contrast li").length,
      docOverflowX: document.documentElement.scrollWidth - document.documentElement.clientWidth,
      panelOverflowX: panel.scrollWidth - panel.clientWidth,
      panelScrolls: panel.scrollHeight > panel.clientHeight + 1,
      spill: [Math.round(box.right - window.innerWidth), Math.round(box.bottom - window.innerHeight)],
      spillers,
    };
  });
  note(`${theme} at 420: ${JSON.stringify(fit)}`);
  if (fit.theme !== theme) fail(`the ${theme} preview came up as ${fit.theme}`);
  if (fit.docOverflowX > 0) fail(`the ${theme} 420 px window scrolls sideways`);
  if (fit.panelOverflowX > 0) fail(`the compact editor overflows the panel in ${theme}`);
  if (fit.spill[0] > 1 || fit.spill[1] > 1) fail(`the panel spills ${fit.spill} in ${theme}`);
  if (fit.spillers.length) fail(`${theme}: ${fit.spillers.length} nodes overflow the editor: ${fit.spillers}`);
  if (fit.barRows !== 2) fail(`the compact toolbar is ${fit.barRows} rows in ${theme}, expected 2`);
  if (fit.contrastRows === 0) fail(`no contrast rows to measure the compact layout with (${theme})`);
  if (!fit.panelScrolls) fail(`the panel is not scrolling with the tallest editor stack (${theme})`);
  await shotOf(narrow, name);
  await narrow.close();
}

await finish();
