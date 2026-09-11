// UI regression test with an in-memory WAV and silent playback recorder.
// No native app, original files, or audio devices are used.
import {chromium} from 'playwright';
import assert from 'node:assert/strict';
const browser=await chromium.launch({executablePath:'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe',headless:true});
const page=await browser.newPage({viewport:{width:1180,height:760}});
const errors=[];page.on('pageerror',e=>errors.push(e.message));
await page.addInitScript(()=>{
  const n=48000*10, wav=new ArrayBuffer(44+n*4),v=new DataView(wav);
  const s=(at,text)=>[...text].forEach((c,i)=>v.setUint8(at+i,c.charCodeAt(0)));
  s(0,'RIFF');v.setUint32(4,36+n*4,true);s(8,'WAVE');s(12,'fmt ');v.setUint32(16,16,true);v.setUint16(20,3,true);v.setUint16(22,1,true);v.setUint32(24,48000,true);v.setUint32(28,192000,true);v.setUint16(32,4,true);v.setUint16(34,32,true);s(36,'data');v.setUint32(40,n*4,true);
  new Float32Array(wav,44).fill(.5);window.__testWave=wav;window.__starts=[];
  window.__liveSources=new Set();window.__maxSources=0;window.__nativePlays=0;window.__opens=0;
  window.AudioContext=class {
    state='running';destination={};get currentTime(){return performance.now()/1000;}
    resume(){return Promise.resolve();}close(){return Promise.resolve();}
    createBuffer(ch,length,sampleRate){return new AudioBuffer({numberOfChannels:ch,length,sampleRate});}
    createGain(){return {gain:{value:1},connect(){return this;}};}
    createBufferSource(){return {playbackRate:{value:1},connect(node){return node;},disconnect(){},stop(){window.__liveSources.delete(this);},start(when,offset,duration){window.__liveSources.add(this);window.__maxSources=Math.max(window.__maxSources,window.__liveSources.size);window.__starts.push({offset,duration,buffer:this.buffer});}};}
  };
});
await page.route('**/src/lib/mock.ts',async route=>{
  const res=await route.fetch();let body=await res.text();
  assert(body.includes('case "editor_io":'));
  body=body.replace('case "editor_io":','case "editor_io": { const r=raw.request; if(r.action==="open")return {canceled:false,name:"seek.wav",path:"seek.wav",byteLength:window.__testWave.byteLength}; if(r.action==="read")return window.__testWave.slice(r.offset || 0); return {canceled:true}; } case "unused_editor_test":');
  body=body.replace('if(r.action==="open")return', 'if(r.action==="open")window.__opens++; if(r.action==="open")return');
  body=body.replace('const r=raw.request;', 'const r=raw.request; if(r.action==="enter")return new Promise(resolve=>setTimeout(()=>resolve({canceled:true}),window.__enterDelay || 0));');
  body=body.replace('case "playlist_play_entry":', 'case "playlist_play_entry": window.__nativePlays++;');
  await route.fulfill({response:res,body});
});
try {
  await page.goto('http://127.0.0.1:1421/');
  await page.getByRole('tab',{name:'剪辑',exact:true}).click();
  await page.locator('.edit-empty').click();
  await page.waitForFunction(()=>document.querySelector('.edit-status')?.textContent.includes('已打开完整'));
  const wave=page.locator('.waveform');
  async function drag(from,to){const r=await wave.boundingBox();await page.mouse.move(r.x+r.width*from,r.y+30);await page.mouse.down();await page.mouse.move(r.x+r.width*to,r.y+30,{steps:6});await page.mouse.up();}
  for(const direction of ['淡入','淡出']){
    await drag(.2,.6);await page.getByRole('button',{name:direction,exact:true}).click();
    const selection=await page.locator('.edit-selection').innerText();
    await drag(.7,.8);
    assert.equal(await page.locator('.edit-selection').innerText(),selection);
    assert.equal(await page.evaluate(()=>window.__starts.length),direction==='淡入'?0:4);
    await page.getByRole('button',{name:'试听曲线',exact:true}).click();
    await page.waitForFunction(()=>Math.abs(window.__starts.at(-1)?.offset-8)<.03);
    assert(Math.abs(await page.evaluate(()=>window.__starts.at(-1).offset)-8)<.03);
    await drag(.7,.1);
    await page.waitForFunction(()=>Math.abs(window.__starts.at(-1)?.offset-1)<.03);
    assert(Math.abs(await page.evaluate(()=>window.__starts.at(-1).offset)-1)<.03);
    assert(await page.getByRole('button',{name:'停止试听',exact:true}).isVisible());
    assert(await page.evaluate(()=>window.__starts.at(-1).buffer===window.__starts.at(-2).buffer));
    const h=await page.getByRole('button',{name:'淡变曲率',exact:true}).boundingBox();
    await page.mouse.move(h.x+h.width/2,h.y+h.height/2);await page.mouse.down();await page.mouse.move(h.x+h.width/2,h.y-20,{steps:5});await page.mouse.up();
    await page.waitForFunction(()=>window.__liveSources.size===1);
    assert(await page.getByRole('button',{name:'停止试听',exact:true}).isVisible());
    assert(await page.evaluate(()=>window.__starts.at(-1).buffer!==window.__starts.at(-2).buffer));
    const slider=await page.getByRole('slider',{name:'曲率',exact:true}).boundingBox();
    await page.mouse.move(slider.x+slider.width*.5,slider.y+slider.height/2);await page.mouse.down();await page.mouse.move(slider.x+slider.width*.7,slider.y+slider.height/2,{steps:5});await page.mouse.up();
    await page.waitForFunction(()=>window.__liveSources.size===1);
    assert(await page.getByRole('button',{name:'停止试听',exact:true}).isVisible());
    assert.equal(await page.locator('.edit-selection').innerText(),selection);
    await page.getByRole('button',{name:'取消曲线',exact:true}).click();
  }
  await drag(.1,.3);assert.match(await page.locator('.edit-selection').innerText(),/2\.000/);
  // A held Space must not toggle repeatedly; rapid clicks must not overlap.
  await page.keyboard.press('Space');
  await page.waitForFunction(()=>window.__liveSources.size===1);
  const starts=await page.evaluate(()=>window.__starts.length);
  await page.evaluate(()=>window.dispatchEvent(new KeyboardEvent('keydown',{key:' ',code:'Space',repeat:true,bubbles:true})));
  assert.equal(await page.evaluate(()=>window.__starts.length),starts);
  assert.equal(await page.evaluate(()=>window.__liveSources.size),1);
  const nativePlays=await page.evaluate(()=>window.__nativePlays);
  await page.locator('.edit-playlist .pl-row').first().click();
  await page.waitForFunction(()=>window.__opens===2 && window.__liveSources.size===0);
  assert.equal(await page.evaluate(()=>window.__nativePlays),nativePlays);
  await page.evaluate(()=>{const button=document.querySelector('.edit-transport .primary.play');button.click();button.click();});
  await page.waitForTimeout(100);
  assert.equal(await page.evaluate(()=>window.__liveSources.size),0);
  assert.equal(await page.evaluate(()=>window.__maxSources),1);
  await page.evaluate(async()=>{
    const {switchMode}=await import('/src/lib/documentMode.ts');
    window.__enterDelay=150;
    document.querySelector('.edit-transport .primary.play').click();
    await switchMode(false);
  });
  assert.equal(await page.evaluate(()=>window.__liveSources.size),0);
  assert(await page.getByRole('tab',{name:'试听',exact:true}).getAttribute('aria-selected')==='true');
  assert.deepEqual(errors,[]);
  console.log('PASS: fades and seek; curve resume/cache; single live source; repeated Space; rapid clicks; playlist opens without native play; pending play canceled on mode exit. Silent mock only.');
} finally {await browser.close();}
