/**
 * The keyboard map from SPEC.md §4 plus the monitor matrix keys from
 * SPEC.md §6, installed once at app level.
 *
 * Rules:
 *  - every key is ignored while focus sits in a text field or a `<select>`,
 *    and while an input method is composing (`lib/ime.ts`) — a candidate
 *    window's `Tab` / `Space` / digits are not this app's shortcuts;
 *  - while a blind test is running, nothing may change which deck is audible
 *    except the blind slot keys — otherwise the test is silently invalidated
 *    (and `A` / `B` would tell the subject exactly what they are listening to);
 *  - the audible slot / vote keys follow the protocol: `ab` uses x / y, `abx`
 *    uses a / b / x;
 *  - mnemonic keys are matched on the character (`M` is mute on every layout),
 *    positional keys on `event.code` (`[ ] \ , . 1 2` are unreachable, or
 *    AltGr-only, on most non-US layouts). See `lib/layout.ts` for the whole
 *    argument.
 */

import * as api from "./api";
import { isTypingTarget } from "./dom";
import type { Deck } from "./types";
import { nudgeOffset } from "./align";
import { toggleEqWindow } from "./eqwindow";
import { currentTransport } from "./frame";
import { dbToLin, linToDb } from "./format";
import { isComposing } from "./ime";
import { keyLegend, observeLegend, POSITIONAL_KEYS, type PositionalCode } from "./layout";
import { monitorCode, monitorForEvent, MONITOR_FOLDS, toggleMonitor } from "./monitor";
import { blindLocked, useStore } from "./store";
import type { BlindState } from "./types";

/**
 * The deck a key names, by **physical position** first (`KeyA` / `KeyB`, SPEC
 * §4) and by the character second, so the binding lands on the key legended A
 * or B on a US layout *and* on the key in that position elsewhere. Shift is
 * irrelevant to both handles, which is what makes `⇧A` / `⇧B` safe to bind on
 * the same keys as plain `A` / `B`.
 */
function deckForKey(e: KeyboardEvent): Deck | null {
  if (e.code === "KeyA") return "a";
  if (e.code === "KeyB") return "b";
  const k = e.key.toLowerCase();
  if (k === "a" || k === "b") return k;
  return null;
}

function fail(err: unknown): void {
  useStore.getState().pushToast("error", api.errorMessage(err));
}

function run(p: Promise<unknown>): void {
  void p.catch(fail);
}

/** volume is linear 0..1; the keyboard steps it in dB. */
function nudgeVolume(deltaDb: number): void {
  const cur = currentTransport()?.volume ?? 1;
  const db = cur <= 0.0001 ? -60 : linToDb(cur);
  const next = db + deltaDb;
  run(api.setVolume(next <= -60 ? 0 : dbToLin(Math.min(0, next))));
}

/** Slots the subject can switch to, and the two slots a vote can name. */
function protocolSlots(blind: BlindState | null | undefined): {
  switchable: string[];
  answers: string[];
} {
  if (blind?.mode === "abx") return { switchable: ["a", "b", "x"], answers: ["a", "b"] };
  return { switchable: ["x", "y"], answers: ["x", "y"] };
}

export interface KeyMapRow {
  /** US legend, and the fallback when the real one is unknown. */
  keys: string;
  action: string;
  /**
   * Physical keys this row is bound to, for the rows Onyx matches by position
   * rather than by character. Their legends differ per layout, so the overlay
   * resolves them at render time instead of printing `keys`.
   */
  codes?: PositionalCode[];
}

/** The single source of truth for the map (SPEC §4). */
export const SHORTCUTS: KeyMapRow[] = [
  { keys: "Space", action: "Play / pause" },
  { keys: "\u2190 \u2192", action: "Nudge \u22135 s / +5 s (\u21E7 = 1 s)" },
  { keys: "\u2191 \u2193", action: "Volume \u00B11 dB" },
  { keys: "Home", action: "Seek to start" },
  { keys: "A / B", action: "Listen to deck A / deck B \u00B7 ABX slots A / B" },
  {
    keys: "\u21E7A / \u21E7B",
    action: "Assign the selected track to deck A / deck B (B turns A/B on)",
  },
  { keys: "Tab", action: "Toggle A/B deck" },
  { keys: "X / Y", action: "Blind slot switch (X only in ABX)" },
  {
    keys: "1 / 2",
    codes: ["Digit1", "Digit2"],
    action: "Blind vote \u00B7 A/B: X / Y is deck A \u00B7 ABX: X = A / X = B",
  },
  { keys: "L", action: "Loop on / off" },
  { keys: "M", action: "Mute" },
  { keys: "E", action: "EQ window \u2014 \u21E7E bypasses the EQ" },
  { keys: "\u2318/Ctrl + drag", action: "EQ band-solo sweep (X = frequency, Y = Q)" },
  { keys: "\u2325 + click node", action: "Bypass that EQ band" },
  { keys: "G", action: "Level match on / off (A/B)" },
  {
    keys: ", / .",
    codes: ["Comma", "Period"],
    action: "Nudge deck B earlier / later \u00B7 10 ms, \u21E7 100 ms, \u2325 1 sample",
  },
  { keys: "\u2325 + drag lane B", action: "Slide the A/B time offset" },
  ...MONITOR_FOLDS.map((m) => {
    const code = monitorCode(m);
    return {
      keys: code ? POSITIONAL_KEYS[code] : (m.char ?? "").toUpperCase(),
      codes: code ? [code] : undefined,
      action: `Monitor: ${m.long.toLowerCase()} (${m.maths}) \u2014 again for stereo`,
    };
  }),
  { keys: "Delete", action: "Remove selected track" },
  { keys: "\u2318/Ctrl + O", action: "Open files (replaces playlist)" },
  { keys: "\u2318/Ctrl + \u21E7 + O", action: "Add files (appends)" },
  { keys: "\u2318/Ctrl + K", action: "Clear playlist" },
  { keys: "Esc", action: "Close the top-most panel" },
  { keys: "?", action: "This overlay" },
];

/**
 * `SHORTCUTS` with every positional key relabelled for the keyboard actually
 * attached — `,` is `;` on AZERTY and `[` does not exist on it at all, so a
 * fixed legend would be a lie. Recomputed on each render of the overlay,
 * because the legends arrive asynchronously (see `lib/layout.ts`).
 */
export function shortcutRows(): KeyMapRow[] {
  return SHORTCUTS.map((row) =>
    row.codes ? { ...row, keys: row.codes.map(keyLegend).join(" / ") } : row,
  );
}

/**
 * Keys that mean "do it again" when held. Everything else is a toggle or a
 * one-shot: auto-repeat on `Space` flapped play/pause at the key-repeat rate,
 * and on `L`/`M`/`G` it flapped the engine's state just as fast.
 */
const REPEATABLE = new Set(["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"]);
/** The same, for the keys matched by position rather than by character. */
const REPEATABLE_CODES = new Set(["Comma", "Period"]);

export function installKeyboard(): () => void {
  const onKeyDown = (e: KeyboardEvent): void => {
    // Learn the legends even while the user is typing in a field: on macOS
    // this is the only way the overlay ever finds out what `[` is called here.
    observeLegend(e);
    if (useStore.getState().editorActive) {
      const state = useStore.getState();
      if (e.key === 'Escape') { state.setSettingsOpen(false); state.setShortcutsOpen(false); }
      if (e.key === '?' && !isTypingTarget(e.target) && !isComposing(e)) state.toggleShortcuts();
      return;
    }
    /* An input method is composing. Focus is normally in a field then, which
       the next line already catches, but not always: a candidate window is
       open over a webview whose focus has moved (a panel closed under it, the
       caret is in a canvas-hosted control), and every binding below —
       `Space`, `Tab`, `1`/`2`, `,`/`.` — is a key that IME wants and calls
       `preventDefault()` on. See `lib/ime.ts`. */
    if (isComposing(e)) return;
    if (isTypingTarget(e.target)) return;
    if (e.repeat && !REPEATABLE.has(e.key) && !REPEATABLE_CODES.has(e.code)) return;
    const store = useStore.getState();
    const blind = store.snapshot?.blind ?? null;
    const blindActive = blind?.active ?? false;
    const { switchable, answers } = protocolSlots(blind);
    const mod = e.metaKey || e.ctrlKey;

    if (mod) {
      const k = e.key.toLowerCase();
      if (k === "o") {
        e.preventDefault();
        const replace = !e.shiftKey;
        // shift appends, which leaves both decks alone; a plain O replaces them
        if (replace && blindLocked("Opening files")) return;
        run(api.pickAndOpenFiles(replace));
        return;
      }
      if (k === "k") {
        e.preventDefault();
        if (blindLocked("Clearing the playlist")) return;
        run(api.playlistClear());
        return;
      }
      return; // leave every other accelerator to the OS / webview
    }

    switch (e.key) {
      case " ":
      case "Spacebar":
        e.preventDefault();
        run(api.transportToggle());
        return;
      case "ArrowLeft":
        e.preventDefault();
        run(api.transportNudge(e.shiftKey ? -1 : -5));
        return;
      case "ArrowRight":
        e.preventDefault();
        run(api.transportNudge(e.shiftKey ? 1 : 5));
        return;
      case "ArrowUp":
        e.preventDefault();
        nudgeVolume(1);
        return;
      case "ArrowDown":
        e.preventDefault();
        nudgeVolume(-1);
        return;
      case "Home":
        e.preventDefault();
        run(api.transportSeek(0));
        return;
      case "Tab":
        e.preventDefault();
        // switching decks by hand during a blind test would invalidate it
        if (!blindActive) run(api.abToggleDeck());
        return;
      case "Escape":
        if (store.shortcutsOpen) store.setShortcutsOpen(false);
        else if (store.settingsOpen) store.setSettingsOpen(false);
        else if (store.blindOpen && !blindActive) store.setBlindOpen(false);
        // The EQ is a window of its own now, not a panel in this one: Esc here
        // closes what is in front of the user, and a window on another monitor
        // is not it. `E` still toggles it, and it has its own Esc.
        return;
      case "Delete":
      case "Backspace": {
        const id = store.selectedId;
        if (id != null) {
          e.preventDefault();
          if (blindLocked("Removing a track")) return;
          run(api.playlistRemove(id));
        }
        return;
      }
      // `?` stays character-based: every layout can produce it, but from a
      // different key (Shift+/ on US and JIS, Shift+ß on German, Shift++ on
      // the Nordic layouts), so the character is the only portable handle.
      case "?":
        e.preventDefault();
        store.toggleShortcuts();
        return;
      case "/":
        if (e.shiftKey) {
          e.preventDefault();
          store.toggleShortcuts();
        }
        return;
      default:
        break;
    }

    const key = e.key.toLowerCase();

    // monitor matrix — one key per fold, pressing it again returns to stereo
    const fold = monitorForEvent(e);
    if (fold) {
      e.preventDefault();
      const current = currentTransport()?.monitorMode ?? "stereo";
      run(api.setMonitorMode(toggleMonitor(current, fold)));
      return;
    }

    /* A / B — the two things they can mean, kept visibly apart.
       Plain: *listen* to that deck; it changes nothing about what is loaded.
       Shift: *assign* the selected playlist row to that deck, one of the four
       routes to deck B in SPEC §2.8. Conflating them is the bug this map is
       part of fixing — a user pressed `B`, heard the same deck A material,
       and reported deck B as unassignable. */
    const deck = deckForKey(e);
    if (deck) {
      if (blindActive) {
        // during ABX, A and B are slots, not decks; during 2AFC they do nothing
        if (!e.shiftKey && switchable.includes(deck)) run(api.blindSwitch(deck));
        return;
      }
      e.preventDefault();
      if (!e.shiftKey) {
        run(api.abSelect(deck));
        return;
      }
      const id = store.selectedId;
      if (id == null) {
        store.pushToast(
          "info",
          `Select a playlist row first \u2014 \u21E7${deck.toUpperCase()} assigns it to deck ${deck.toUpperCase()}.`,
        );
        return;
      }
      if (blindLocked("Assigning a deck")) return;
      run(api.abAssign(deck, id).then((snap) => store.setSnapshot(snap)));
      return;
    }

    switch (key) {
      case "x":
      case "y":
        if (blindActive && switchable.includes(key)) run(api.blindSwitch(key));
        break;
      case "l": {
        const t = currentTransport();
        run(api.setLoopEnabled(!(t?.loopEnabled ?? false)));
        break;
      }
      case "m": {
        const t = currentTransport();
        run(api.setMuted(!(t?.muted ?? false)));
        break;
      }
      case "e":
        if (e.shiftKey) {
          // bypass without opening the window
          const eq = store.snapshot?.eq;
          if (eq) run(api.setEq({ ...eq, enabled: !eq.enabled }));
        } else {
          // Rust decides whether that means "create", "focus" or "close": the
          // same key does the same thing from either window (SPEC §12).
          toggleEqWindow();
        }
        break;
      case "g": {
        // Changing the trim mid-test changes the very thing under test, and the
        // A/B rail is hidden anyway — refuse rather than silently invalidate.
        if (blindLocked("Level matching")) break;
        const enabled = store.snapshot?.ab.levelMatch.enabled ?? false;
        run(api.setLevelMatch(!enabled));
        break;
      }
      default:
        break;
    }

    /* Positional keys (SPEC §4, §11). Matched on `e.code`, so they survive
       both a non-US layout and a modifier rewriting the character: Shift+`,`
       reports `<` and macOS Option+`,` reports `≤`, which is why the 100 ms
       and one-sample nudges never fired before. */
    switch (e.code) {
      case "Digit1":
      case "Digit2": {
        if (!blindActive) break;
        const slot = answers[e.code === "Digit1" ? 0 : 1];
        if (slot) run(api.blindVote(slot));
        break;
      }
      case "Comma":
      case "Period": {
        // plain = 10 ms, shift = 100 ms, alt = one sample (SPEC §11)
        if (!(store.snapshot?.ab.enabled ?? false) || blindActive) break;
        e.preventDefault();
        const sign = e.code === "Comma" ? -1 : 1;
        const rate = currentTransport()?.engineSampleRate ?? 48000;
        if (e.altKey) nudgeOffset({ samples: sign }, rate);
        else nudgeOffset({ ms: sign * (e.shiftKey ? 100 : 10) }, rate);
        break;
      }
      default:
        break;
    }
  };

  window.addEventListener("keydown", onKeyDown);
  return () => window.removeEventListener("keydown", onKeyDown);
}
