import { t } from "../lib/i18n";
import { useEffect, useRef } from 'react';
import { onThemeChange, paint, type PaintToken } from '../lib/theme';
import { barGeometry, waveRamps } from '../lib/waveStyle';
import type { Deck } from '../lib/types';
import { waveIndexes } from './waveIndex';

// Read PCM/float WAV directly: decodeAudioData would silently
// resample to the playback device's rate before the first edit.
export function readFloatWav(bytes: ArrayBuffer): AudioBuffer {
  const v = new DataView(bytes);
  let channels = 0, rate = 0, bits = 0, format = 0, align = 0, start = 0, size = 0;
  const tag = (p: number) => String.fromCharCode(...new Uint8Array(bytes, p, 4));
  if (v.byteLength < 44 || tag(0) !== 'RIFF' || tag(8) !== 'WAVE') throw Error('无效音频数据');
  for (let p = 12; p + 8 <= v.byteLength;) {
    const n = v.getUint32(p + 4, true);
    if (p + 8 + n > v.byteLength) throw Error('音频数据不完整');
    if (tag(p) === 'fmt ') {
      if (n < 16) throw Error('无效音频格式');
      format = v.getUint16(p + 8, true); channels = v.getUint16(p + 10, true);
      rate = v.getUint32(p + 12, true); bits = v.getUint16(p + 22, true);
      align = v.getUint16(p + 20, true);
      if (format === 65534 && n >= 40) format = v.getUint16(p + 32, true);
    }
    if (tag(p) === 'data') { start = p + 8; size = n; }
    p += 8 + n + (n % 2);
  }
  const supported = format === 1 ? [8,16,24,32].includes(bits) : format === 3 && [32,64].includes(bits);
  const width = bits / 8;
  if (!supported || channels < 1 || channels > 2 || !size || !rate || align !== width * channels || size % align) throw Error('无效音频格式');
  const length = size / align;
  if (length * channels * 4 > 256 * 1024 * 1024) throw Error('解码音频超过 256 MiB 编辑上限，请先缩短音频');
  const sample = format === 3
    ? (at: number) => bits === 32 ? v.getFloat32(at, true) : v.getFloat64(at, true)
    : bits === 8 ? (at: number) => (v.getUint8(at) - 128) / 128
    : bits === 16 ? (at: number) => v.getInt16(at, true) / 32768
    : bits === 24 ? (at: number) => ((v.getUint8(at) | v.getUint8(at+1) << 8 | v.getUint8(at+2) << 16) << 8 >> 8) / 8388608
    : (at: number) => v.getInt32(at, true) / 2147483648;
  const audio = new AudioBuffer({ numberOfChannels: channels, sampleRate: rate, length });
  for (let c = 0; c < channels; c++) {
    const data = audio.getChannelData(c);
    for (let i = 0; i < length; i++) { const value = sample(start + (i * channels + c) * width); data[i] = Number.isFinite(value) ? value : 0; }
  }
  return audio;
}

export function WaveCanvas({ buffer, channel, zoom, deck }: { buffer: AudioBuffer; channel: number; zoom: number; deck: Deck }) {
  const ref = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const canvas = ref.current!;
    const viewport = canvas.closest('.wave-viewport') as HTMLElement;
    const lane = canvas.parentElement!;
    const data = buffer.getChannelData(channel);
    const index = waveIndexes(buffer)[channel];
    let frame = 0;
    const draw = () => {
      const width = viewport.clientWidth, height = lane.clientHeight;
      if (!width || !height) return;
      const dpr = devicePixelRatio || 1;
      canvas.width = Math.ceil(width * dpr); canvas.height = Math.ceil(height * dpr);
      canvas.style.width = width + 'px'; canvas.style.height = height + 'px';
      const ctx = canvas.getContext('2d')!; ctx.scale(dpr, dpr);
      const p = paint();
      const ink: PaintToken = deck === 'b' ? '--lane-b-rgb' : '--lane-a-rgb';
      const { step, barW } = barGeometry(dpr, p);
      const mid = Math.round(height / 2) + .5, half = height / 2 - 2, px = 1 / dpr;
      ctx.strokeStyle = p.color('--wf-mid'); ctx.beginPath(); ctx.moveTo(0, mid); ctx.lineTo(width, mid); ctx.stroke();
      const totalWidth = viewport.clientWidth * zoom;
      const bars: { x: number; peak: number; rms: number }[] = [];
      // Every sample belongs to a bar, including samples under its visual gap.
      // Anchor bars to the document so scrolling cannot change their values.
      const first = Math.floor(viewport.scrollLeft / step);
      for (let n = first; n * step < viewport.scrollLeft + width; n++) {
        const a = Math.floor(n * step / totalWidth * data.length);
        const b = Math.min(data.length, Math.max(a + 1, Math.floor((n + 1) * step / totalWidth * data.length)));
        const { peak, rms } = index.range(a, b);
        if (b > a) bars.push({ x: Math.round((n * step - viewport.scrollLeft) * dpr) / dpr, peak, rms });
      }
      const { outer, core } = waveRamps(ctx, p, ink, width);
      ctx.fillStyle = outer;
      for (const bar of bars) { const a = Math.max(px, bar.peak * half); ctx.fillRect(bar.x, mid - a, barW, a * 2); }
      ctx.fillStyle = core;
      for (const bar of bars) { const a = Math.max(px, bar.rms * half * .98); ctx.fillRect(bar.x, mid - a, barW, a * 2); }
      ctx.globalCompositeOperation = p.blend(); ctx.fillStyle = p.tint(ink, p.num('--wf-cap-a'));
      for (const bar of bars) { const a = bar.peak * half; if (a < 2.5) continue; ctx.fillRect(bar.x, mid - a, barW, px); ctx.fillRect(bar.x, mid + a - px, barW, px); }
      ctx.globalCompositeOperation = 'source-over';
    };
    const schedule = () => { cancelAnimationFrame(frame); frame = requestAnimationFrame(draw); };
    const observer = new ResizeObserver(schedule); observer.observe(viewport); observer.observe(lane);
    const untheme = onThemeChange(schedule);
    viewport.addEventListener('scroll', schedule); schedule();
    return () => { cancelAnimationFrame(frame); observer.disconnect(); untheme(); viewport.removeEventListener('scroll', schedule); };
  }, [buffer, channel, zoom, deck]);
  return <canvas ref={ref} className="wave-canvas" aria-label={t(channel === 0 ? '左声道波形' : '右声道波形')} />;
}
