import assert from 'node:assert/strict';
import {mkdir} from 'node:fs/promises';
import {join} from 'node:path';
import {chromium} from 'playwright';
import {ROOT, shotsUrl} from './lib/shots-base.mjs';

const out = join(ROOT, 'docs/screenshots');
await mkdir(out, {recursive:true});
const browser = await chromium.launch();
try {
  const context = await browser.newContext({viewport:{width:1180,height:760},deviceScaleFactor:1});
  const errors = [];
  context.on('page', p => p.on('pageerror', e => errors.push(e.message)));
  const page = await context.newPage();
  await page.goto(shotsUrl());
  await page.waitForSelector('.wave-lane');
  await page.waitForTimeout(4500);
  const metadata = await page.evaluate(() => window.__onyxMockHost.invoke('app_state'));
  assert.ok(metadata.playlist.every(e => e.title === 'Song name' && e.artist === 'Artist name'),
    'README screenshots must use placeholder metadata');
  const engine = (cmd,args={}) => page.evaluate(([c,a])=>window.__onyxMockHost.invoke(c,a),[cmd,args]);
  const shot = async (p,name) => {
    for (const button of await p.locator('.toast-dismiss').all()) await button.click();
    await p.waitForTimeout(350);
    await p.screenshot({path:join(out,name+'.png')});
    console.log(name+'.png');
  };
  await engine('transport_play');
  await engine('transport_seek',{secs:82});
  await engine('ab_set_enabled',{value:false});
  await page.setViewportSize({width:480,height:560});
  await page.waitForTimeout(500);
  await shot(page,'compact');
  await page.setViewportSize({width:1180,height:760});
  await engine('ab_set_enabled',{value:true});
  await engine('set_level_match',{enabled:true});
  await page.waitForTimeout(1200);
  await shot(page,'compare');

  const popup = page.waitForEvent('popup');
  await page.getByRole('button',{name:'Equaliser',exact:true}).click();
  const eq = await popup;
  await eq.setViewportSize({width:940,height:560});
  await eq.waitForSelector('.eq-canvas-wrap');
  await eq.waitForTimeout(300);
  // Fixed demonstration curve keeps documentation captures repeatable.
  await engine('set_eq', {config:{enabled:true,bands:[
    {id:1,enabled:true,kind:'bell',freqHz:80,gainDb:-4.8,q:1,slopeDbOct:12},
    {id:2,enabled:true,kind:'bell',freqHz:420,gainDb:5.4,q:1,slopeDbOct:12},
    {id:3,enabled:true,kind:'bell',freqHz:1900,gainDb:-4,q:1,slopeDbOct:12},
    {id:4,enabled:true,kind:'bell',freqHz:6200,gainDb:3.5,q:1,slopeDbOct:12},
  ]}});
  await eq.waitForFunction(()=>document.querySelectorAll('.eq-row').length===4);
  await engine('transport_play');
  await eq.waitForTimeout(800);
  await shot(eq,'equaliser');
  await eq.close();

  await page.locator('.blind-btn').click();
  await page.getByRole('button',{name:'Begin',exact:true}).click();
  await page.waitForSelector('.blind-slots');
  await shot(page,'blind');
  await page.getByRole('region',{name:'Blind listening test'}).getByRole('button',{name:'Close',exact:true}).click();
  await engine('transport_play');
  await engine('transport_seek',{secs:82});
  await page.getByRole('button',{name:'Settings',exact:true}).click();
  await page.getByRole('tab',{name:'Simple',exact:true}).click();
  await page.getByRole('button',{name:'Light',exact:true}).click();
  await page.waitForFunction(()=>document.documentElement.dataset.theme==='light');
  await page.getByRole('region',{name:'Settings',exact:true}).getByRole('button',{name:'Close',exact:true}).click();
  await shot(page,'light');
  assert.deepEqual(errors,[]);
} finally {
  await browser.close();
}
