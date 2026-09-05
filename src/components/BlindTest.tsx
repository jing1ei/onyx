/**
 * Blind testing — 2AFC (`ab`) and ABX (`abx`), SPEC.md §7.
 *
 * Leak discipline, because a single slip makes the whole feature worthless:
 *  - the panel only ever renders slot letters that come from `blind.slots`, in
 *    that order, and never anything derived from deck state (no deck letters,
 *    no file names, no LUFS, no trims, no `data-deck` attributes);
 *  - tooltips are written from the slot letter alone;
 *  - the mapping is only read from `blind.mapping` / `blind.abxMapping`, which
 *    the backend leaves null until the test is finished.
 */

import { useCallback, useEffect, useState } from "react";
import * as api from "../lib/api";
import { formatPValue } from "../lib/format";
import { useStore } from "../lib/store";
import type { BlindMode, BlindState } from "../lib/types";
import { IconClose } from "./Icons";

const TRIAL_OPTIONS = [5, 8, 12, 20];

const PROTOCOLS: Array<{ mode: BlindMode; label: string; blurb: string }> = [
  {
    mode: "ab",
    label: "A / B",
    blurb:
      "Two hidden slots, X and Y, re-randomised every trial. Switch as often as you like, then say which slot is deck A.",
  },
  {
    mode: "abx",
    label: "ABX",
    blurb:
      "A is deck A, B is deck B, X is one of them \u2014 re-randomised every trial. Switch freely, then say whether X is A or B.",
  },
];

/** Slot label for the UI: the letter itself, never the deck behind it. */
const slotLabel = (slot: string): string => slot.toUpperCase();

function verdict(score: number, n: number, p: number | null): string {
  const head = `${score} / ${n} correct, p = ${formatPValue(p)}`;
  if (p == null) return head;
  return p < 0.05
    ? `${head} \u2014 you can reliably hear a difference`
    : `${head} \u2014 no evidence you can hear a difference`;
}

export default function BlindTest() {
  const snapshot = useStore((s) => s.snapshot);
  const setBlindOpen = useStore((s) => s.setBlindOpen);
  const pushToast = useStore((s) => s.pushToast);
  const [blind, setBlind] = useState<BlindState | null>(snapshot?.blind ?? null);
  const [trials, setTrials] = useState(8);
  const [mode, setMode] = useState<BlindMode>("abx");
  const [busy, setBusy] = useState(false);
  /**
   * `finished` stays true in the engine until the next run, so the protocol
   * picker was unreachable for the rest of the session after one test: closing
   * and reopening the panel came straight back to the old result, and "Run
   * again" is fixed to the same mode and trial count. This is the way back.
   */
  const [setupAgain, setSetupAgain] = useState(false);

  useEffect(() => {
    if (snapshot?.blind) setBlind(snapshot.blind);
  }, [snapshot?.blind]);

  const fail = useCallback((err: unknown) => pushToast("error", api.errorMessage(err)), [pushToast]);

  const run = useCallback(
    (p: Promise<BlindState>) => {
      setBusy(true);
      p.then((s) => {
        setBlind(s);
        setSetupAgain(false);
      })
        .catch(fail)
        .finally(() => setBusy(false));
    },
    [fail],
  );

  const active = blind?.active ?? false;
  const finished = (blind?.finished ?? false) && !setupAgain;
  // while a test is up (running or revealed) the engine owns the protocol;
  // in the setup view the picker does
  const runningMode: BlindMode = (active || finished) && blind ? blind.mode : mode;
  const slots = active && blind ? blind.slots : runningMode === "abx" ? ["a", "b", "x"] : ["x", "y"];
  const answers = runningMode === "abx" ? ["a", "b"] : ["x", "y"];
  const current = blind?.currentSlot ?? slots[0];
  const trial = blind?.trial ?? 0;
  const totalTrials = blind?.trials ?? trials;
  const score = blind?.score ?? 0;
  const bothLoaded = (snapshot?.deckA.loaded ?? false) && (snapshot?.deckB.loaded ?? false);

  const question =
    runningMode === "abx" ? "Is X the same as A, or the same as B?" : "Which slot is deck A?";
  const voteLabel = (slot: string): string =>
    runningMode === "abx" ? `X = ${slotLabel(slot)}` : `${slotLabel(slot)} is A`;

  return (
    <div className="float-panel blind">
      <div className="panel-head">
        <span className="panel-title">
          {runningMode === "abx" ? "ABX test" : "Blind A / B"}
        </span>
        <span className="spacer" />
        {active ? (
          <span className="label num">
            Trial {trial} / {totalTrials}
          </span>
        ) : (
          <span className="label">{finished ? "Complete" : "Ready"}</span>
        )}
        <button
          className="close-btn"
          disabled={busy}
          onClick={() => {
            if (!active) {
              setBlindOpen(false);
              return;
            }
            // Close only once the engine confirms the abort. Closing first and
            // firing the abort blind left a *running* test with no panel when
            // the command failed: the deck keys stay locked and nothing on
            // screen can reach the test any more.
            setBusy(true);
            api
              .blindAbort()
              .then((s) => {
                setBlind(s);
                setBlindOpen(false);
              })
              .catch(fail)
              .finally(() => setBusy(false));
          }}
          aria-label="Close"
        >
          <IconClose />
        </button>
      </div>

      {!active && !finished && (
        <>
          <div className="proto-picker">
            {PROTOCOLS.map((p) => (
              <button
                key={p.mode}
                className="proto"
                data-on={mode === p.mode}
                onClick={() => setMode(p.mode)}
              >
                <span className="s">{p.label}</span>
                <span className="h">{p.mode === "abx" ? "3 slots" : "2 slots"}</span>
              </button>
            ))}
          </div>
          <div className="eq-hint" style={{ marginBottom: 12 }}>
            {PROTOCOLS.find((p) => p.mode === mode)?.blurb}
            {mode === "abx"
              ? " Keys: A / B / X to switch, 1 = X is A, 2 = X is B."
              : " Keys: X / Y to switch, 1 = X is A, 2 = Y is A."}
          </div>

          <div className="field" style={{ borderTop: "1px solid var(--hairline)" }}>
            <span className="k">Trials</span>
            <div className="xfade-opts">
              {TRIAL_OPTIONS.map((n) => (
                <button key={n} className="num" data-on={trials === n} onClick={() => setTrials(n)}>
                  {n}
                </button>
              ))}
            </div>
          </div>

          {!bothLoaded && (
            <div className="eq-hint" style={{ color: "var(--m-hot)", marginTop: 10 }}>
              Both decks need a track before a test can start.
            </div>
          )}

          <div className="blind-actions" style={{ marginTop: 16 }}>
            <button className="solid-btn quiet" onClick={() => setBlindOpen(false)}>
              Cancel
            </button>
            <button
              className="solid-btn"
              disabled={busy || !bothLoaded}
              onClick={() => run(api.blindStart(trials, mode))}
            >
              Begin
            </button>
          </div>
        </>
      )}

      {active && blind && (
        <>
          <div className="blind-slots" data-count={slots.length}>
            {slots.map((slot) => (
              <button
                key={slot}
                className="blind-slot"
                data-on={current === slot}
                disabled={busy}
                onClick={() => run(api.blindSwitch(slot))}
                title={`Listen to slot ${slotLabel(slot)}`}
              >
                <span className="s">{slotLabel(slot)}</span>
                <span className="h">{current === slot ? "audible" : "switch"}</span>
              </button>
            ))}
          </div>

          <div className="blind-progress">
            <i style={{ width: `${((trial - 1) / Math.max(1, totalTrials)) * 100}%` }} />
          </div>

          <div className="blind-question">{question}</div>

          <div className="blind-actions">
            {answers.map((slot) => (
              <button
                key={slot}
                className="solid-btn"
                disabled={busy}
                onClick={() => run(api.blindVote(slot))}
              >
                {voteLabel(slot)}
              </button>
            ))}
          </div>

          <div className="field" style={{ marginTop: 6 }}>
            <span className="k">Running score</span>
            <span className="v num">
              {score} / {Math.max(0, trial - 1)}
            </span>
          </div>

          <button
            className="solid-btn quiet"
            style={{ width: "100%", marginTop: 8 }}
            disabled={busy}
            onClick={() => run(api.blindAbort())}
          >
            Abort
          </button>
        </>
      )}

      {!active && finished && blind && (
        <>
          <div className="blind-score">
            <span className="v num">
              {score}
              <span style={{ fontSize: 15, color: "var(--text-faint)" }}>/{blind.votes.length}</span>
            </span>
            <span className="k">correct identifications</span>
          </div>

          <div className="blind-verdict" data-significant={blind.pValue != null && blind.pValue < 0.05}>
            {verdict(score, blind.votes.length, blind.pValue)}
          </div>

          <div className="field">
            <span className="k">One-tailed exact binomial</span>
            <span className="v num">p = {formatPValue(blind.pValue)}</span>
          </div>

          {blind.mode === "ab" && blind.mapping && (
            <div className="field">
              <span className="k">Final mapping</span>
              <span className="v num">
                X = deck {blind.mapping.x.toUpperCase()} {"\u00B7"} Y = deck{" "}
                {blind.mapping.y.toUpperCase()}
              </span>
            </div>
          )}
          {blind.mode === "abx" && blind.abxMapping && (
            <div className="field">
              <span className="k">X on the final trial</span>
              <span className="v num">deck {blind.abxMapping.x.toUpperCase()}</span>
            </div>
          )}

          <table className="result-table">
            <thead>
              <tr>
                <th>#</th>
                <th>Chose</th>
                <th>Answer</th>
                <th>Result</th>
              </tr>
            </thead>
            <tbody>
              {blind.votes.map((v) => (
                <tr key={v.trial}>
                  <td className="num">{v.trial}</td>
                  <td className="num">{slotLabel(v.chose)}</td>
                  <td className="num">{slotLabel(v.correctSlot)}</td>
                  <td data-ok={v.correct}>{v.correct ? "hit" : "miss"}</td>
                </tr>
              ))}
            </tbody>
          </table>

          <div className="blind-actions" style={{ marginTop: 14 }}>
            <button className="solid-btn quiet" onClick={() => setBlindOpen(false)}>
              Close
            </button>
            <button
              className="solid-btn quiet"
              disabled={busy}
              onClick={() => {
                setMode(blind.mode);
                setTrials(blind.trials);
                setSetupAgain(true);
              }}
            >
              New test
            </button>
            <button
              className="solid-btn"
              disabled={busy || !bothLoaded}
              onClick={() => run(api.blindStart(totalTrials, blind.mode))}
            >
              Run again
            </button>
          </div>
        </>
      )}
    </div>
  );
}
