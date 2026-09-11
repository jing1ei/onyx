// Exercises the real Editor switch/preload branches against a fake native IO.
// No Onyx process, user files, AudioContext or audio devices are used.
import {chromium} from 'playwright';
import assert from 'node:assert/strict';
const browser=await chromium.launch({executablePath:'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe',headless:true});
try {
  for(const warmed of [false,true]) {
    const page=await browser.newPage({viewport:{width:1180,height:760}});
    const errors=[];page.on('pageerror',e=>errors.push(e.message));
    await page.addInitScript(()=>{
      window.__counts={}; window.__path='I:/fixtures/three-minute.wav';window.__canonical=String.raw`\\?\I:\fixtures\three-minute.wav`;
      const frames=48000*180, align=6, bytes=new ArrayBuffer(44+frames*align),v=new DataView(bytes);
      const str=(at,s)=>[...s].forEach((c,i)=>v.setUint8(at+i,c.charCodeAt(0)));
      str(0,'RIFF');v.setUint32(4,bytes.byteLength-8,true);str(8,'WAVE');str(12,'fmt ');v.setUint32(16,16,true);
      v.setUint16(20,1,true);v.setUint16(22,2,true);v.setUint32(24,48000,true);v.setUint32(28,48000*align,true);v.setUint16(32,align,true);v.setUint16(34,24,true);
      str(36,'data');v.setUint32(40,frames*align,true);
      for(let i=0;i<frames;i++){v.setUint8(44+i*align+2,64);v.setUint8(44+i*align+5,192);}
      window.__wav=bytes;
      window.AudioContext=class {constructor(){throw new Error('AudioContext is forbidden in switch test');}};
    });
    await page.route('**/src/editor/Editor.tsx',async route=>{
      const res=await route.fetch();let body=await res.text();
      assert(body.includes('!MOCK && path &&'));
      body=body.replace('!MOCK && path &&','path &&').replace('MOCK || active || dirty','active || dirty').replace('MOCK || !active || busy','!active || busy');
      await route.fulfill({response:res,body});
    });
    await page.route('**/src/lib/mock.ts',async route=>{
      const res=await route.fetch();let body=await res.text();
      assert(body.includes('return structuredClone(state);'));
      body=body.replace('return structuredClone(state);',`const s=structuredClone(state);s.deckA.info={...s.deckA.info,path:window.__path};s.deckA.decoded=true;s.transport.activeDeck='a';s.ab.enabled=false;s.blind.active=false;return s;`);
      body=body.replace('case "editor_io":',`case "editor_io": { const r=raw.request; window.__counts[r.action]=(window.__counts[r.action]||0)+1;
        if(r.action==='open'||r.action==='prepare')return {canceled:false,name:'three-minute.wav',path:window.__canonical,byteLength:window.__wav.byteLength,preparedId:'1'};
        if(r.action==='read')return window.__wav.slice(r.offset||0,(r.offset||0)+8*1024*1024);
        if(r.action==='preview'||r.action==='overwrite'||r.action==='saveAs')throw new Error('Unchanged document was encoded or saved');
        return {canceled:true}; } case "unused_editor_test":`);
      await route.fulfill({response:res,body});
    });
    await page.goto('http://127.0.0.1:1421/');
    if(warmed)await page.waitForFunction(()=>performance.getEntriesByName('onyx-editor-preloaded').length>0);
    const first=await page.evaluate(async()=>{const {switchMode}=await import('/src/lib/documentMode.ts');const at=performance.now();await switchMode(true);return performance.now()-at;});
    await page.waitForSelector('.channel-stack.stereo');
    const initial=await page.evaluate(()=>({...window.__counts}));
    const times=[];
    for(let i=0;i<5;i++)times.push(await page.evaluate(async()=>{
      const {switchMode,samePath}=await import('/src/lib/documentMode.ts');
      if(!samePath(window.__path,window.__canonical))throw new Error('Canonical path mismatch');
      if(!samePath(String.raw`\\server\share\a.wav`,String.raw`\\?\UNC\server\share\a.wav`))throw new Error('UNC mismatch');
      await switchMode(false);const at=performance.now();await switchMode(true);return performance.now()-at;
    }));
    const final=await page.evaluate(()=>({...window.__counts}));
    assert.equal(final.open,1);assert.equal(final.read,initial.read);assert.equal(final.prepare,initial.prepare);
    assert.deepEqual(errors,[]);
    console.log(JSON.stringify({warmed,firstMs:first,repeatMs:times,openCalls:final.open,readCalls:final.read,prepareCalls:final.prepare||0,nativeHardware:false}));
    await page.close();
  }
} finally {await browser.close();}
