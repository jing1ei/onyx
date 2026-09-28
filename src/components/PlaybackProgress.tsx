import { useRef } from "react";
import * as api from "../lib/api";
import { setAttr } from "../lib/dom";
import { formatTime } from "../lib/format";
import { useFrameEffect } from "../lib/frame";
import { isComposingReact } from "../lib/ime";
import { useStore } from "../lib/store";

/** A shared seek control that follows the engine's playback timeline. */
export default function PlaybackProgress() {
  const slider = useRef<HTMLInputElement>(null);
  const dragging = useRef(false);
  const pushToast = useStore((s) => s.pushToast);

  useFrameEffect((frame) => {
    const el = slider.current;
    const transport = frame?.transport;
    if (!el || !transport) return;
    const duration = Math.max(0, transport.durationSecs);
    el.disabled = duration === 0;
    el.max = String(duration);
    if (!dragging.current) el.value = String(Math.min(duration, Math.max(0, transport.positionSecs)));
    setAttr(el, "aria-valuetext", `${formatTime(Number(el.value))} of ${formatTime(duration)}`);
  });

  return (
    <input
      className="playback-progress"
      ref={slider}
      type="range"
      min={0}
      max={0}
      step={0.1}
      defaultValue={0}
      disabled
      aria-label="Playback position"
      onPointerDown={(event) => {
        if (event.button !== 0) return;
        dragging.current = true;
        event.currentTarget.setPointerCapture(event.pointerId);
      }}
      onPointerUp={() => { dragging.current = false; }}
      onPointerCancel={() => { dragging.current = false; }}
      onLostPointerCapture={() => { dragging.current = false; }}
      onBlur={() => { dragging.current = false; }}
      onKeyDown={(event) => {
        if (isComposingReact(event)) return;
        event.stopPropagation();
      }}
      onChange={(event) => {
        void api.transportSeek(Number(event.currentTarget.value)).catch((err) => {
          dragging.current = false;
          pushToast("error", api.errorMessage(err));
        });
      }}
    />
  );
}
