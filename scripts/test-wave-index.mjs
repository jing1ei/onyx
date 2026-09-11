// Generated in-memory samples only. No audio context, device or user files.
import { readFileSync } from 'node:fs';
import ts from 'typescript';
import assert from 'node:assert/strict';
import { performance } from 'node:perf_hooks';
const js = ts.transpileModule(readFileSync('src/editor/waveIndex.ts', 'utf8'), {compilerOptions:{module:ts.ModuleKind.ESNext}}).outputText;
const {WaveIndex, waveIndexes} = await import('data:text/javascript;base64,' + Buffer.from(js).toString('base64'));
function direct(data, a, b) {
  a = Math.max(0, Math.min(data.length, Math.floor(a)));
  b = Math.max(a, Math.min(data.length, Math.floor(b)));
  let peak = 0, squares = 0;
  for (let i = a; i < b; i++) { peak = Math.max(peak, Math.abs(data[i])); squares += data[i] * data[i]; }
  return {peak, rms:b > a ? Math.sqrt(squares / (b - a)) : 0};
}
let seed = 71;
function random() { seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0; return seed / 2 ** 32; }
for (const length of [1, 255, 256, 257, 1027, 48000]) {
  const data = Float32Array.from({length}, () => random() * 2 - 1);
  data[0] = 1.25; data[length - 1] = -1.5;
  const index = new WaveIndex(data);
  const ranges = [[0,length], [0,1], [length-1,length], [256,512], [-50,length+50], [0,0]];
  for (let i = 0; i < 200; i++) { const a = Math.floor(random()*length); ranges.push([a, a+Math.floor(random()*(length-a))]); }
  for (const [a,b] of ranges) {
    const actual = index.range(a,b), expected = direct(data,a,b);
    assert.equal(actual.peak, expected.peak);
    assert(Math.abs(actual.rms-expected.rms) < 1e-12);
  }
}
const left = new Float32Array([1,0,-.5]), right = new Float32Array([0,.25,0]);
const audio = {numberOfChannels:2, getChannelData:c => c ? right : left};
const indexes = waveIndexes(audio);
assert.equal(waveIndexes(audio), indexes);
assert.equal(indexes[0].range(0,3).peak,1);
assert.equal(indexes[1].range(0,3).peak,.25);
assert.notEqual(waveIndexes({...audio}), indexes);
// Isolate waveform statistics cost; this is not native Edit-switch latency.
const data = Float32Array.from({length:48000*60*8}, (_,i) => Math.sin(i*.017)*.8);
const before = performance.now(), index = new WaveIndex(data), built = performance.now();
function measure(query) {
  const times = [];
  for (let run = 0; run < 5; run++) {
    const start = performance.now();
    for (let bar = 0; bar < 400; bar++) query(Math.floor(bar*data.length/400), Math.floor((bar+1)*data.length/400));
    times.push(performance.now()-start);
  }
  return times.sort((a,b)=>a-b)[2];
}
const directMs = measure((a,b)=>direct(data,a,b));
const indexedMs = measure((a,b)=>index.range(a,b));
console.log(JSON.stringify({passed:true,fixture:'8 min / 48 kHz / one channel',indexBuildMs:built-before,directMs,indexedMs,speedup:directMs/indexedMs}));
