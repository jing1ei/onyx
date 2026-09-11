// Generated files only; no Onyx, installer, playback, or audio devices.
import {mkdtempSync,mkdirSync,writeFileSync} from 'node:fs';
import {resolve,join} from 'node:path';
import {execFileSync} from 'node:child_process';
import assert from 'node:assert/strict';
const bin=resolve(process.argv[2] || 'vendor/ffmpeg/bin');
mkdirSync('.tools',{recursive:true});
const work=mkdtempSync(resolve('.tools/bundled-codecs-'));
const env={...process.env,PATH:join(process.env.SystemRoot || 'C:/Windows','System32')};
const run=(name,args)=>execFileSync(join(bin,name+'.exe'),args,{env,windowsHide:true,encoding:'utf8'});
const rate=48000,frames=12000,bytes=Buffer.alloc(44+frames*6);
bytes.write('RIFF');bytes.writeUInt32LE(bytes.length-8,4);bytes.write('WAVEfmt ',8);
bytes.writeUInt32LE(16,16);bytes.writeUInt16LE(1,20);bytes.writeUInt16LE(2,22);
bytes.writeUInt32LE(rate,24);bytes.writeUInt32LE(rate*6,28);bytes.writeUInt16LE(6,32);bytes.writeUInt16LE(24,34);
bytes.write('data',36);bytes.writeUInt32LE(frames*6,40);
for(let i=0;i<frames;i++){const value=Math.round(Math.sin(i*440*2*Math.PI/rate)*2000000);bytes.writeIntLE(value,44+i*6,3);bytes.writeIntLE(-value,47+i*6,3);}
const source=join(work,'input.wav');writeFileSync(source,bytes);
assert.match(run('ffmpeg',['-version']),/ffmpeg version 8\.0/);
assert.match(run('ffprobe',['-version']),/ffprobe version 8\.0/);
const cases=[['wav','pcm_s24le'],['mp3','libmp3lame'],['flac','flac'],['m4a','aac'],['aac','aac'],['ogg','libvorbis'],['opus','libopus'],['aiff','pcm_s24be']];
for(const [ext,codec] of cases){
  const file=join(work,'output.'+ext);
  run('ffmpeg',['-v','error','-nostdin','-i',source,'-c:a',codec,file]);
  run('ffmpeg',['-v','error','-xerror','-nostdin','-i',file,'-f','null','-']);
  const info=JSON.parse(run('ffprobe',['-v','error','-show_streams','-of','json',file])).streams.find(s=>s.codec_type==='audio');
  assert.equal(info.channels,2);assert.equal(Number(info.sample_rate),rate);
  if(ext==='wav')assert.equal(info.codec_name,'pcm_s24le');
}
console.log('PASS: bundled FFmpeg/FFprobe, 8 export formats, full decode validation, clean PATH, no playback.');
