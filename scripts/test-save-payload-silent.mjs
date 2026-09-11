// In-memory browser test only: no native app, file writes, or audio device.
import {readFileSync} from 'node:fs';
import assert from 'node:assert/strict';
import ts from 'typescript';
import {chromium} from 'playwright';
const source=readFileSync('src/editor/Editor.tsx','utf8');
const helpers=source.slice(source.indexOf('function encodeWav('),source.indexOf('export default function Editor'));
const save=source.slice(source.indexOf('  async function nativeSave('),source.indexOf('  function showSaveSuccess('));
const js=s=>ts.transpile(s,{target:ts.ScriptTarget.ES2022});
const browser=await chromium.launch({executablePath:'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe',headless:true});
try {
  const page=await browser.newPage();
  const result=await page.evaluate(async ({helpers,save})=>{
    window.AudioContext=class {constructor(){throw Error('Audio devices forbidden');}};
    const make=new Function(helpers+';return {encodeWav,wavPayload}');
    const {encodeWav,wavPayload}=make();
    const buffer=new AudioBuffer({numberOfChannels:2,length:48000*180,sampleRate:48000});
    buffer.getChannelData(0).fill(.25);buffer.getChannelData(1).fill(-.5);
    let start=performance.now();
    const raw=new Uint8Array(await encodeWav(buffer).arrayBuffer()),chunks=[];
    for(let i=0;i<raw.length;i+=32768)chunks.push(String.fromCharCode(...raw.subarray(i,i+32768)));
    const old=btoa(chunks.join('')),oldMs=performance.now()-start;
    start=performance.now();const payload=await wavPayload(buffer),newMs=performance.now()-start;
    if(old!==payload)throw Error('Encoded audio changed');
    const exercise=new Function('mode','alreadySynced','outcome', `
      return (async()=>{
        const buffer={},fadeDraft=false,ioBusy={current:false},savedBuffer={current:null};
        const sourcePath={current:'source.wav'},syncedBuffer={current:alreadySynced?buffer:null};
        const fileName='source',sourceExtension='wav',calls=[],notices=[];
        const setBusy=()=>{},setSaveToast=()=>{},setNotice=x=>notices.push(x),stop=()=>{};
        const setFileName=()=>{},setSourceExtension=()=>{},t=x=>x,api={errorMessage:String};
        const showSaveSuccess=x=>notices.push(x),samePath=(a,b)=>a===b;
        const wavPayload=async()=>{calls.push('encode');return 'exact-payload';};
        const editorIo=async req=>{calls.push(req);if(outcome==='fail')throw Error('write failed');return {canceled:outcome==='cancel',path:mode==='saveAs'?'other.wav':'source.wav'};};
        const syncAudition=async payload=>{if(syncedBuffer.current===buffer)return;calls.push({action:'preview',bytes:payload});syncedBuffer.current=buffer;};
        ${save}
        await nativeSave(mode);
        return {calls,saved:savedBuffer.current===buffer,busy:ioBusy.current,notices};
      })();`);
    const cases=[];
    for(const args of [['overwrite',true,'ok'],['overwrite',false,'ok'],['saveAs',true,'ok'],['overwrite',false,'fail'],['overwrite',false,'cancel']])cases.push(await exercise(...args));
    return {oldMs,newMs,cases};
  },{helpers:js(helpers),save:js(save)});
  const [synced,changed,saveAs,failed,canceled]=result.cases;
  assert.equal(synced.calls.length,2);
  for(const item of [changed,saveAs]){assert.equal(item.calls.length,3);assert.equal(item.calls[1].bytes,item.calls[2].bytes);}
  for(const item of result.cases){assert.equal(item.calls.filter(x=>x==='encode').length,1);assert.equal(item.busy,false);}
  assert.equal(failed.saved,false);assert.equal(canceled.saved,false);
  assert.equal(failed.calls.length,2);assert.equal(canceled.calls.length,2);
  console.log(JSON.stringify({passed:true,oldMs:result.oldMs,newMs:result.newMs,cases:5,nativeAudio:false}));
} finally {await browser.close();}
