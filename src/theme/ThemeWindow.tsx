import { t, useLanguage } from "../lib/i18n";
/**
 * The theme editor window's root — SPEC §20.
 *
 * # Why this window exists at all
 *
 * The feature is "paste a theme document and watch the app change". The
 * failure mode of that feature is "paste a theme document and watch the app
 * become one flat rectangle". Every way out of that state which lives inside
 * the re-skinned window is, at that moment, part of the problem — so the way
 * out lives here: a separate `WebviewWindow` with native decorations
 * (`src-tauri/src/themewindow.rs`), a native *Appearance ▸ Reset Appearance*
 * menu item (`src-tauri/src/appmenu.rs`), and a **Revert** button in a window
 * that is never wearing the document it is editing (`ignoreThemeDoc()`).
 *
 * # What it does, in order
 *
 *  - `ignoreThemeDoc()` at module scope in `main.tsx`, before React: this
 *    window follows the appearance (dark/light, accent, fonts, size) but never
 *    the token layer of the document being edited;
 *  - `startBridge({ waveforms: false })` — the same bridge both other windows
 *    run, minus the waveform poller it has no lanes for. It is how
 *    `onyx://state` arrives, which is how this window learns that the document
 *    in force changed underneath it (another window applied one, the escape
 *    hatch fired, the native menu was used);
 *  - `startAppearanceSync()` — so the editor follows a dark/light change like
 *    every other window;
 *  - a two-key keyboard map. `Escape` closes the window, `Cmd/Ctrl+Enter`
 *    applies (that one belongs to the textarea and lives in `ThemeEditor`).
 *    The app-wide map of SPEC §4 is deliberately *not* installed: this window
 *    has no playlist, no decks and no A/B rail, and a text editor in which `k`
 *    seeks and `Delete` removes a track would be a trap. The escape hatch
 *    chord is bound in `theme.ts` at the capture phase in every window,
 *    including this one, and is the one exception.
 */

import { useEffect, useState } from "react";
import * as api from "../lib/api";
import ThemeEditor from "../components/ThemeEditor";
import Toasts from "../components/Toasts";
import { isTypingTarget } from "../lib/dom";
import { installFocusHygiene } from "../lib/focus";
import { isComposing } from "../lib/ime";
import { startBridge } from "../lib/frame";
import { logWarn } from "../lib/log";
import { useStore } from "../lib/store";
import {
  currentAppearance,
  onThemeDocProblem,
  startAppearanceSync,
  takeThemeDocProblem,
} from "../lib/theme";

export default function ThemeWindow() {
  useLanguage();
  const snapshot = useStore((s) => s.snapshot);
  const pushToast = useStore((s) => s.pushToast);
  const connected = snapshot != null;
  const [look, setLook] = useState(() => currentAppearance());

  useEffect(() => {
    const stopBridge = startBridge({ waveforms: false });
    const stopAppearance = startAppearanceSync();
    const stopFocus = installFocusHygiene();
    return () => {
      stopBridge();
      stopAppearance();
      stopFocus();
    };
  }, []);

  /* A saved document that could not be read is reported here as well as in the
     main window: this is the window where it can be fixed, and the editor is
     otherwise showing the *designed* default with no explanation of why the
     document the user pasted yesterday is gone. */
  useEffect(() => {
    const first = takeThemeDocProblem();
    if (first) pushToast("error", first);
    return onThemeDocProblem((why) => pushToast("error", why));
  }, [pushToast]);

  // The header read-out follows the appearance actually in force.
  useEffect(() => {
    setLook(currentAppearance());
  }, [snapshot?.appearance, snapshot?.themeDoc]);

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent): void => {
      /* Escape while an IME is composing cancels the *composition*, which is
         the input method's business and not this window's: closing the editor
         then would throw away the document being typed into it
         (`lib/ime.ts`). */
      if (isComposing(e)) return;
      if (e.key !== "Escape") return;
      // Escape inside the code box means "I am done typing", not "throw the
      // window away" — blur first, close on the second press.
      if (isTypingTarget(e.target)) {
        (e.target as HTMLElement).blur();
        return;
      }
      void api.themeWindowClose().catch((err) => logWarn("could not close the editor", err));
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  const inForce = snapshot?.themeDoc ? nameOf(snapshot.themeDoc) : null;

  return (
    <div className="theme-window">
      <div className="tw-head">
        <span className="tw-title">{t("Theme")}</span>
        <span className="tw-state">{t("in force:")}<b>{t(inForce ?? "Onyx (designed)")}</b> · {t(look.theme)} · {t(look.accent)}
        </span>
        <span className="spacer" />
        <span className="tw-state">{t("this window is never re-skinned by the document it is editing")}</span>
      </div>

      <div className="tw-body">
        <ThemeEditor />
      </div>

      <Toasts />
      {t(!connected && <div className="connecting">{t("connecting to engine")}</div>)}
    </div>
  );
}

/**
 * The document's own `"name"`, read out of the text without parsing it.
 *
 * A read-out, not a decision: the parse that matters happens in `themedoc.ts`
 * and this must never disagree with it in a way that costs anything, so a
 * document it cannot make sense of is reported as "a pasted theme" rather than
 * guessed at.
 */
function nameOf(text: string): string {
  const m = /"name"\s*:\s*"([^"\n]{1,64})"/.exec(text);
  return m ? m[1] : "a pasted theme";
}
