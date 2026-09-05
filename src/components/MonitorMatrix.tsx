/**
 * Monitor matrix — SPEC.md §6.
 *
 * A monitoring fold, not a processing setting, so it is driven straight from
 * the 60 Hz frame stream and written into the DOM: pressing `S` feels
 * instantaneous and the badge can never disagree with the engine, because both
 * read the same `transport.monitorMode`. Nothing here re-renders React.
 *
 * `MonitorBadge` is always mounted and hidden by CSS while the mode is
 * `stereo`; that keeps the show/hide off the React path too.
 */

import { useCallback, useRef } from "react";
import * as api from "../lib/api";
import { setAttr, setText } from "../lib/dom";
import { currentTransport, useFrameEffect } from "../lib/frame";
import {
  MONITOR_FOLDS,
  MONITOR_HINT,
  monitorLegend,
  monitorSpec,
  toggleMonitor,
} from "../lib/monitor";
import { useStore } from "../lib/store";
import type { MonitorMode } from "../lib/types";

/** Optimistic write, corrected by the next frame if the engine disagrees. */
function useApplyMode(): (mode: MonitorMode) => void {
  const pushToast = useStore((s) => s.pushToast);
  return useCallback(
    (mode: MonitorMode) => {
      void api.setMonitorMode(mode).catch((err) => {
        pushToast("error", `Monitor fold refused: ${api.errorMessage(err)}`);
      });
    },
    [pushToast],
  );
}

function currentMode(mode: string | undefined): MonitorMode {
  return (mode ?? "stereo") as MonitorMode;
}

/* ── the segmented control that lives in the transport bar ────────────────── */

export function MonitorControl() {
  const apply = useApplyMode();
  const wrapRef = useRef<HTMLDivElement | null>(null);

  useFrameEffect((frame) => {
    const mode = currentMode(frame?.transport.monitorMode);
    const wrap = wrapRef.current;
    if (!wrap) return;
    setAttr(wrap, "data-mode", mode);
    for (const btn of wrap.querySelectorAll<HTMLButtonElement>("button[data-mode]")) {
      setAttr(btn, "data-on", String(btn.dataset.mode === mode));
    }
  });

  return (
    <div className="monitor">
      <span className="label" title={MONITOR_HINT}>
        Monitor
      </span>
      <div className="mon-opts" ref={wrapRef} data-mode="stereo">
        {MONITOR_FOLDS.map((m) => (
          <button
            key={m.mode}
            type="button"
            data-mode={m.mode}
            data-on="false"
            data-alarm={m.alarming ? "true" : undefined}
            title={`${m.long} \u2014 ${m.maths} \u00B7 key ${monitorLegend(m)}, again for stereo.\n${MONITOR_HINT}`}
            aria-label={`Monitor ${m.long}`}
            onClick={() => {
              // the frame stream, not the 10 Hz snapshot: a fold set by its
              // key a moment ago is not in the snapshot yet, and toggling
              // against a stale mode sends the engine back where it started
              const now = currentMode(currentTransport()?.monitorMode);
              apply(toggleMonitor(now, m.mode));
            }}
          >
            {m.short}
          </button>
        ))}
      </div>
    </div>
  );
}

/* ── the persistent, unmissable badge ────────────────────────────────────── */

export function MonitorBadge() {
  const apply = useApplyMode();
  const rootRef = useRef<HTMLButtonElement | null>(null);
  const nameRef = useRef<HTMLSpanElement | null>(null);
  const mathsRef = useRef<HTMLSpanElement | null>(null);

  useFrameEffect((frame) => {
    const mode = currentMode(frame?.transport.monitorMode);
    const spec = monitorSpec(mode);
    const root = rootRef.current;
    if (!root) return;
    setAttr(root, "data-on", String(mode !== "stereo"));
    setAttr(root, "data-tone", spec.alarming ? "alarm" : "champagne");
    setText(nameRef.current, spec.long.toUpperCase());
    setText(mathsRef.current, `${spec.maths} \u00B7 ${monitorLegend(spec)} or click to clear`);
  });

  return (
    <button
      type="button"
      className="state-badge"
      ref={rootRef}
      data-on="false"
      data-tone="champagne"
      title={`${MONITOR_HINT}\nClick to return to stereo.`}
      onClick={() => apply("stereo")}
    >
      <i className="sb-dot" />
      <span className="sb-k">Monitor</span>
      <span className="sb-v" ref={nameRef}>
        MONO
      </span>
      <span className="sb-m num" ref={mathsRef} />
    </button>
  );
}
