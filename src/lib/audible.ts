/**
 * Which deck you are hearing.
 *
 * `ab_select` and `ab_toggle_deck` return `()` and emit no `onyx://state`
 * (SPEC §3.1 — the command surface is fixed, and a deck switch has to be free),
 * so `AppSnapshot.transport.activeDeck` only refreshes when some *other*
 * command happens to push a snapshot. Anything that read the audible deck out
 * of the store was therefore showing the deck that was audible when that
 * snapshot was built: clicking a waveform lane switched the engine, the
 * frame-painted surfaces (the A|B buttons, the meters, the title-bar rate)
 * followed, and the React-rendered ones — the lane's AUDIBLE badge and active
 * styling, the playlist row highlight, the title bar's file name — stayed put.
 *
 * The authoritative value rides the 60 Hz frame stream. This module mirrors it,
 * and publishes only the *transition*: `setState` runs when the deck actually
 * changes, i.e. at most once per switch, so `frame.ts`'s rule still holds —
 * arriving frames never drive a render.
 */

import { useRef, useState } from "react";
import { currentTransport, useFrameEffect } from "./frame";
import { useStore } from "./store";
import type { Deck } from "./types";

/** Imperative read, for pointer handlers that must not depend on a render. */
export function audibleDeck(): Deck {
  return currentTransport()?.activeDeck ?? "a";
}

/** The audible deck, live. Re-renders on a switch and on nothing else. */
export function useAudibleDeck(): Deck {
  // Before the first frame lands there is nothing to mirror; the snapshot is
  // the only thing that knows, and it is right at that moment.
  const fromSnapshot = useStore((s) => s.snapshot?.transport.activeDeck ?? "a");
  const [live, setLive] = useState<Deck | null>(() => currentTransport()?.activeDeck ?? null);
  const liveRef = useRef(live);

  useFrameEffect((frame) => {
    const next = frame?.transport.activeDeck ?? null;
    if (next === liveRef.current) return;
    liveRef.current = next;
    setLive(next);
  });

  return live ?? fromSnapshot;
}
