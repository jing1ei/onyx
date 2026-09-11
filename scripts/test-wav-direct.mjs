// Generated fixtures only; FFmpeg decodes to a file and never plays audio.
import {readFileSync,writeFileSync,mkdtempSync} from 'node:fs';
import {execFileSync} from 'node:child_process';
import path from 'node:path';
import assert from 'node:assert/strict';
import ts from 'typescript';
const source=readFileSync('src/editor/wave.tsx','utf8').split('export function WaveCanvas')[0].replace(/^import .*;\r?\n/gm,'');
const js=ts.transpileModule(source,{compilerOptions:{module:ts.ModuleKind.ESNext}}).outputText;
globalThis.AudioBuffer=class {
  constructor({numberOfChannels,length,sampleRate}){Object.assign(this,{numberOfChannels,length,sampleRate});this.data=Array.from({length:numberOfChannels},()=>new Float32Array(length));}
  getChannelData(c){return this.data[c];}
};
const {readFloatWav}=await import('data:text/javascript;base64,'+Buffer.from(js).toString('base64'));
function fixture(format,bits,frames,extended=false) {
  const width=bits/8,align=width*2,header=extended?68:44,buffer=new ArrayBuffer(header+frames*align),v=new DataView(buffer);
  const str=(at,s)=>[...s].forEach((c,i)=>v.setUint8(at+i,c.charCodeAt(0)));
  str(0,'RIFF');v.setUint32(4,buffer.byteLength-8,true);str(8,'WAVE');str(12,'fmt ');v.setUint32(16,extended?40:16,true);
  v.setUint16(20,extended?65534:format,true);v.setUint16(22,2,true);v.setUint32(24,48000,true);v.setUint32(28,48000*align,true);v.setUint16(32,align,true);v.setUint16(34,bits,true);
  if(extended){v.setUint16(36,22,true);v.setUint16(38,bits,true);v.setUint32(40,3,true);v.setUint16(44,format,true);new Uint8Array(buffer,46,14).set([0,0,0,0,16,0,128,0,0,170,0,56,155,113]);}
  str(header-8,'data');v.setUint32(header-4,frames*align,true);
  for(let i=0;i<frames;i++)for(let c=0;c<2;c++) {
    const at=header+(i*2+c)*width, value=[-1,-.5,0,.25,.5,.9921875][(i+c)%6];
    if(format===3){bits===32?v.setFloat32(at,value,true):v.setFloat64(at,value,true);}
    else if(bits===8)v.setUint8(at,Math.round(value*128+128));
    else if(bits===16)v.setInt16(at,value*32768,true);
    else if(bits===24){const n=value*8388608;v.setUint8(at,n&255);v.setUint8(at+1,n>>8&255);v.setUint8(at+2,n>>16&255);}
    else v.setInt32(at,value*2147483648,true);
  }
  return buffer;
}
for(const [format,bits] of [[1,8],[1,16],[1,24],[1,32],[3,32],[3,64]])for(const extended of [false,true]) {
  const buffer=fixture(format,bits,19,extended),audio=readFloatWav(buffer);
  assert.equal(audio.sampleRate,48000);assert.equal(audio.length,19);
  for(let c=0;c<2;c++)for(let i=0;i<19;i++)assert.equal(audio.getChannelData(c)[i],[-1,-.5,0,.25,.5,.9921875][(i+c)%6]);
  assert.throws(()=>readFloatWav(buffer.slice(0,-1)));
}
const directory=mkdtempSync(path.resolve('.tools/wav-direct-'));
const original=fixture(1,24,48000*180,true),input=path.join(directory,'generated.wav'),output=path.join(directory,'reference.wav');
writeFileSync(input,new Uint8Array(original));
const at=performance.now(),direct=readFloatWav(original),directMs=performance.now()-at;
const start=performance.now();
 execFileSync(process.env.FFMPEG || 'ffmpeg',['-v','error','-nostdin','-i',input,'-map','0:a:0','-c:a','pcm_f32le',output],{windowsHide:true});
const bytes=readFileSync(output);const reference=readFloatWav(bytes.buffer.slice(bytes.byteOffset,bytes.byteOffset+bytes.byteLength));
const convertedMs=performance.now()-start;
for(let c=0;c<2;c++)assert.deepEqual(direct.getChannelData(c),reference.getChannelData(c));
console.log(JSON.stringify({passed:true,fixture:'3 min / 48 kHz / stereo / PCM24 extensible',directParseMs:directMs,ffmpegAndParseMs:convertedMs,originalBytes:original.byteLength,floatBytes:bytes.byteLength,allSamplesEqual:true,nativeApp:false}));
