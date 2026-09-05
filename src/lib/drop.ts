/**
 * Dropping material onto a deck (SPEC §2.8).
 *
 * Two gestures land here, and they arrive through completely different
 * plumbing, which is why the shared parts live in one module rather than in
 * whichever component happened to need them first:
 *
 *  - **a playlist row dragged onto a waveform lane.** An HTML5 drag inside the
 *    document. The row publishes its entry id under {@link ENTRY_MIME}; the
 *    lane reads it back and assigns. `text/plain` carries the same id for
 *    debuggability, but the private type is what a lane accepts, so text
 *    dragged in from another app is never mistaken for a track.
 *  - **a file dragged in from Finder / Explorer onto a lane.** Inside Tauri the
 *    webview never sees a DOM drop for files: the native drag-drop handler
 *    swallows them and reports one window-level event with `paths` and a
 *    *position* (`App.tsx`). So the lane cannot claim that drop by stopping
 *    propagation — the window handler has to hit-test the pointer instead, and
 *    that is what {@link deckAtPoint} is for. In the browser preview the DOM
 *    drop does fire, so the lane stops propagation there and the window handler
 *    additionally ignores anything already handled.
 *
 * In both cases dropping on a lane means "put it on *that* deck", not the
 * window rule of §2.2 ("drop = append"). The file is still appended — nothing
 * is loaded that is not in the playlist — and then assigned, so the two rules
 * do not contradict each other.
 */

import * as api from "./api";
import { useStore } from "./store";
import type { AppSnapshot, Deck } from "./types";

/**
 * Private drag type for a playlist row. A MIME type rather than a flag on
 * `text/plain`, because `dragover` may only inspect `dataTransfer.types` —
 * the payload itself is unreadable until the drop — so the lane has to be able
 * to decide whether it accepts the drag from the type list alone.
 */
export const ENTRY_MIME = "application/x-onyx-entry";

/** True when the drag carries OS files (as opposed to a playlist row). */
export function carriesFiles(dt: DataTransfer | null | undefined): boolean {
  return Array.from(dt?.types ?? []).includes("Files");
}

/** True when the drag carries one of our playlist rows. */
export function carriesEntry(dt: DataTransfer | null | undefined): boolean {
  return Array.from(dt?.types ?? []).includes(ENTRY_MIME);
}

/** The entry id on a dropped playlist row, or `null` if this is not one. */
export function entryIdFromTransfer(dt: DataTransfer | null | undefined): number | null {
  const raw = dt?.getData(ENTRY_MIME);
  if (!raw) return null;
  const id = Number(raw);
  return Number.isFinite(id) ? id : null;
}

/**
 * Which deck's lane is under a point, in CSS pixels — `null` for anywhere
 * else, and for the anonymous lane of a blind test, whose deck is precisely
 * what must not be knowable (SPEC §7).
 */
export function deckAtPoint(x: number, y: number): Deck | null {
  const el = document.elementFromPoint(x, y);
  const lane = el?.closest?.(".wave-lane");
  if (!(lane instanceof HTMLElement)) return null;
  if (lane.dataset.masked === "true") return null;
  const deck = lane.dataset.deck;
  return deck === "a" || deck === "b" ? deck : null;
}

/**
 * Append dropped files and put the first of them on `deck`.
 *
 * Append, not replace: §2.1 reserves "replace the playlist" for opening, and a
 * lane drop is a drop. The assignment is a second command on purpose — it is
 * the same `ab_assign` every other route uses, so deck B still turns A/B on in
 * exactly one place (`abrules::ab_enabled_after_assign`).
 */
export async function dropFilesOnDeck(paths: string[], deck: Deck): Promise<AppSnapshot> {
  const before = new Set(useStore.getState().snapshot?.playlist.map((e) => e.id) ?? []);
  const appended = await api.openFiles(paths, false);
  const added = appended.playlist.find((e) => !before.has(e.id));
  if (!added) return appended;
  return api.abAssign(deck, added.id);
}
