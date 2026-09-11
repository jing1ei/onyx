import { t } from "../lib/i18n";
/**
 * Persistent monitoring badges.
 *
 * Three states can make a user believe the app is broken: a monitor fold
 * (SPEC §6), a non-zero A/B offset (§11) and an active band-solo audition
 * (§12). Each gets a quiet, always-mounted badge that is shown by CSS from a
 * `data-on` attribute written inside the frame loop — no React re-render, and
 * no way for the badge to disagree with the engine.
 *
 * The audition badge is the reason `AuditionFrame` exists. The sweep is
 * dragged in the *EQ window* now, and a module ref in that webview is invisible
 * from this one — so this badge reads the bandpass off the engine's own frame
 * stream instead. The user hears a narrow filter; the window that explains it
 * may be on another monitor, or closed; this badge is what is left, so it must
 * be driven by the thing that is actually filtering the audio.
 */

import { useRef } from "react";
import { alignRef, formatOffsetFrames, formatOffsetMs } from "../lib/align";
import { forceStopAudition } from "../lib/audition";
import { setAttr, setText } from "../lib/dom";
import { useFrameEffect } from "../lib/frame";
import { useStore } from "../lib/store";
import { formatFreqWithNote } from "../lib/eq";
import { resetOffset } from "./AbRail";
import { MonitorBadge } from "./MonitorMatrix";

function OffsetBadge() {
  const root = useRef<HTMLButtonElement | null>(null);
  const val = useRef<HTMLSpanElement | null>(null);

  useFrameEffect((frame) => {
    // Hidden during a blind test: it is the last piece of deck-labelled
    // chrome on screen, and the A/B rail is folded away for the same reason.
    const blind = useStore.getState().snapshot?.blind.active ?? false;
    const on = !blind && (frame?.transport.abEnabled ?? false) && alignRef.frames !== 0;
    setAttr(root.current, "data-on", String(on));
    if (!on) return;
    const rate = frame?.transport.engineSampleRate ?? 48000;
    setText(
      val.current,
      `${formatOffsetMs(alignRef.frames, rate)} \u00B7 ${formatOffsetFrames(alignRef.frames)}`,
    );
  });

  return (
    <button
      type="button"
      className="state-badge"
      data-tone="steel"
      data-on="false"
      ref={root}
      onClick={resetOffset}
      title={t("Deck B is time-shifted against deck A. Click to reset to zero.")}
    >
      <i className="sb-dot" />
      <span className="sb-k">{t("B offset")}</span>
      <span className="sb-v num" ref={val} />
    </button>
  );
}

function AuditionBadge() {
  const root = useRef<HTMLButtonElement | null>(null);
  const val = useRef<HTMLSpanElement | null>(null);

  useFrameEffect((frame) => {
    const aud = frame?.audition ?? null;
    setAttr(root.current, "data-on", String(aud != null));
    if (!aud) return;
    setText(val.current, `${formatFreqWithNote(aud.freqHz)} \u00B7 Q ${aud.q.toFixed(1)}`);
  });

  return (
    <button
      type="button"
      className="state-badge"
      data-tone="alarm"
      data-on="false"
      ref={root}
      onClick={forceStopAudition}
      title={t("A narrow band-pass is being auditioned. This is not the programme. Click to stop.")}
    >
      <i className="sb-dot" />
      <span className="sb-k">{t("Band solo")}</span>
      <span className="sb-v num" ref={val} />
    </button>
  );
}

export default function BadgeRail() {
  return (
    <div className="badge-rail">
      <MonitorBadge />
      <OffsetBadge />
      <AuditionBadge />
    </div>
  );
}
