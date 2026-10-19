import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import ErrorBoundary from "./components/ErrorBoundary";
import * as api from "./lib/api";
import { MOCK } from "./lib/api";
import { installDiagnosticsBridge } from "./lib/diag";
import { installDiagnostics, logWarn } from "./lib/log";
import * as theme from "./lib/theme";
import {
  applyAppearance,
  applyPreviewOverride,
  initAppearance,
  onThemeChange,
  resolvedSurface,
  resolvedTheme,
  setResetHook,
} from "./lib/theme";
import "./styles/tokens.css";
import "./styles/app.css";

// Installed before the tree mounts so a failure during the first render is
// still recorded. Never uninstalled: it lives as long as the window does.
installDiagnostics();

/* Theme before the first paint (SPEC §14). The authoritative appearance
   arrives with the first snapshot — `App` calls `startAppearanceSync()` — but
   the last one this window saw is a far better first frame than "dark" for
   anyone who has chosen otherwise. A pasted theme document (SPEC §20) rides in
   the same cache, so the first frame carries it too; one that cannot be read
   is dropped here and reported by `App` rather than left to paint a broken
   window. */
initAppearance();
applyPreviewOverride(MOCK);

/* The escape hatch (SPEC §20). `theme.ts` binds `Ctrl/Cmd+Alt+Shift+R` at the
   capture phase, at module scope, in every window and before React exists, so
   it works even if the tree never mounted or a theme has made the UI
   invisible; the repaint is local and immediate. This hook is the other half:
   persist the reset, so the app does not come back wearing the same
   unreadable theme. Logged rather than toasted — a toast the user cannot read
   is not an answer. */
setResetHook(() => {
  void api
    .resetAppearance()
    .then((appearance) => applyAppearance(appearance))
    .catch((err) => logWarn("could not persist the appearance reset", err));
});

/* The *window* surface (SPEC §14). Everything above this line themes the webview;
   the window the webview sits in has a background colour of its own, and an
   untold one keeps the system's — never obsidian, never alabaster. It shows
   around the antialiased rounded corners of a decorated macOS window and for the
   frames before the first paint, which is the light rim this reports away.

   Only the main window reports. It is the one that always exists and always
   wears the theme document (the EQ window wears it too but is not always open;
   the editor deliberately never wears it, and Rust gives that window the
   designed colour instead). `onThemeChange` fires for a theme change, an OS
   appearance change under `system`, an applied or cleared document and the reset
   chord — every path that can move the colour — and the call below covers the
   appearance already applied at module scope. Failures are logged, not toasted:
   a window edge is not worth interrupting anyone for. */
const reportWindowSurface = (): void => {
  const color = resolvedSurface();
  if (!color) return;
  void api
    .setWindowSurface(color, resolvedTheme())
    .catch((err) => logWarn("could not set the window surface", err));
};
onThemeChange(reportWindowSurface);
reportWindowSurface();

/* Mock preview only: a handle for `scripts/shots-theme.mjs`, which has to be
   able to switch the theme from outside React to prove that nothing — canvas
   included — is left holding the old palette, and `window.__onyxDiag` for
   `scripts/shots.mjs`, which asserts that every canvas repaints at the *new*
   geometry after a resize and with the *new* material after a track change
   instead of photographing one and hoping. Gated on the build-time flag, so a
   shipped build exposes nothing and Rollup drops `lib/diag.ts` entirely. */
if (MOCK) {
  (window as unknown as Record<string, unknown>).__onyxTheme = theme;
  installDiagnosticsBridge();
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </React.StrictMode>,
);
