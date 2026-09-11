import { readFileSync } from 'node:fs';
import ts from 'typescript';
import assert from 'node:assert/strict';
const js = ts.transpileModule(readFileSync('src/editor/fade.ts','utf8'), {compilerOptions:{module:ts.ModuleKind.ESNext}}).outputText;
const {fadeSamples,fadeGain}=await import('data:text/javascript;base64,'+Buffer.from(js).toString('base64'));
for(const power of [.125,.5,1,2,8])for(const direction of ['in','out']){
  const data=new Float32Array(5).fill(1);fadeSamples(data,0,5,direction,power);
  for(let i=0;i<5;i++)assert(Math.abs(data[i]-fadeGain(i/4,direction,power))<1e-7);
  assert.equal(data[0],direction==='in'?0:1);assert.equal(data[4],direction==='in'?1:0);
}
for(const direction of ['in','out']) {
  const data=new Float32Array(12).fill(1);
  fadeSamples(data,3,8,direction);
  assert.deepEqual([...data],direction==='in'?[1,1,1,0,.25,.5,.75,1,1,1,1,1]:[1,1,1,1,.75,.5,.25,0,1,1,1,1]);
  const right=new Float32Array(12).fill(-.5);fadeSamples(right,3,8,direction);
  assert.deepEqual([...right],[...data].map(v=>v===0?-0:-.5*v));
  const short=new Float32Array([1,1]);fadeSamples(short,0,1,direction);assert.deepEqual([...short],[0,1]);
}
const empty=new Float32Array([1,2]);fadeSamples(empty,1,1,'in');assert.deepEqual([...empty],[1,2]);
console.log('Fade in/out: exact endpoints, independent directions, selection-only changes, stereo polarity and short selections passed');
