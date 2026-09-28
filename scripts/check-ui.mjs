import assert from "node:assert/strict";
import { chromium, webkit } from "playwright";
import { shotsUrl } from "./lib/shots-base.mjs";

const browser = await (process.env.ONYX_BROWSER === "webkit" ? webkit : chromium).launch();
try {
  const page = await browser.newPage({ viewport: { width: 1180, height: 760 } });
  if (process.env.ONYX_LEGACY_WEBKIT === '1') {
    await page.addInitScript(() => {
      delete HTMLElement.prototype.inert;
      const supports = CSS.supports.bind(CSS);
      CSS.supports = (...args) => args.some(v => String(v).includes('oklch(')) ? false : supports(...args);
      const add = EventTarget.prototype.addEventListener;
      EventTarget.prototype.addEventListener = function(type, listener, options) {
        if (options && typeof options === 'object' && 'signal' in options) throw new Error('unsupported signal option');
        return add.call(this, type, listener, options);
      };
    });
  }
  const errors = [];
  page.on("pageerror", e => errors.push(e.message));
  await page.goto(shotsUrl());
  await page.waitForSelector(".wave-lane");
  const state = () => page.evaluate(() => window.__onyxMockHost.invoke("app_state"));
  assert.equal((await state()).ab.enabled,false,"comparison starts off");
  await page.keyboard.press('b');
  assert.equal((await state()).transport.activeDeck,'a','B key cannot select B while comparison is off');
  await page.keyboard.press('Tab');
  assert.equal((await state()).transport.activeDeck,'a','Tab cannot bypass comparison off');
  await page.evaluate(() => window.__onyxMockHost.invoke('playlist_clear'));
  await page.waitForSelector('.pl-empty');
  assert.match(await page.locator('.pl-empty').innerText(), /starts playing/);
  await page.locator('.blind-btn').click();
  await page.getByText('Choose two tracks first', {exact:true}).waitFor();
  assert.equal(await page.getByRole('button',{name:'Begin',exact:true}).isEnabled(),false);
  await page.getByRole('button',{name:'Choose tracks',exact:true}).click();
  await page.getByRole('button',{name:'Open files',exact:true}).click();
  await page.waitForSelector('.pl-row');
  await page.getByRole('button',{name:'Add',exact:true}).click();
  await page.locator('.pl-row').nth(1).locator('.deck-chip[data-deck="b"]').click();
  assert.equal((await state()).ab.enabled,false,'assigning B must not enable comparison');
  await page.evaluate(() => window.__onyxMockHost.invoke('ab_set_enabled',{value:false}));
  await page.locator('.blind-btn').click();
  assert.equal(await page.getByRole('button',{name:'Begin',exact:true}).isEnabled(),false);
  await page.getByRole('button',{name:'Enable A/B',exact:true}).click();
  await page.getByRole('button',{name:'Begin',exact:true}).click();
  await page.waitForFunction(() => window.__onyxMockHost.invoke('app_state').then(s=>s.blind.active && s.ab.enabled));
  await page.getByRole('region',{name:'Blind listening test'}).getByRole('button',{name:'Close',exact:true}).click();
  await page.waitForFunction(() => window.__onyxMockHost.invoke('app_state').then(s=>!s.blind.active));
  await page.reload();
  await page.waitForSelector('.pl-row');
  const initial = await state();
  assert.equal(initial.ab.enabled,false,'new session does not restore A/B');
  await page.locator('.pl-row').nth(2).click();
  await page.waitForFunction(id=>window.__onyxMockHost.invoke('app_state').then(s=>s.deckA.entryId===id),initial.playlist[2].id);
  const clicked = await state();
  assert.equal(clicked.deckB.entryId,initial.deckB.entryId,'single click preserves other deck');
  assert.equal(clicked.transport.playing,true);
  assert.equal(clicked.ab.enabled,false);
  await page.getByRole('button',{name:'A/B comparison',exact:true}).click();
  await page.waitForFunction(()=>document.querySelector('[aria-label="A/B comparison"]').getAttribute('aria-pressed')==='true');
  await page.keyboard.press('b');
  await page.waitForFunction(()=>window.__onyxMockHost.invoke('app_state').then(s=>s.transport.activeDeck==='b'));
  const comparing = await state();
  await page.locator('.pl-row').nth(3).click();
  await page.waitForFunction(id=>window.__onyxMockHost.invoke('app_state').then(s=>s.deckB.entryId===id),initial.playlist[3].id);
  assert.equal((await state()).deckA.entryId,comparing.deckA.entryId,'click on B preserves A');
  assert.equal((await state()).transport.activeDeck,'b');
  assert.equal((await state()).ab.enabled,true);

  await page.keyboard.press("Shift+Tab");
  assert.notEqual(await page.evaluate(() => document.activeElement.tagName), "BODY",
    "Shift+Tab must enter keyboard navigation");
  const chip = page.locator('.deck-chip[data-deck="b"]').first();
  await chip.focus();
  await page.keyboard.press("Space");
  await page.waitForFunction(id => document.querySelector('.pl-row')?.dataset.selected === 'true' &&
    window.__onyxMockHost.invoke("app_state").then(s => s.deckB.entryId === id), initial.playlist[0].id);
  let current = await state();
  assert.equal(current.deckB.entryId, initial.playlist[0].id, "Space activates focused deck chip");
  assert.equal(current.transport.playing, initial.transport.playing, "Space on a button must not pause playback");
  await page.keyboard.press("Tab");
  assert.equal(await chip.evaluate(n => n === document.activeElement), false, "Tab advances focus");

  await page.locator('.pl-row').first().click({ button: 'right' });
  await page.getByRole('button', { name: 'Remove Del', exact: true }).click();
  await page.waitForFunction(n => document.querySelectorAll('.pl-row').length === n, initial.playlist.length - 1);
  assert.equal(await page.locator('.pl-row').count(), initial.playlist.length - 1, "menu action must run before dismissal");
  await page.locator('.pl-row').first().click({ button: 'right' });
  await page.keyboard.press('Escape');
  assert.equal(await page.locator('.ctx-menu').count(), 0, "Escape dismisses menu");

  await page.getByRole('button', { name: /Settings/ }).click();
  await page.setViewportSize({ width: 420, height: 560 });
  const openWindow = page.getByRole('button', { name: 'Open window', exact: true });
  assert.equal(await openWindow.evaluate(n => n.scrollHeight <= n.clientHeight), true,
    'settings action label must fit inside its button at minimum width');
  const editor = page.getByRole('textbox', { name: 'Theme document', exact: true });
  await editor.focus();
  await page.keyboard.press('Escape');
  assert.equal(await editor.evaluate(n => n === document.activeElement), false, "Escape exits theme editor");
  assert.equal(await page.locator('.float-panel.settings').count(), 1, "first Escape preserves settings panel");
  await page.keyboard.press('Escape');
  assert.equal(await page.locator('.float-panel.settings').count(), 0, "second Escape closes panel");
  await page.setViewportSize({ width: 1180, height: 760 });

  await page.emulateMedia({ reducedMotion: 'reduce' });
  const animation = await page.evaluate(() => {
    const el = document.createElement('div'); el.className = 'shimmer'; document.body.append(el);
    const result = getComputedStyle(el).animationName; el.remove(); return result;
  });
  assert.equal(animation, 'none', "reduced motion disables decorative animation");
  const band = {id:1,enabled:true,kind:'bell',freqHz:1000,gainDb:4,q:1,slopeDbOct:12};
  await page.evaluate(config => window.__onyxMockHost.invoke('set_eq',{config}), {enabled:false,bands:[band]});
  const popup = page.waitForEvent('popup');
  await page.keyboard.press('e');
  const eq = await popup;
  if (!eq) throw new Error('EQ popup did not open');
  await eq.waitForSelector('.eq-canvas-wrap');
  await eq.locator('.eq-status[data-on="true"]').waitFor();
  const eqButton = page.getByRole('button',{name:'EQ processing',exact:true});
  await page.waitForFunction(()=>document.querySelector('[aria-label="EQ processing"]').textContent.includes('EQ ON'));
  assert.equal((await state()).eq.enabled,true,'opening a bypassed EQ engages it');
  assert.deepEqual((await state()).eq.bands,[band],'opening preserves the saved bands');
  await page.setViewportSize({width:420,height:560});
  assert.equal(await eqButton.isVisible(),true);
  assert.equal(await eqButton.evaluate(n => n.scrollWidth <= n.clientWidth),true,'EQ ON fits narrow titlebar');
  await page.setViewportSize({width:1180,height:760});

  const beforeRepeat = (await state()).transport.playing;
  await eq.evaluate(() => window.dispatchEvent(new KeyboardEvent('keydown', { key: ' ', repeat: true, bubbles: true })));
  assert.equal((await state()).transport.playing, beforeRepeat, "held Space does not repeatedly toggle playback in EQ");
  await eq.close();
  await page.waitForFunction(()=>window.__onyxMockHost.invoke('eq_window_state').then(s=>!s.open));
  assert.match(await eqButton.innerText(), /EQ ON/, 'closing editor retains the filtering reminder');
  await eqButton.click();
  await page.waitForFunction(()=>window.__onyxMockHost.invoke('app_state').then(s=>!s.eq.enabled));
  await page.waitForFunction(()=>document.querySelector('[aria-label="EQ processing"]').dataset.eqActive==='false');
  assert.equal((await page.evaluate(()=>window.__onyxMockHost.invoke('eq_window_state'))).open,false,'power control must not open editor');
  assert.deepEqual((await state()).eq.bands,[band],'turning off preserves bands');
  await eqButton.click();
  await page.waitForFunction(()=>window.__onyxMockHost.invoke('app_state').then(s=>s.eq.enabled));
  assert.equal((await page.evaluate(()=>window.__onyxMockHost.invoke('eq_window_state'))).open,false,'turning on must not open editor');
  await eqButton.click();
  await page.waitForFunction(()=>window.__onyxMockHost.invoke('app_state').then(s=>!s.eq.enabled));
  const reopening = page.waitForEvent('popup');
  await page.getByRole('button',{name:'Equaliser',exact:true}).click();
  const reopened = await reopening;
  await reopened.locator('.eq-status[data-on="true"]').waitFor();
  assert.equal(await reopened.getByRole('button',{name:/bypass|engaged/i}).count(),0,'editor no longer has global bypass button');
  await eqButton.click();
  await reopened.locator('.eq-status[data-on="false"]').waitFor();
  await eqButton.click();
  await reopened.locator('.eq-status[data-on="true"]').waitFor();
  assert.equal((await state()).eq.enabled,true,'reopening engages EQ again');
  await page.evaluate(config=>window.__onyxMockHost.invoke('set_eq',{config}),{enabled:true,bands:[{...band,enabled:false}]});
  await page.waitForFunction(()=>document.querySelector('[aria-label="EQ processing"]').dataset.eqActive==='false');
  await reopened.close();
  await page.reload();
  await page.waitForSelector('.pl-row');
  const rows = page.locator('.pl-row');
  const initialUI = await state();
  await rows.first().focus();
  await page.keyboard.press('ArrowDown');
  assert.equal(await rows.nth(1).evaluate(n => n === document.activeElement), true);
  assert.equal((await state()).transport.volume, initialUI.transport.volume, 'row navigation does not change volume');
  await page.keyboard.press('Enter');
  await page.waitForFunction(id=>window.__onyxMockHost.invoke('app_state').then(s=>s.deckA.entryId===id),initialUI.playlist[1].id);
  await rows.nth(1).locator('.deck-chip[data-deck="a"]').click();
  await page.waitForFunction(id => window.__onyxMockHost.invoke('app_state').then(s => s.deckA.entryId === id), initialUI.playlist[1].id);
  const volume = page.getByRole('slider', { name: 'Volume', exact: true });
  await volume.focus();
  await page.keyboard.press('Home');
  await page.waitForFunction(() => window.__onyxMockHost.invoke('app_state').then(s => s.transport.volume === 0));
  await page.keyboard.press('ArrowRight');
  await page.waitForFunction(() => window.__onyxMockHost.invoke('app_state').then(s => Math.abs(s.transport.volume - .01) < .00001));
  await page.keyboard.press('End');
  await page.waitForFunction(() => window.__onyxMockHost.invoke('app_state').then(s => s.transport.volume === 1));

  for (const [width, height] of [[1180,760], [640,560], [420,560]]) {
    await page.setViewportSize({width, height});
    await page.getByRole('button', {name:'Shortcuts', exact:true}).click();
    const dialog = page.getByRole('dialog', {name:'Keyboard'});
    await page.keyboard.press('Tab');
    assert.equal(await dialog.evaluate(n => n.contains(document.activeElement)), true, 'focus stays in shortcuts');
    await page.keyboard.press('Shift+Tab');
    assert.equal(await dialog.evaluate(n => n.contains(document.activeElement)), true);
    const previousVolume = (await state()).transport.volume;
    await page.keyboard.press('ArrowDown');
    assert.equal((await state()).transport.volume, previousVolume, 'overlay arrows must not change playback');
    assert.equal(await dialog.evaluate(n => {
      const r=n.getBoundingClientRect(); return r.left>=0 && r.right<=innerWidth && r.top>=0 && r.bottom<=innerHeight;
    }), true, 'shortcuts fit viewport');
    await dialog.evaluate(n => n.scrollTop=n.scrollHeight);
    assert.equal(await dialog.getByRole('button',{name:'Close shortcuts',exact:true}).evaluate(n => {
      const r=n.getBoundingClientRect();return r.top>=0 && r.bottom<=innerHeight;
    }), true, 'shortcuts close stays reachable when scrolled');
    await page.keyboard.press('Escape');
    assert.equal(await page.getByRole('button',{name:'Shortcuts',exact:true}).evaluate(n => n === document.activeElement), true);
  }
  await page.getByRole('button',{name:'Settings',exact:true}).click();
  const settings = page.getByRole('region',{name:'Settings',exact:true});
  await settings.evaluate(n => n.scrollTop=n.scrollHeight);
  assert.equal(await settings.getByRole('button',{name:'Close',exact:true}).evaluate(n => {
    const r=n.getBoundingClientRect();return r.top>=0 && r.bottom<=innerHeight;
  }), true, 'settings close stays reachable at bottom');
  await settings.getByRole('button',{name:'Close',exact:true}).click();
  await page.setViewportSize({width:1180,height:760});
  if (process.env.ONYX_LEGACY_WEBKIT === '1') {
    await page.evaluate(() => window.__onyxMockHost.invoke('set_theme_doc', {
      text: JSON.stringify({onyx:'theme',version:1,light:{'ink-900':'oklch(0.3 0.02 240)'}}),
    }));
    await page.waitForFunction(() => /^#[0-9a-f]{6}$/.test(document.documentElement.style.getPropertyValue('--ink-900')));
  }
  const productionIndex = process.argv.indexOf('--production-url');
  if (productionIndex >= 0) {
    const recovery = await browser.newPage();
    await recovery.addInitScript(() => {
      let seq = 0;
      window.__TAURI_INTERNALS__ = {
        transformCallback: () => ++seq,
        metadata: {currentWindow: {label:'main'}, currentWebview: {label:'main'}},
        invoke: async cmd => {
          if (cmd === 'app_state') throw new Error('Test engine unavailable');
          if (cmd === 'plugin:event|listen') return ++seq;
          if (cmd === 'eq_window_state') return {open:false,pinned:true};
          return null;
        },
      };
    });
    await recovery.goto(process.argv[productionIndex + 1]);
    const reconnect = recovery.getByRole('button', {name:'Reconnect',exact:true});
    await reconnect.waitFor();
    assert.match(await recovery.locator('.connection-error').innerText(), /Test engine unavailable/);
    const reloaded = recovery.waitForEvent('domcontentloaded');
    await reconnect.click();
    await reloaded;
    await recovery.close();
  }
  assert.deepEqual(errors, []);
  console.log('UI regression checks passed: menu, playlist, volume, panels, shortcuts, editor, reduced motion, EQ repeat.');
} finally {
  await browser.close();
}
