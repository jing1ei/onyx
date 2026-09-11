import { t } from '../lib/i18n';
import { fadeGain } from './fade';
import type { PointerEvent } from 'react';
export type FadeDraft = { start: number; end: number; direction: 'in' | 'out'; power: number };
export function FadeCurve({draft,duration,onChange,onDragStart,onDragEnd}:{draft:FadeDraft;duration:number;onChange:(d:FadeDraft)=>void;onDragStart:()=>void;onDragEnd:()=>void}) {
  const gain=(x:number)=>fadeGain(x,draft.direction,draft.power);
  const points=Array.from({length:65},(_,i)=>`${i/64*1000},${96-gain(i/64)*92}`).join(' ');
  function move(e:PointerEvent<HTMLButtonElement>,kind:'start'|'end'|'curve') {
    e.stopPropagation();
    if(!e.currentTarget.hasPointerCapture(e.pointerId))return;
    const wave=e.currentTarget.closest('.waveform')!.getBoundingClientRect();
    const time=Math.max(0,Math.min(duration,(e.clientX-wave.left)/wave.width*duration));
    const min=Math.min(.01,duration);
    if(kind==='start')onChange({...draft,start:Math.min(draft.end-min,time)});
    else if(kind==='end')onChange({...draft,end:Math.max(draft.start+min,time)});
    else {
      const g=Math.max(.01,Math.min(.99,1-(e.clientY-wave.top)/wave.height));
      const power=Math.log(draft.direction==='in'?g:1-g)/Math.log(.5);
      onChange({...draft,power:Math.max(.125,Math.min(8,power))});
    }
  }
  return <div className="fade-curve" style={{left:`${draft.start/duration*100}%`,width:`${(draft.end-draft.start)/duration*100}%`}}>
    <svg viewBox="0 0 1000 100" preserveAspectRatio="none" aria-hidden="true"><polyline points={points} fill="none" stroke="currentColor" strokeWidth="2" vectorEffect="non-scaling-stroke"/></svg>
    {(['start','end','curve'] as const).map(kind=><button key={kind} className={'fade-handle '+kind}
      style={kind==='curve'?{top:`${96-gain(.5)*92}%`}:undefined}
      aria-label={t(kind==='start'?'淡变起点':kind==='end'?'淡变终点':'淡变曲率')}
      onPointerDown={e=>{e.stopPropagation();onDragStart();e.currentTarget.setPointerCapture(e.pointerId);}}
      onPointerMove={e=>move(e,kind)} onPointerCancel={e=>{e.stopPropagation();onDragEnd();}}
      onPointerUp={e=>{e.stopPropagation();if(e.currentTarget.hasPointerCapture(e.pointerId))e.currentTarget.releasePointerCapture(e.pointerId);onDragEnd();}}/>)}
  </div>;
}
