/**
 * Entry point for `theme.html` — the theme editor window (SPEC §20).
 *
 * The third document in the same bundle, alongside `src/main.tsx` and
 * `src/eq/main.tsx`. It shares `src/lib/*` with them, so the editor cannot
 * drift from the app on what a theme document means, what the IPC contract is,
 * or what the design tokens are called. It loads its own stylesheet: none of
 * the main window's layout means anything here.
 *
 * Two lines differ from the EQ window's entry point, and they are the whole
 * point of this file:
 *
 *   ignoreThemeDoc()  — this window follows the appearance but never wears the
 *                       token document being edited, so a theme that paints
 *                       every surface the same colour cannot take away the
 *                       window you undo it from;
 *   setResetHook(...) — the escape hatch chord works in every window; here, as
 *                       in the others, the local repaint is immediate and the
 *                       backend is told afterwards so the reset survives a
 *                       relaunch.
 */

import React from "react";
import ReactDOM from "react-dom/client";
import ThemeWindow from "./ThemeWindow";
import ErrorBoundary from "../components/ErrorBoundary";
import * as api from "../lib/api";
import { MOCK } from "../lib/api";
import { installDiagnostics, logWarn } from "../lib/log";
import * as theme from "../lib/theme";
import {
  applyAppearance,
  applyPreviewOverride,
  ignoreThemeDoc,
  initAppearance,
  setResetHook,
} from "../lib/theme";
import "../styles/tokens.css";
import "../styles/theme-editor.css";

// Before the tree mounts, exactly as in the other two windows: a failure
// during the first render of a window with no devtools is otherwise invisible.
installDiagnostics();

/* The cached appearance first, so this window does not flash obsidian before
   the snapshot lands — then `ignoreThemeDoc()`, which drops the document out
   of *this* window's token layer while leaving it on record for the editor to
   load. Order matters: `initAppearance()` adopts the cached document, and the
   call below is what stops it from being rendered here. */
initAppearance();
ignoreThemeDoc();
applyPreviewOverride(MOCK);

/* `Ctrl/Cmd+Alt+Shift+R` repaints this window immediately (that part is in
   `theme.ts`, at the capture phase, and does not depend on React); this is the
   half that makes it stick. A failure is logged, not toasted: the user pressed
   this because something was already wrong. */
setResetHook(() => {
  void api
    .resetAppearance()
    .then((appearance) => applyAppearance(appearance))
    .catch((err) => logWarn("could not persist the appearance reset", err));
});

/* Mock preview only: the same handle the main window publishes, so
   `scripts/shots-theme.mjs` can read this window's token layer and prove it is
   *not* wearing the document. Gated on the build-time flag. */
if (MOCK) {
  (window as unknown as Record<string, unknown>).__onyxTheme = theme;
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ErrorBoundary>
      <ThemeWindow />
    </ErrorBoundary>
  </React.StrictMode>,
);
