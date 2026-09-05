/**
 * App state. Deliberately small: the authoritative model lives in Rust and
 * arrives as an `AppSnapshot`; everything else here is UI-only.
 *
 * The 60 Hz frame stream NEVER touches this store (see `frame.ts`), and neither
 * do the two pointer-rate gestures (EQ solo sweep, A/B offset drag) — they use
 * module refs in `audition.ts` / `align.ts`.
 */

import { create } from "zustand";
import type { AppSnapshot, Deck, EqWindowState, ToastKind, WaveformData } from "./types";

export interface Toast {
  id: number;
  kind: ToastKind;
  message: string;
}

export interface DeckWaveform {
  entryId: number | null;
  data: WaveformData | null;
  /** bumped whenever `data` is mutated in place so canvases can invalidate */
  version: number;
}

/**
 * Snapshot observers. Used by modules that keep a non-React mirror of a piece
 * of engine state (the A/B offset) and must not import the store's consumers.
 */
type SnapshotObserver = (snap: AppSnapshot) => void;
const observers = new Set<SnapshotObserver>();

export function onSnapshot(fn: SnapshotObserver): () => void {
  observers.add(fn);
  return () => {
    observers.delete(fn);
  };
}

/* The EQ is a separate window now (SPEC §12, `lib/eqwindow.ts`), so its
   visibility is not this store's to decide: it is whatever the window system
   says, relayed by Rust as `onyx://eq-window` and persisted in settings.json
   with the rest of the EQ. What used to live here — an `eqOpen` boolean in
   this webview's localStorage — could not survive the move: it would have been
   a second copy of a fact the other window could change, i.e. exactly the
   drift this feature had to avoid. `eqWindow` below is a cache of the last
   broadcast and is never written by the UI. */

interface OnyxState {
  snapshot: AppSnapshot | null;
  connected: boolean;
  lastError: string | null;

  /* UI-only */
  /** last `onyx://eq-window` broadcast; owned by Rust, never set locally */
  eqWindow: EqWindowState;
  settingsOpen: boolean;
  shortcutsOpen: boolean;
  blindOpen: boolean;
  dropActive: boolean;
  /**
   * The lane a drag is currently over, which is a deck-assignment target
   * (SPEC §2.8) rather than the window-level append of §2.2. It lives here, not
   * in `WaveformStack`, because inside Tauri the DOM never sees a file drag at
   * all: the native `onDragDropEvent` in `App.tsx` reports one window event
   * with a position, hit-tests it, and has to be able to light the lane it
   * found. In the browser the lanes set it from their own `dragover`.
   */
  dropDeck: Deck | null;
  selectedId: number | null;
  /** row highlighted the instant it is clicked, before the IPC round-trip */
  pendingPlayId: number | null;
  toasts: Toast[];
  waveforms: Record<Deck, DeckWaveform>;

  setSnapshot: (snap: AppSnapshot) => void;
  setConnected: (v: boolean, error?: string | null) => void;

  setEqWindow: (v: EqWindowState) => void;
  setSettingsOpen: (v: boolean) => void;
  toggleSettings: () => void;
  setShortcutsOpen: (v: boolean) => void;
  toggleShortcuts: () => void;
  setBlindOpen: (v: boolean) => void;
  toggleBlind: () => void;
  setDropActive: (v: boolean) => void;
  setDropDeck: (deck: Deck | null) => void;
  setSelectedId: (id: number | null) => void;
  setPendingPlayId: (id: number | null) => void;

  pushToast: (kind: ToastKind, message: string) => void;
  dismissToast: (id: number) => void;

  resetWaveform: (deck: Deck, entryId: number | null) => void;
  appendWaveform: (deck: Deck, entryId: number | null, chunk: WaveformData, from: number) => void;
}

let toastSeq = 1;

const emptyWaveform = (): DeckWaveform => ({ entryId: null, data: null, version: 0 });

export const useStore = create<OnyxState>((set, get) => ({
  snapshot: null,
  connected: false,
  lastError: null,

  eqWindow: { open: false, pinned: true },
  settingsOpen: false,
  shortcutsOpen: false,
  blindOpen: false,
  dropActive: false,
  dropDeck: null,
  selectedId: null,
  pendingPlayId: null,
  toasts: [],
  waveforms: { a: emptyWaveform(), b: emptyWaveform() },

  setSnapshot: (snap) => {
    set((s) => {
      // A deck that swapped tracks invalidates its cached waveform.
      let waveforms = s.waveforms;
      const fix = (deck: Deck, entryId: number | null): void => {
        if (waveforms[deck].entryId !== entryId) {
          waveforms = { ...waveforms, [deck]: { entryId, data: null, version: 0 } };
        }
      };
      fix("a", snap.deckA.entryId);
      fix("b", snap.deckB.entryId);
      return { snapshot: snap, connected: true, waveforms, pendingPlayId: null };
    });
    for (const fn of observers) fn(snap);
  },

  setConnected: (v, error = null) => set({ connected: v, lastError: error }),

  // The event repeats the current value whenever anything about the window
  // changes; re-rendering the title bar for an identical payload is noise.
  setEqWindow: (v) =>
    set((s) => (s.eqWindow.open === v.open && s.eqWindow.pinned === v.pinned ? {} : { eqWindow: v })),
  setSettingsOpen: (v) => set({ settingsOpen: v }),
  toggleSettings: () => set((s) => ({ settingsOpen: !s.settingsOpen })),
  setShortcutsOpen: (v) => set({ shortcutsOpen: v }),
  toggleShortcuts: () => set((s) => ({ shortcutsOpen: !s.shortcutsOpen })),
  setBlindOpen: (v) => set({ blindOpen: v }),
  toggleBlind: () => set((s) => ({ blindOpen: !s.blindOpen })),
  setDropActive: (v) => set({ dropActive: v, dropDeck: v ? get().dropDeck : null }),
  setDropDeck: (deck) => set((s) => (s.dropDeck === deck ? s : { dropDeck: deck })),
  setSelectedId: (id) => set({ selectedId: id }),
  setPendingPlayId: (id) => set({ pendingPlayId: id }),

  pushToast: (kind, message) => {
    const id = toastSeq++;
    set((s) => ({ toasts: [...s.toasts.slice(-3), { id, kind, message }] }));
    window.setTimeout(() => get().dismissToast(id), kind === "error" ? 7000 : 4200);
  },
  dismissToast: (id) => set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })),

  resetWaveform: (deck, entryId) =>
    set((s) => ({ waveforms: { ...s.waveforms, [deck]: { entryId, data: null, version: 0 } } })),

  appendWaveform: (deck, entryId, chunk, from) =>
    set((s) => {
      const cur = s.waveforms[deck];
      const fresh = cur.entryId !== entryId || cur.data == null || from === 0;
      const base: WaveformData = fresh
        ? { bucketSecs: chunk.bucketSecs, count: 0, expected: chunk.expected, min: [], max: [], rms: [] }
        : cur.data!;
      const data: WaveformData = {
        bucketSecs: chunk.bucketSecs || base.bucketSecs,
        expected: chunk.expected || base.expected,
        min: base.min.concat(chunk.min),
        max: base.max.concat(chunk.max),
        rms: base.rms.concat(chunk.rms),
        count: 0,
      };
      data.count = data.max.length;
      return {
        waveforms: { ...s.waveforms, [deck]: { entryId, data, version: cur.version + 1 } },
      };
    }),
}));

/**
 * Guard for the actions that would pull a track out from under a running blind
 * test: loading, assigning, removing or clearing playlist rows all clear or
 * replace a deck, which leaves the test running against material the subject
 * is no longer comparing (the engine does not stop it). Returns true — and
 * explains itself — when the caller must not proceed.
 */
export function blindLocked(what: string): boolean {
  const state = useStore.getState();
  if (!(state.snapshot?.blind.active ?? false)) return false;
  state.pushToast("warn", `${what} is locked while a blind test is running.`);
  return true;
}
