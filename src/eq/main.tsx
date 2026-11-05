/**
 * Entry point for `eq.html` — the detached EQ window (SPEC §12).
 *
 * A second document in the same bundle, not a second app: it shares
 * `src/lib/*` with the main window and is code-split from it by Vite, so the
 * two windows cannot drift apart on the IPC contract, the curve maths or the
 * design tokens. It loads its own stylesheet rather than `app.css`: none of
 * the main window's layout — the title bar, the playlist grid, the transport —
 * means anything here.
 */

import React from "react";
import ReactDOM from "react-dom/client";
import EqWindow from "./EqWindow";
import ErrorBoundary from "../components/ErrorBoundary";
import * as api from "../lib/api";
import { MOCK } from "../lib/api";
import { installDiagnosticsBridge } from "../lib/diag";
import { installDiagnostics, logWarn } from "../lib/log";
import { applyAppearance, applyPreviewOverride, initAppearance, setResetHook } from "../lib/theme";
import "../styles/tokens.css";
import "../styles/eq.css";

// Before the tree mounts, exactly as in the main window: a failure during the
// first render of a window with no devtools is otherwise invisible.
installDiagnostics();

/* The same two calls as the main window, for the same reason: this webview has
   its own `<html>` and its own token layer, so it applies the theme itself. The
   cached appearance is what the main window last wrote, which is why opening
   the EQ window in the light theme does not flash obsidian first (SPEC §14),
   and it carries the theme document too (SPEC §20). `EqWindow` then keeps it
   in step through `startAppearanceSync()`. */
initAppearance();
applyPreviewOverride(MOCK);

/* The escape hatch works from *this* window as well: a theme document skins
   every webview, so the EQ window can be the unreadable one, and the chord
   must not require finding the main window first. `theme.ts` does the local
   repaint; this persists it (SPEC §20). */
setResetHook(() => {
  void api
    .resetAppearance()
    .then((appearance) => applyAppearance(appearance))
    .catch((err) => logWarn("could not persist the appearance reset", err));
});

/* Mock preview only, as in the main window: the EQ curve is a canvas too, and
   this window is where a stale painter would be least visible — the harness
   opens it as a popup and resizes it (`scripts/shots.mjs`). Dropped from a
   shipped build with the rest of `lib/diag.ts`. */
if (MOCK) {
  installDiagnosticsBridge();
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ErrorBoundary>
      <EqWindow />
    </ErrorBoundary>
  </React.StrictMode>,
);
