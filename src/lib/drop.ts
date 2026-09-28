/** Deck assignment from playlist rows and external file drops (SPEC §2.8).
 * Native Tauri drops require position hit-testing; browser drops use DOM events.
 * External files are appended before assigning their first entry to a deck.
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
 * the same `ab_assign` every other route uses; comparison remains controlled
 * by the separate A/B switch.
 */
export async function dropFilesOnDeck(paths: string[], deck: Deck): Promise<AppSnapshot> {
  const before = new Set(useStore.getState().snapshot?.playlist.map((e) => e.id) ?? []);
  const appended = await api.openFiles(paths, false);
  const added = appended.playlist.find((e) => !before.has(e.id));
  if (!added) return appended;
  return api.abAssign(deck, added.id);
}
