import { t } from "../lib/i18n";
'use client';

import { readFloatWav, WaveCanvas } from './wave';
import { waveIndexes } from './waveIndex';
import { editorTiming } from './timing';
import { fadeSamples } from './fade';
import { FadeCurve, type FadeDraft } from './FadeCurve';
import * as api from '../lib/api';
import { useAudibleDeck } from '../lib/audible';
import { logWarn } from '../lib/log';
import { registerModeSwitcher, samePath } from '../lib/documentMode';
import Playlist from '../components/Playlist';
import { IconPlay, IconPause, IconLoop, IconPrev, IconNext } from '../components/Icons';
import { editorIo, MOCK } from '../lib/api';
import { useStore } from '../lib/store';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { PointerEvent, useCallback, useEffect, useMemo, useRef, useState } from 'react';

type Snapshot = { buffer: AudioBuffer; label: string };
// Bound undo memory as well as the number of steps for long recordings.
function boundedHistory(items: Snapshot[]) {
  let bytes = 0;
  const kept: Snapshot[] = [];
  for (const item of items.slice(-10).reverse()) {
    bytes += item.buffer.length * item.buffer.numberOfChannels * 4;
    if (bytes > 256 * 1024 * 1024) break;
    kept.unshift(item);
  }
  return kept;
}
function fmt(seconds: number, precise = false) {
  if (!Number.isFinite(seconds)) return precise ? '00:00.000' : '00:00';
  const mins = Math.floor(seconds / 60);
  const secs = seconds - mins * 60;
  return precise
    ? String(mins).padStart(2, '0') + ':' + secs.toFixed(3).padStart(6, '0')
    : String(mins).padStart(2, '0') + ':' + String(Math.floor(secs)).padStart(2, '0');
}

function cloneBuffer(ctx: AudioContext, source: AudioBuffer) {
  const next = ctx.createBuffer(source.numberOfChannels, source.length, source.sampleRate);
  for (let c = 0; c < source.numberOfChannels; c++) next.copyToChannel(source.getChannelData(c), c);
  return next;
}

function sliceBuffer(ctx: AudioContext, source: AudioBuffer, from: number, to: number) {
  const start = Math.max(0, Math.floor(from * source.sampleRate));
  const end = Math.min(source.length, Math.floor(to * source.sampleRate));
  const next = ctx.createBuffer(source.numberOfChannels, Math.max(1, end - start), source.sampleRate);
  for (let c = 0; c < source.numberOfChannels; c++) next.copyToChannel(source.getChannelData(c).slice(start, end), c);
  return next;
}

function removeSlice(ctx: AudioContext, source: AudioBuffer, from: number, to: number) {
  const start = Math.max(0, Math.floor(from * source.sampleRate));
  const end = Math.min(source.length, Math.floor(to * source.sampleRate));
  const next = ctx.createBuffer(source.numberOfChannels, Math.max(1, source.length - (end - start)), source.sampleRate);
  for (let c = 0; c < source.numberOfChannels; c++) {
    const data = source.getChannelData(c);
    const merged = new Float32Array(next.length);
    merged.set(data.subarray(0, start), 0);
    merged.set(data.subarray(end), start);
    next.copyToChannel(merged, c);
  }
  return next;
}

function encodeWav(buffer: AudioBuffer) {
  const channels = Math.min(buffer.numberOfChannels, 2);
  const bytesPerSample = 4;
  const dataLength = buffer.length * channels * bytesPerSample;
  const view = new DataView(new ArrayBuffer(44 + dataLength));
  const text = (offset: number, value: string) => [...value].forEach((char, i) => view.setUint8(offset + i, char.charCodeAt(0)));
  text(0, 'RIFF'); view.setUint32(4, 36 + dataLength, true); text(8, 'WAVE');
  text(12, 'fmt '); view.setUint32(16, 16, true); view.setUint16(20, 3, true);
  view.setUint16(22, channels, true); view.setUint32(24, buffer.sampleRate, true);
  view.setUint32(28, buffer.sampleRate * channels * bytesPerSample, true);
  view.setUint16(32, channels * bytesPerSample, true); view.setUint16(34, 32, true);
  text(36, 'data'); view.setUint32(40, dataLength, true);
  let offset = 44;
  const channelData = Array.from({length: channels}, (_, c) => buffer.getChannelData(c));
  for (let i = 0; i < buffer.length; i++) {
    for (let c = 0; c < channels; c++) {
      const sample = Math.max(-1, Math.min(1, channelData[c][i]));
      view.setFloat32(offset, sample, true);
      offset += 4;
    }
  }
  return new Blob([view], { type: 'audio/wav' });
}

// Let the browser encode the Blob without allocating a JS string per byte.
function wavPayload(buffer: AudioBuffer): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result).split(',', 2)[1]);
    reader.onerror = () => reject(reader.error || Error('无法读取保存数据'));
    reader.readAsDataURL(encodeWav(buffer));
  });
}

export default function Editor({ active = true }: { active?: boolean }) {
  const [buffer, setBuffer] = useState<AudioBuffer | null>(null);
  const [fileName, setFileName] = useState('未命名项目');
  const channelIds = useMemo(() => Array.from({length: buffer?.numberOfChannels ?? 0}, (_, c) => c), [buffer]);
  const [selection, setSelection] = useState<[number, number]>([0, 0]);
  const [current, setCurrent] = useState(0);
  const [playing, setPlaying] = useState(false);
  const [loop, setLoop] = useState(false);
  const [volume, setVolume] = useState(82);
  const [speed, setSpeed] = useState(1);
  const [zoom, setZoom] = useState(1);
  const [history, setHistory] = useState<Snapshot[]>([]);
  const [redo, setRedo] = useState<Snapshot[]>([]);
  const [notice, setNotice] = useState('导入一段音频，马上开始');
  const [dragging, setDragging] = useState(false);
  const [, setCanOverwrite] = useState(false);
  const [sourceExtension, setSourceExtension] = useState('');
  const [, setDesktopMode] = useState(true);
  const [saveToast, setSaveToast] = useState('');
  const audioCtx = useRef<AudioContext | null>(null);
  const sourceRef = useRef<AudioBufferSourceNode | null>(null);
  const playGeneration = useRef(0);
  const playPending = useRef(false);
  const fadePreview = useRef<{source:AudioBuffer; key:string; audio:AudioBuffer} | null>(null);
  const gainRef = useRef<GainNode | null>(null);
  const rafRef = useRef(0);
  const startedAt = useRef(0);
  const startedOffset = useRef(0);
  const dragStart = useRef<number | null>(null);
  const previewSeek = useRef<{resume:boolean} | null>(null);
  const curveDrag = useRef<{resume:boolean; time:number} | null>(null);
  const waveformRef = useRef<HTMLDivElement | null>(null);
  const waveViewportRef = useRef<HTMLDivElement | null>(null);

  const ioBusy = useRef(false);
  const [busy, setBusy] = useState(false);
  const savedBuffer = useRef<AudioBuffer | null>(null);
  const sourcePath = useRef('');
  const syncedBuffer = useRef<AudioBuffer | null>(null);
  const engineSnapshot = useStore(s => s.snapshot);
  const activeDeck = useAudibleDeck();
  const enginePath = (activeDeck === 'b' ? engineSnapshot?.deckB.info?.path : engineSnapshot?.deckA.info?.path) || '';
  const [fadeDraft, setFadeDraft] = useState<FadeDraft | null>(null);
  const dirty = !!buffer && (buffer !== savedBuffer.current || !!fadeDraft);
  const preload = useRef<{path:string; id?:string; cancelled:boolean; ready:Promise<{id:string; audio:AudioBuffer} | null>} | null>(null);

  // Decode just the audible source in the background. This never activates a
  // document, pauses playback, or replaces unsaved edits. Native cache checks
  // source bytes again at open; speculative failures are retried on demand.
  useEffect(() => {
    if (MOCK || active || dirty || !enginePath || samePath(enginePath, sourcePath.current)) return;
    let task: typeof preload.current = null;
    const timer = window.setTimeout(() => {
      // A foreground open during the debounce already prepares this source.
      if (ioBusy.current) return;
      const pending = {path:enginePath, cancelled:false, id:undefined as string | undefined, ready:Promise.resolve(null) as NonNullable<typeof preload.current>['ready']};
      task = pending;
      pending.ready = (async () => {
        const meta = await editorIo({action:'prepare',path:enginePath});
        pending.id = meta.preparedId;
        if (pending.cancelled || !meta.path || !meta.byteLength || !meta.preparedId) return null;
        const bytes = await api.editorRead(meta.path, meta.byteLength, meta.preparedId, () => pending.cancelled);
        const audio = readFloatWav(bytes);
        waveIndexes(audio);
        performance.clearMarks('onyx-editor-preloaded');
        performance.mark('onyx-editor-preloaded', {detail:{path:enginePath}});
        return {id:meta.preparedId,audio};
      })().catch(() => null);
      preload.current = pending;
    }, 300);
    return () => { window.clearTimeout(timer); if (task) task.cancelled = true; };
  }, [enginePath, active, dirty]);

  useEffect(() => {
    const unload = (e: BeforeUnloadEvent) => { if (dirty || ioBusy.current) { e.preventDefault(); e.returnValue = ''; } };
    if (MOCK) return;
    window.addEventListener('beforeunload', unload);
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void getCurrentWindow().onCloseRequested(async e => {
      if (dirty || ioBusy.current) {
        e.preventDefault();
        useStore.getState().setEditorActive(true);
        setNotice(ioBusy.current ? '正在读写文件，请稍候再关闭' : '有未保存的修改，请先保存，或点击“放弃修改”');
      }
    }).then(un => { if (disposed) un(); else unlisten = un; });
    return () => { disposed = true; unlisten?.(); window.removeEventListener('beforeunload', unload); };
  }, [dirty]);

  async function nativeOpen(path?: string, attach = true): Promise<boolean> {
    if (ioBusy.current) return false;
    if (!attach && path && samePath(path, sourcePath.current) && buffer) return true;
    if (dirty && !window.confirm(t('当前修改未保存，放弃修改并打开另一个文件？'))) return false;
    ioBusy.current = true; setBusy(true); setSaveToast(''); setNotice('正在读取音频…');
    const timing = editorTiming('open');
    stop(false);
    try {
      await api.transportPause();
      timing.step('pause');
      const result = await editorIo({ action: 'open', path });
      timing.step('nativeOpen');
      if (!result.canceled && result.name && result.path && result.byteLength) {
        const pending = preload.current;
        const matching = pending && !pending.cancelled && pending.id === result.preparedId && samePath(pending.path, result.path);
        if (pending && !matching) pending.cancelled = true;
        const ready = matching ? await pending.ready : null;
        timing.step('preloadWait');
        const decoded = ready && ready.id === result.preparedId ? ready.audio : readFloatWav(await api.editorRead(result.path, result.byteLength));
        timing.step('transferAndParse');
        stop(); fadePreview.current=null; setFadeDraft(null); setBuffer(decoded); setSelection([0, 0]);
        savedBuffer.current = decoded;
        sourcePath.current = result.path || path || '';
        syncedBuffer.current = null;
        setZoom(1); setCurrent(0); setHistory([]); setRedo([]);
        setFileName(result.name.replace(/\.[^.]+$/, ''));
        setSourceExtension(result.name.split('.').pop()?.toLowerCase() || 'wav');
        setDesktopMode(true); setCanOverwrite(true);
        setNotice('已打开完整音频 · 拖选片段 · R/T 缩放 · Ctrl+S 覆盖保存');
        if (attach) { await editorIo({ action: 'attach' }); useStore.getState().setSnapshot(await api.appState()); }
        timing.step('attach');
        syncedBuffer.current = decoded;
        return true;
      }
    } catch (e) {
      const why = api.errorMessage(e);
      setNotice(why);
      useStore.getState().setModeError(why);
      logWarn('Editor audio load failed', e);
    }
    finally { timing.finish(); ioBusy.current = false; setBusy(false); }
    return false;
  }

  async function syncAudition(payload?: string) {
    if (!buffer || syncedBuffer.current === buffer) return;
    await editorIo({ action: 'preview', bytes: payload ?? await wavPayload(buffer) });
    // Native decoding is asynchronous. Do not reveal an old transport frame
    // while the replacement PCM is still being decoded/resampled.
    const deadline = Date.now() + 30000;
    let snap = await api.appState();
    while (true) {
      const decks = [snap.deckA, snap.deckB].filter(d => d.info && samePath(d.info.path, sourcePath.current));
      if (decks.length && decks.every(d => d.decoded && !d.error)) break;
      const error = decks.find(d => d.error)?.error;
      if (error || Date.now() >= deadline) throw new Error(error || t('试听准备超时，请重试'));
      await new Promise(resolve => setTimeout(resolve, 40));
      snap = await api.appState();
    }
    syncedBuffer.current = buffer;
    useStore.getState().resetWaveform('a', null);
    useStore.getState().resetWaveform('b', null);
    useStore.getState().setSnapshot(snap);
  }
  useEffect(() => { useStore.getState().setEditorDocument(sourcePath.current, dirty); }, [buffer, dirty, busy]);
  useEffect(() => registerModeSwitcher(async editing => {
    if (fadeDraft) throw new Error('请先应用或取消淡变曲线');
    if (ioBusy.current) throw new Error('正在读写音频，请稍候再切换');
    const timing = editorTiming(editing ? 'enter-edit' : 'leave-edit');
    try {
      if (editing) {
        const snap = await api.appState();
        timing.step('snapshot');
        // Use the engine snapshot from this request, not a possibly older
        // animation frame after a rapid A/B selection.
        if (snap.blind.active) throw new Error('请先结束盲测，再进入剪辑');
        const path = (snap.transport.activeDeck === 'b' ? snap.deckB : snap.deckA).info?.path;
        if (snap.ab.enabled && !path) throw new Error('当前监听的音轨没有音频，请先选择已载入音频的 A 或 B');
        if (!MOCK && path && !await nativeOpen(path, false)) return;
        timing.step('document');
        await api.editorPlayback(true);
        timing.step('playbackOwner');
      } else {
        ioBusy.current = true; setBusy(true); stop(false);
        await syncAudition();
        timing.step('syncAudition');
        await api.editorPlayback(false);
      }
      useStore.getState().setEditorActive(editing);
    } catch(e) { setNotice(api.errorMessage(e)); throw e; }
    finally { timing.finish(); ioBusy.current = false; setBusy(false); }
  }));
  useEffect(() => {
    if (MOCK || !active || busy || !enginePath || samePath(enginePath, sourcePath.current)) return;
    void nativeOpen(enginePath, false).then(async ok => {
      if (!ok && sourcePath.current) { syncedBuffer.current = null; await syncAudition(); }
    }).catch(e => setNotice(api.errorMessage(e)));
  }, [enginePath, active]);

  useEffect(() => {
    let disposed = false; let off: (() => void) | undefined;
    if (MOCK) return;
    void getCurrentWebview().onDragDropEvent(({ payload }) => {
      if (!active) return;
      if (payload.type === 'leave') { setDragging(false); return; }
      const pos = 'position' in payload ? payload.position : null;
      const element = pos ? document.elementFromPoint(pos.x / devicePixelRatio, pos.y / devicePixelRatio) : null;
      const over = !!element?.closest('[data-audio-dropzone]');
      setDragging(payload.type === 'over' && over);
      if (payload.type === 'drop' && over && payload.paths[0]) void nativeOpen(payload.paths[0]);
    }).then(un => { if (disposed) un(); else off = un; }).catch(e => setNotice(String(e)));
    return () => { disposed = true; off?.(); };
  }, [dirty, active]);

  const duration = buffer?.duration ?? 0;
  const selStart = Math.min(...selection);
  const selEnd = Math.max(...selection);
  const hasSelection = !!buffer && selEnd - selStart > 0.01;
  const channels = buffer ? (buffer.numberOfChannels === 1 ? '单声道' : '立体声') : '—';
  const sampleRate = buffer ? (buffer.sampleRate / 1000).toFixed(1) + ' kHz' : '—';

  const context = useCallback(() => {
    if (!audioCtx.current) audioCtx.current = new AudioContext();
    return audioCtx.current;
  }, []);

  const stop = useCallback((reset = true) => {
    playGeneration.current++;
    playPending.current = false;
    cancelAnimationFrame(rafRef.current);
    if (sourceRef.current) {
      try { sourceRef.current.stop(); } catch {}
      sourceRef.current.disconnect();
      sourceRef.current = null;
    }
    setPlaying(false);
    if (reset) setCurrent(0);
  }, []);

  async function pickFile() { await nativeOpen(); }

  async function nativeSave(mode: 'overwrite' | 'saveAs') {
    if (fadeDraft) { setNotice('请先应用或取消淡变曲线'); return; }
    if (!buffer || ioBusy.current) return;
    ioBusy.current = true; setBusy(true); setSaveToast(''); setNotice('正在保存，请稍候…');
    stop(false);
    try {
      const payload = await wavPayload(buffer);
      const result = await editorIo({ action: mode, bytes: payload, suggestedName: fileName + '.' + sourceExtension });
      if (!result.canceled) {
        savedBuffer.current = buffer;
        const sameSource = !result.path || samePath(result.path, sourcePath.current);
        if (result.path) sourcePath.current = result.path;
        if (!sameSource) syncedBuffer.current = null;
        if (result.name) { setFileName(result.name.replace(/\.[^.]+$/, '')); setSourceExtension(result.name.split('.').pop() || 'wav'); }
        showSaveSuccess('保存成功：' + (result.name || fileName));
        try { await syncAudition(payload); } catch(e) { setNotice(t('文件已保存，但试听同步失败：') + api.errorMessage(e)); }
      } else setNotice('已取消保存');
    } catch (e) { setNotice('保存失败：' + (e instanceof Error ? e.message : String(e))); }
    finally { ioBusy.current = false; setBusy(false); }
  }

  function showSaveSuccess(message: string) {
    setNotice(message);
    setSaveToast(message);
    window.setTimeout(() => setSaveToast(''), 4200);
  }

  const pushEdit = useCallback((label: string, next: AudioBuffer) => {
    if (!buffer || ioBusy.current) return;
    setHistory(items => boundedHistory([...items, { buffer, label }]));
    setRedo([]); setBuffer(next);
    setSelection([0, 0]); setCurrent(0); setZoom(1); stop(); setNotice(label);
    window.requestAnimationFrame(() => { if (waveViewportRef.current) waveViewportRef.current.scrollLeft = 0; });
  }, [buffer, stop]);

  const playAt = useCallback(async (seekTo?: number) => {
    if (!useStore.getState().editorActive || ioBusy.current) return;
    if (!buffer) { void nativeOpen(); return; }
    if (seekTo === undefined && (sourceRef.current || playPending.current)) { stop(false); return; }
    stop(false);
    if (seekTo !== undefined && seekTo >= duration) { setCurrent(duration); return; }
    const generation = playGeneration.current;
    playPending.current = true;
    try {
      await api.editorPlayback(true);
      if (generation !== playGeneration.current || ioBusy.current || !useStore.getState().editorActive) return;
    const ctx = context();
    const source = ctx.createBufferSource();
    const gain = ctx.createGain();
    source.buffer = fadeDraft ? renderFade(fadeDraft) : buffer; source.playbackRate.value = speed;
    gain.gain.value = volume / 100;
    source.connect(gain).connect(ctx.destination);
    gainRef.current = gain;
    // Fade range controls gain, not the audition boundaries: allow hearing
    // before/after the fade and seeking anywhere without changing its range.
    const selectionOnly = hasSelection && !fadeDraft;
    const end = selectionOnly ? selEnd : duration;
    const rangeStart = selectionOnly ? selStart : 0;
    let from = seekTo ?? current;
    if (from >= end - 0.01 || from < rangeStart) from = rangeStart;
    source.loop = loop;
    if (loop) { source.loopStart = rangeStart; source.loopEnd = end; }
    startedAt.current = ctx.currentTime; startedOffset.current = from; sourceRef.current = source;
    source.start(0, from, loop ? undefined : Math.max(0.01, end - from));
    setCurrent(from); setPlaying(true);
    const tick = () => {
      const elapsed = (ctx.currentTime - startedAt.current) * speed;
      let next = startedOffset.current + elapsed;
      if (loop && next >= end) next = rangeStart + ((next - rangeStart) % (end - rangeStart));
      setCurrent(Math.min(next, end));
      rafRef.current = requestAnimationFrame(tick);
    };
    rafRef.current = requestAnimationFrame(tick);
    source.onended = () => {
      if (sourceRef.current === source && !source.loop) {
        cancelAnimationFrame(rafRef.current); sourceRef.current = null;
        setPlaying(false); setCurrent(end);
      }
    };
    } catch (e) {
      if (generation === playGeneration.current) { stop(false); setNotice(api.errorMessage(e)); }
    } finally {
      if (generation === playGeneration.current) playPending.current = false;
    }
  }, [buffer, context, current, duration, hasSelection, loop, playing, selEnd, selStart, speed, stop, volume, fadeDraft]);
  const play = useCallback(() => playAt(), [playAt]);
  function beginCurveDrag() { curveDrag.current = {resume:!!sourceRef.current || playPending.current,time:current}; stop(false); }
  function endCurveDrag() { const drag=curveDrag.current; curveDrag.current=null; if(drag?.resume)playAt(drag.time); }

  const trim = useCallback(() => {
    if (buffer && hasSelection) pushEdit('已裁剪到所选片段', sliceBuffer(context(), buffer, selStart, selEnd));
  }, [buffer, context, hasSelection, pushEdit, selEnd, selStart]);

  const remove = useCallback(() => {
    if (buffer && hasSelection && selEnd - selStart < duration - 0.01) pushEdit('已删除所选片段', removeSlice(context(), buffer, selStart, selEnd));
  }, [buffer, context, duration, hasSelection, pushEdit, selEnd, selStart]);

  function normalize() {
    if (!buffer) return;
    const next = cloneBuffer(context(), buffer);
    let peak = 0;
    for (let c = 0; c < next.numberOfChannels; c++) for (const sample of next.getChannelData(c)) peak = Math.max(peak, Math.abs(sample));
    if (peak > 0) for (let c = 0; c < next.numberOfChannels; c++) {
      const data = next.getChannelData(c);
      for (let i = 0; i < data.length; i++) data[i] *= 0.96 / peak;
    }
    pushEdit('音量已标准化至 -0.4 dB 峰值', next);
  }

  function fade(direction: 'in' | 'out') {
    if (!buffer || !hasSelection) return;
    stop(false);
    setFadeDraft({start:selStart,end:selEnd,direction,power:1});
    setNotice('拖动两端调整范围，拖动中点调整曲线；空格试听，满意后应用');
  }
  function renderFade(draft: FadeDraft) {
    if (!buffer) throw new Error('请先打开音频');
    const key = `${draft.start}:${draft.end}:${draft.direction}:${draft.power}`;
    const cached = fadePreview.current;
    if (cached?.source === buffer && cached.key === key) return cached.audio;
    const next = cloneBuffer(context(), buffer);
    const start = Math.floor(draft.start * next.sampleRate);
    const end = Math.floor(draft.end * next.sampleRate);
    for (let c = 0; c < next.numberOfChannels; c++) {
      fadeSamples(next.getChannelData(c), start, end, draft.direction, draft.power);
    }
    fadePreview.current = {source:buffer,key,audio:next};
    return next;
  }
  function applyFade() {
    if (!fadeDraft) return;
    const next = renderFade(fadeDraft);
    pushEdit(fadeDraft.direction === 'in' ? '已对选区应用淡入' : '已对选区应用淡出', next);
    fadePreview.current=null;
    setFadeDraft(null);
  }

  const undo = useCallback(() => {
    if (!buffer || !history.length) return;
    const previous = history[history.length - 1];
    setRedo(items => boundedHistory([...items].reverse().concat({ buffer, label: previous.label })).reverse());
    setHistory(items => items.slice(0, -1));
    setBuffer(previous.buffer);
    setSelection([0, 0]); setCurrent(0); setZoom(1); stop(); setNotice('已撤销');
    window.requestAnimationFrame(() => { if (waveViewportRef.current) waveViewportRef.current.scrollLeft = 0; });
  }, [buffer, history, stop]);

  const redoEdit = useCallback(() => {
    if (!buffer || !redo.length) return;
    const next = redo[0];
    setHistory(items => boundedHistory([...items, { buffer, label: next.label }]));
    setRedo(items => items.slice(1));
    setBuffer(next.buffer);
    setSelection([0, 0]); setCurrent(0); setZoom(1); stop(); setNotice('已重做');
    window.requestAnimationFrame(() => { if (waveViewportRef.current) waveViewportRef.current.scrollLeft = 0; });
  }, [buffer, redo, stop]);


  async function saveAs() { await nativeSave('saveAs'); }
  async function overwriteOriginal() { await nativeSave('overwrite'); }
  function pointerTime(event: PointerEvent<HTMLDivElement>) {
    const rect = waveformRef.current!.getBoundingClientRect();
    return Math.max(0, Math.min(duration, ((event.clientX - rect.left) / rect.width) * duration));
  }
  function pointerDown(event: PointerEvent<HTMLDivElement>) {
    if (!buffer) return;
    if (fadeDraft) {
      previewSeek.current = {resume:!!sourceRef.current || playPending.current};
      stop(false); event.currentTarget.setPointerCapture(event.pointerId);
      setCurrent(pointerTime(event)); return;
    }
    stop(false); event.currentTarget.setPointerCapture(event.pointerId);
    const time = pointerTime(event); dragStart.current = time;
    setSelection([time, time]); setCurrent(time);
  }
  function pointerMove(event: PointerEvent<HTMLDivElement>) {
    if (previewSeek.current) { setCurrent(pointerTime(event)); return; }
    if (dragStart.current !== null) setSelection([dragStart.current, pointerTime(event)]);
  }
  function pointerUp(event: PointerEvent<HTMLDivElement>) {
    if (previewSeek.current) {
      const seek = previewSeek.current; previewSeek.current=null;
      const time=pointerTime(event);setCurrent(time);
      if(event.currentTarget.hasPointerCapture(event.pointerId))event.currentTarget.releasePointerCapture(event.pointerId);
      if(seek.resume)playAt(time);
      return;
    }
    if (dragStart.current === null) return;
    const time = pointerTime(event);
    if (Math.abs(time - dragStart.current) < duration / zoom * 0.004) setSelection([0, 0]);
    dragStart.current = null;
  }

  function applyHorizontalZoom(nextZoom: number, anchorTime: number, align = 0.5) {
    const clamped = Math.max(1, Math.min(32, nextZoom));
    setZoom(clamped);
    window.requestAnimationFrame(() => window.requestAnimationFrame(() => {
      const viewport = waveViewportRef.current;
      if (!viewport || !duration) return;
      const maxScroll = Math.max(0, viewport.scrollWidth - viewport.clientWidth);
      const target = (anchorTime / duration) * viewport.scrollWidth - viewport.clientWidth * align;
      viewport.scrollLeft = Math.max(0, Math.min(maxScroll, target));
    }));
  }

  function changeHorizontalZoom(direction: 1 | -1) {
    const next = direction > 0 ? zoom * 1.5 : zoom / 1.5;
    const anchor = hasSelection ? (selStart + selEnd) / 2 : current;
    applyHorizontalZoom(next, anchor);
  }

  function zoomToSelection() {
    if (!hasSelection || !duration) return;
    const selectionDuration = selEnd - selStart;
    const next = Math.max(1, Math.min(32, (duration / selectionDuration) * 0.9));
    applyHorizontalZoom(next, selStart, 0.05);
  }

  function viewEntireAudio() {
    setZoom(1);
    window.requestAnimationFrame(() => {
      if (waveViewportRef.current) waveViewportRef.current.scrollLeft = 0;
    });
  }

  function handleZoomWheel(event: WheelEvent) {
    if (!event.ctrlKey || !duration) return;
    event.preventDefault();
    const viewport = waveViewportRef.current;
    if (!viewport) return;
    const rect = viewport.getBoundingClientRect();
    const pointerX = Math.max(0, Math.min(viewport.clientWidth, event.clientX - rect.left));
    const anchorTime = ((viewport.scrollLeft + pointerX) / viewport.scrollWidth) * duration;
    applyHorizontalZoom(zoom * (event.deltaY < 0 ? 1.18 : 1 / 1.18), anchorTime, pointerX / viewport.clientWidth);
  }

  useEffect(() => {
    const viewport = waveViewportRef.current;
    if (!viewport) return;
    viewport.addEventListener('wheel', handleZoomWheel, { passive: false });
    return () => viewport.removeEventListener('wheel', handleZoomWheel);
  }, [zoom, duration]);

  useEffect(() => { if (gainRef.current) gainRef.current.gain.value = volume / 100; }, [volume]);
  useEffect(() => {
    if (!buffer) return;
    window.requestAnimationFrame(() => window.requestAnimationFrame(() => {
      if (waveViewportRef.current) waveViewportRef.current.scrollLeft = 0;
    }));
  }, [buffer]);
  useEffect(() => {
    setDesktopMode(true);

  }, [buffer, fileName, sourceExtension]);
  useEffect(() => {
    const keys = (event: KeyboardEvent) => {
      if (!active || useStore.getState().settingsOpen || useStore.getState().shortcutsOpen) return;
      if (event.isComposing || ['INPUT', 'SELECT', 'TEXTAREA'].includes((event.target as HTMLElement).tagName)) return;
      const key = event.key.toLowerCase();
      if (fadeDraft && (event.ctrlKey || event.metaKey || ['Delete','Backspace'].includes(event.key))) { event.preventDefault(); setNotice('请先应用或取消淡变曲线'); return; }
      if (buffer && !event.ctrlKey && !event.metaKey && !event.altKey && key === 'r') { event.preventDefault(); changeHorizontalZoom(-1); }
      if (buffer && !event.ctrlKey && !event.metaKey && !event.altKey && key === 't') { event.preventDefault(); changeHorizontalZoom(1); }
      if ((event.ctrlKey || event.metaKey) && key === 's') { event.preventDefault(); void overwriteOriginal(); return; }
      if ((event.ctrlKey || event.metaKey) && key === 'o') { event.preventDefault(); void pickFile(); return; }
      if (ioBusy.current) return;
      if (event.code === 'Space') { event.preventDefault(); if (!event.repeat) void play(); }
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'z') { event.preventDefault(); event.shiftKey ? redoEdit() : undo(); }
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 't') { event.preventDefault(); trim(); }
      if (event.key === 'Delete' || event.key === 'Backspace') { event.preventDefault(); remove(); }
    };
    window.addEventListener('keydown', keys);
    return () => window.removeEventListener('keydown', keys);
  }, [active, buffer, current, hasSelection, play, redoEdit, remove, selEnd, selStart, trim, undo, zoom, fadeDraft]);
  useEffect(() => { if (!active) stop(false); }, [active, stop]);
  useEffect(() => {
    if (!active) return;
    void api.editorPlayback(true).catch(e => {
      stop(false); setNotice(api.errorMessage(e));
      useStore.getState().setModeError(api.errorMessage(e));
    });
    return () => { void api.editorPlayback(false).catch(e => logWarn('Editor playback release failed', e)); };
  }, [active, stop]);
  useEffect(() => () => { stop(false); audioCtx.current?.close(); }, [stop]);

  const ruler = useMemo(() => {
    const d = duration || 60;
    const count = Math.max(5, Math.ceil(zoom * 4) + 1);
    const interval = d / (count - 1);
    return Array.from({ length: count }, (_, i) => fmt(interval * i, interval < 10));
  }, [duration, zoom]);

  return (
    <main className={'editor-page unified-editor' + (busy ? ' io-busy' : '') + (fadeDraft ? ' fading' : '')}>
      <section className="edit-wave-stack">
        <div className="lane-head">
          <span className="deck-badge">{t(activeDeck.toUpperCase())}</span>
          <span className="lane-name">{buffer ? fileName : t('未载入音频')}</span>
          <span className="edit-state">{t(dirty ? '未保存' : '')}</span>
          <span className="edit-format">{t(channels)} · {t(sampleRate)}</span>
        </div>
        <div className={'timeline-card ' + (!buffer ? 'empty ' : '') + (dragging ? 'dragging' : '')}>
          <div ref={waveViewportRef} data-audio-dropzone className="wave-viewport" onDragOver={e => { e.preventDefault(); setDragging(true); }} onDragLeave={() => setDragging(false)} onDrop={e => e.preventDefault()}>
            <div className="wave-content" style={{ width: (zoom * 100) + '%' }}>
              <div className="ruler">{t(ruler.map((time, i) => <span key={i}>{t(time)}</span>))}</div>
              <div ref={waveformRef} className="waveform" onPointerDown={pointerDown} onPointerMove={pointerMove} onPointerUp={pointerUp} onPointerCancel={() => { previewSeek.current=null; dragStart.current=null; }} aria-label={t("音频波形，拖动以选择片段")}>
                <div className={'channel-stack ' + (channelIds.length > 1 ? 'stereo' : 'mono')}>
                  {t(buffer ? channelIds.map(channelIndex => (
                    <div className="wave-channel" key={channelIndex}>
                      <span className="channel-badge">{t(channelIds.length > 1 ? (channelIndex === 0 ? 'L' : 'R') : 'M')}</span>
                      <WaveCanvas buffer={buffer} channel={channelIndex} zoom={zoom} deck={activeDeck} />
                    </div>
                  )) : <button className="edit-empty" onClick={pickFile}>{t("把音频拖到这里，或点击打开")}</button>)}
                </div>
                {t(buffer && hasSelection && <div className="selection" style={{ left: ((selStart / duration) * 100) + '%', width: (((selEnd - selStart) / duration) * 100) + '%' }} />)}
                {t(buffer && <div className="playhead" style={{ left: ((current / duration) * 100) + '%' }} />)}
                {fadeDraft && <FadeCurve draft={fadeDraft} duration={duration} onDragStart={beginCurveDrag} onDragEnd={endCurveDrag} onChange={draft=>{stop(false);setFadeDraft(draft);setSelection([draft.start,draft.end]);}}/>}
              </div>
            </div>
          </div>
        </div>
        <div className="timeline-toolbar">
          {fadeDraft && <div className="fade-tools">
            <span>{t(fadeDraft.direction==='in'?'淡入':'淡出')} · {(fadeDraft.end-fadeDraft.start).toFixed(2)} s</span>
            <label>{t('曲率')} <input aria-label={t('曲率')} type="range" min="-3" max="3" step=".05" value={Math.log2(fadeDraft.power)} onPointerDown={beginCurveDrag} onPointerUp={endCurveDrag} onPointerCancel={endCurveDrag} onChange={e=>{stop(false);setFadeDraft({...fadeDraft,power:2**Number(e.target.value)});}}/></label>
            <button onClick={()=>{stop(false);setFadeDraft({...fadeDraft,power:1});}}>{t('线性')}</button>
            <button onClick={play}>{t(playing?'停止试听':'试听曲线')}</button>
            <button onClick={applyFade}>{t('应用曲线')}</button>
            <button onClick={()=>{stop(false);fadePreview.current=null;setFadeDraft(null);setNotice('已取消曲线，音频未改变');}}>{t('取消曲线')}</button>
          </div>}
          <span className="zoom-label">{t("缩放")}</span>
          <button onClick={() => changeHorizontalZoom(-1)} disabled={zoom <= 1} aria-label={t("横向缩小")} title={t("R")}>−</button>
          <input className="zoom-slider" type="range" min="1" max="32" step="0.25" value={zoom} onChange={event => applyHorizontalZoom(Number(event.target.value), hasSelection ? (selStart + selEnd) / 2 : current)} aria-label={t("横向缩放倍率")} />
          <span className="zoom-readout">{t(zoom.toFixed(1))}×</span>
          <button onClick={() => changeHorizontalZoom(1)} disabled={zoom >= 32} aria-label={t("横向放大")} title={t("T")}>＋</button>
          <button onClick={zoomToSelection} disabled={!hasSelection}>{t("放大所选")}</button>
          <button onClick={viewEntireAudio} disabled={!buffer || zoom === 1}>{t("适配全长")}</button>
          <span className="edit-selection num">{t(fmt(selStart, true))} → {t(fmt(selEnd, true))} <span>({t((selEnd - selStart).toFixed(3))}{t(" s)")}</span></span>
        </div>
      </section>
      <div className="edit-toolbar" inert={fadeDraft ? true : undefined} role="toolbar" aria-label={t("编辑工具")}>
        <button disabled={!history.length || busy} onClick={undo} title={t("Ctrl+Z")}>{t("撤销")}</button>
        <button disabled={!redo.length || busy} onClick={redoEdit} title={t("Ctrl+Shift+Z")}>{t("重做")}</button>
        <span className="toolbar-divider" />
        <button disabled={!hasSelection || busy} onClick={trim} title={t("Ctrl+T")}>{t("裁剪到所选")}</button>
        <button disabled={!hasSelection || selEnd - selStart >= duration - .01 || busy} onClick={remove} title={t("Delete")}>{t("删除所选")}</button>
        <button disabled={!hasSelection || busy} onClick={() => fade('in')} title={t('选区从静音渐强至原音量，选区长度即淡入时长')}>{t("淡入")}</button>
        <button disabled={!hasSelection || busy} onClick={() => fade('out')} title={t('选区从原音量渐弱至静音，选区长度即淡出时长')}>{t("淡出")}</button>
        <button disabled={!buffer || busy} onClick={normalize}>{t("标准化音量")}</button>
        <div className="save-actions">
{t(dirty && <button disabled={busy} onClick={() => { if (window.confirm(t('放弃未保存修改？'))) { stop(); setBuffer(savedBuffer.current); setHistory([]); setRedo([]); setSelection([0, 0]); viewEntireAudio(); setNotice('已恢复到上次保存'); } }}>{t("放弃修改")}</button>)}
          <button className="overwrite-button" disabled={!buffer || busy} onClick={overwriteOriginal} title={t("Ctrl+S")}>{t("覆盖原文件")}</button>
          <button disabled={!buffer || busy} onClick={saveAs}>{t("另存为")}</button>
        </div>
      </div>
      <div className="edit-playlist"><Playlist onOpen={path => { if (!fadeDraft) void nativeOpen(path); else setNotice('请先应用或取消淡变曲线'); }} /></div>
      <div className="edit-status status-copy" role="status">{t(notice)}</div>
      <footer className="transport edit-transport">
        <div className="tr-buttons">
          <button className="tr-btn" aria-label={t("回到开头")} onClick={() => stop()}><IconPrev size={12} /></button>
          <button className="tr-btn primary play" aria-label={t(playing ? '暂停' : '播放')} onClick={play}>{t(playing ? <IconPause size={14} /> : <IconPlay size={14} />)}</button>
          <button className="tr-btn" aria-label={t("前进五秒")} onClick={() => { stop(false); setCurrent(v => Math.min(duration, v + 5)); }}><IconNext size={12} /></button>
        </div>
        <div className="timecode tr-time num">{t(fmt(current, true))} <span>/ {t(fmt(duration, true))}</span></div>
        <button className="tr-toggle" data-on={loop} aria-label={t("循环播放")} onClick={() => setLoop(v => !v)}><IconLoop size={12} /><span>{t("循环")}</span></button>
        <div className="speed-row"><label htmlFor="speed">{t("速度")}</label><select id="speed" value={speed} onChange={e => setSpeed(+e.target.value)}><option value=".75">0.75×</option><option value="1">1.00×</option><option value="1.25">1.25×</option><option value="1.5">1.50×</option><option value="2">2.00×</option></select></div>
        <div className="tr-spacer" />
        <div className="volume"><span>{t("音量")}</span><input aria-label={t("音量")} type="range" value={volume} onChange={e => setVolume(+e.target.value)} /></div>
      </footer>
      {t(saveToast && <div className="save-toast" role="status" aria-live="assertive"><strong>{t("保存成功")}</strong><small>{t(saveToast)}</small></div>)}
    </main>
  );
}
