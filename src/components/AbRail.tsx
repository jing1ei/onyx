/**
 * The A/B rail — level matching (SPEC §10) and time alignment (§11).
 *
 * Only mounted while A/B is enabled, and folded away entirely during a blind
 * test: every control here is labelled with a deck letter, which is exactly the
 * information a running test must not give away.
 *
 * The offset read-out is painted from the frame loop rather than from React so
 * that Alt-dragging lane B updates it at pointer rate.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import * as api from "../lib/api";
import {
  adoptOffset,
  alignRef,
  formatOffsetFrames,
  formatOffsetMs,
  nudgeOffset,
  setOffset,
} from "../lib/align";
import { setAttr, setText } from "../lib/dom";
import { currentTransport, useFrameEffect } from "../lib/frame";
import { formatSignedDb } from "../lib/format";
import { useStore } from "../lib/store";
import type { AlignResult, Deck } from "../lib/types";

/**
 * `fine` and `coarse` mark the steps a narrow window drops (`app.css`, "Narrow
 * windows"). Both are keyboard steps of the same key pair — `,` / `.` nudge by
 * 10 ms, with ⇧ by 100 ms and with ⌥ by one sample — and each pair of buttons
 * costs ~100 px that a 420 px window does not have. What is left at 420 is the
 * 10 ms step either side of the read-out, which is the one you reach for while
 * looking at the two lanes.
 */
const NUDGES: Array<{
  label: string;
  ms?: number;
  samples?: number;
  fine?: boolean;
  coarse?: boolean;
}> = [
  { label: "100 ms", ms: 100, coarse: true },
  { label: "10 ms", ms: 10 },
  { label: "1 ms", ms: 1, fine: true },
  { label: "1 smp", samples: 1, fine: true },
];

/** the same steps, coarsest-first for the "later" side of the read-out */
const NUDGES_LATER = [...NUDGES].reverse();

const engineRate = (): number => currentTransport()?.engineSampleRate ?? 48000;

/** Shared with the keyboard map: one place that knows how to move the offset. */
function nudgeOffsetBy(opts: { ms?: number; samples?: number }): void {
  nudgeOffset(opts, engineRate());
}

export function resetOffset(): void {
  setOffset(0, engineRate());
}

export default function AbRail() {
  const snapshot = useStore((s) => s.snapshot);
  const pushToast = useStore((s) => s.pushToast);
  const [aligning, setAligning] = useState(false);
  const [result, setResult] = useState<AlignResult | null>(null);

  const msRef = useRef<HTMLSpanElement | null>(null);
  const smpRef = useRef<HTMLSpanElement | null>(null);
  const groupRef = useRef<HTMLDivElement | null>(null);

  const ab = snapshot?.ab ?? null;
  const entryA = snapshot?.deckA.entryId ?? null;
  const entryB = snapshot?.deckB.entryId ?? null;
  const lm = ab?.levelMatch ?? { enabled: false, ready: false, trimDbA: 0, trimDbB: 0 };
  const bothLoaded = (snapshot?.deckA.loaded ?? false) && (snapshot?.deckB.loaded ?? false);
  const invertA = snapshot?.deckA.invert ?? false;
  const invertB = snapshot?.deckB.invert ?? false;

  const fail = useCallback((err: unknown) => pushToast("error", api.errorMessage(err)), [pushToast]);

  /* An auto-align note is about one pair of tracks. Swap either deck and it is
     stale advice about material that is no longer loaded. */
  useEffect(() => {
    setResult(null);
  }, [entryA, entryB]);

  /* offset read-out: frame-driven so a drag updates it without a re-render */
  useFrameEffect((frame) => {
    const rate = frame?.transport.engineSampleRate ?? 48000;
    const frames = alignRef.frames;
    setText(msRef.current, formatOffsetMs(frames, rate));
    setText(smpRef.current, formatOffsetFrames(frames));
    setAttr(groupRef.current, "data-offset", String(frames !== 0));
  });

  const runAutoAlign = useCallback(() => {
    setAligning(true);
    setResult(null);
    api
      .autoAlignAb()
      .then((res) => {
        setResult(res);
        if (res.applied) {
          adoptOffset(res.offsetFrames);
          pushToast(
            "info",
            `Aligned: ${res.offsetMs >= 0 ? "+" : "\u2212"}${Math.abs(res.offsetMs).toFixed(
              2,
            )} ms on B \u00B7 confidence ${(res.confidence * 100).toFixed(0)}%`,
          );
        } else {
          // an honest failure beats a confident wrong offset
          pushToast(
            "warn",
            "No confident alignment found \u2014 the offset is unchanged. Nudge deck B by hand, or Alt-drag lane B.",
          );
        }
      })
      .catch(fail)
      .finally(() => setAligning(false));
  }, [fail, pushToast]);

  const toggleInvert = useCallback(
    (deck: Deck, next: boolean) => {
      void api.setDeckInvert(deck, next).catch(fail);
    },
    [fail],
  );

  const matchLabel = !lm.enabled
    ? "Level match"
    : !lm.ready
      ? "MATCHED \u00B7 pending"
      : Math.abs(lm.trimDbA) < 0.05 && Math.abs(lm.trimDbB) < 0.05
        ? "MATCHED \u00B7 already level"
        : `MATCHED \u00B7 ${formatSignedDb(
            Math.abs(lm.trimDbB) > Math.abs(lm.trimDbA) ? lm.trimDbB : lm.trimDbA,
          )} dB on ${Math.abs(lm.trimDbB) > Math.abs(lm.trimDbA) ? "B" : "A"}`;

  return (
    <div className="ab-rail">
      <span className="label ab-rail-title">A / B</span>

      <button
        className="tr-toggle match"
        data-on={lm.enabled}
        data-pending={lm.enabled && !lm.ready}
        onClick={() => void api.setLevelMatch(!lm.enabled).catch(fail)}
        title={
          "Match the two decks by integrated LUFS (G).\nOff by default: the signal path is untouched unless you ask for it.\nOnly ever attenuates \u2014 the louder deck comes down."
        }
      >
        {matchLabel}
      </button>

      <span className="rail-sep" />

      <div className="align-group" ref={groupRef} data-offset="false">
        <span className="label" title="Deck B is moved; deck A is the reference timeline">
          Align
        </span>

        <button
          className="ghost-btn"
          disabled={!bothLoaded || aligning}
          onClick={runAutoAlign}
          title="Estimate the offset by cross-correlating the two decks"
        >
          {aligning ? "Working" : "Auto"}
        </button>

        <div className="nudges">
          {NUDGES.map((n) => (
            <button
              key={`-${n.label}`}
              className="num"
              data-fine={n.fine ? "true" : undefined}
              data-coarse={n.coarse ? "true" : undefined}
              disabled={!bothLoaded}
              onClick={() => nudgeOffsetBy({ ms: n.ms ? -n.ms : undefined, samples: n.samples ? -n.samples : undefined })}
              title={`Deck B ${n.label} earlier`}
            >
              {"\u2212"}
              {n.label}
            </button>
          ))}
          <span className="nudge-mid">
            <span className="num off-ms" ref={msRef}>
              0.00 ms
            </span>
            <span className="num off-smp" ref={smpRef}>
              0 smp
            </span>
          </span>
          {NUDGES_LATER.map((n) => (
            <button
              key={`+${n.label}`}
              className="num"
              data-fine={n.fine ? "true" : undefined}
              data-coarse={n.coarse ? "true" : undefined}
              disabled={!bothLoaded}
              onClick={() => nudgeOffsetBy({ ms: n.ms, samples: n.samples })}
              title={`Deck B ${n.label} later`}
            >
              +{n.label}
            </button>
          ))}
        </div>

        <button
          className="ghost-btn"
          disabled={!bothLoaded}
          onClick={resetOffset}
          title="Back to a zero offset"
        >
          Reset
        </button>
      </div>

      <span className="rail-sep" />

      <div className="invert-group">
        <span className="label" title="Polarity invert — flips the whole deck">
          {"\u00F8"}
        </span>
        <button
          className="tr-toggle sm"
          data-on={invertA}
          onClick={() => toggleInvert("a", !invertA)}
          title="Invert the polarity of deck A"
        >
          A
        </button>
        <button
          className="tr-toggle sm"
          data-on={invertB}
          onClick={() => toggleInvert("b", !invertB)}
          title="Invert the polarity of deck B"
        >
          B
        </button>
      </div>

      {result && !result.applied && (
        <span className="align-note warn">
          {`Auto-align was not confident (${(result.confidence * 100).toFixed(
            0,
          )}%) \u2014 use the nudges or Alt-drag lane B.`}
        </span>
      )}
      {result?.polarityInverted && (
        <span className="align-note">
          {"The best match was polarity-inverted \u2014 try \u00F8 on one deck."}
        </span>
      )}

      <span className="spacer" />
      <span className="align-hint">
        {"Alt-drag lane B to slide \u00B7 , / . to nudge \u00B7 \u21E7 = 100 ms, \u2325 = 1 sample"}
      </span>
    </div>
  );
}
