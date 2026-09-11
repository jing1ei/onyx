import { t, useLanguage } from "../lib/i18n";
/**
 * The detached EQ window's root — SPEC §12.
 *
 * Everything this document does that the main window does not:
 *
 *  - it starts the bridge **without** the waveform poller. It draws no lanes,
 *    so pulling decoded buckets over IPC would be pure cost;
 *  - it carries its own, deliberately tiny keyboard map. The app-wide map of
 *    SPEC §4 is bound to things this window has no notion of (a selected
 *    playlist row, the A/B rail, the settings panel), and a tool window that
 *    swallows `Delete` and removes a track from a list you cannot see is a
 *    trap. Four keys survive: `Space`, because pausing without reaching for
 *    the other window is the whole point of a second monitor; `Shift+E` for
 *    bypass; and `E` / `Esc` to close;
 *  - it mounts the panel unconditionally. This window *is* the panel, so the
 *    analyser's lifetime and the window's lifetime are the same thing, which
 *    is what keeps `set_spectrum_enabled` honest with one consumer in another
 *    process' worth of JavaScript.
 */

import { useEffect } from "react";
import * as api from "../lib/api";
import { installFocusHygiene } from "../lib/focus";
import { isTypingTarget } from "../lib/dom";
import { closeEqWindow, startEqWindowSync } from "../lib/eqwindow";
import { isComposing } from "../lib/ime";
import { startBridge } from "../lib/frame";
import { useStore } from "../lib/store";
import { startAppearanceSync } from "../lib/theme";
import EqPanel from "../components/EqPanel";
import Toasts from "../components/Toasts";

export default function EqWindow() {
  useLanguage();
  const connected = useStore((s) => s.snapshot != null);

  useEffect(() => {
    const stopBridge = startBridge({ waveforms: false });
    const stopWindowSync = startEqWindowSync();
    const stopFocus = installFocusHygiene();
    /* This window follows the theme while it is open, including an OS change
       and a change made in the main window (SPEC §14): the appearance is in
       `settings.json`, Rust broadcasts `onyx://state` to every webview, and
       both windows adopt it independently. */
    const stopAppearance = startAppearanceSync();
    return () => {
      stopBridge();
      stopWindowSync();
      stopFocus();
      stopAppearance();
    };
  }, []);

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent): void => {
      // A composing IME owns `Space`, `Escape` and `e` until it commits
      // (`lib/ime.ts`); this window has a text field in it too.
      if (isComposing(e)) return;
      if (isTypingTarget(e.target)) return;
      if (e.metaKey || e.ctrlKey || e.altKey) return;

      if (e.key === " " || e.key === "Spacebar") {
        e.preventDefault();
        void api.transportToggle().catch((err) => {
          useStore.getState().pushToast("error", api.errorMessage(err));
        });
        return;
      }
      if (e.key === "Escape") {
        closeEqWindow();
        return;
      }
      if (e.key.toLowerCase() === "e") {
        if (e.shiftKey) {
          const eq = useStore.getState().snapshot?.eq;
          if (eq) {
            void api.setEq({ ...eq, enabled: !eq.enabled }).catch((err) => {
              useStore.getState().pushToast("error", api.errorMessage(err));
            });
          }
        } else {
          // `E` is a toggle everywhere; from in here the toggle means "close".
          closeEqWindow();
        }
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  return (
    <div className="eq-window">
      <EqPanel />
      <Toasts />
      {t(!connected && <div className="connecting">{t("connecting to engine")}</div>)}
    </div>
  );
}
