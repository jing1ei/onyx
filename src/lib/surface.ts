/**
 * The window surface — the one colour the *window manager* paints, SPEC §14.
 *
 * # Why this exists at all
 *
 * A native window has a background colour of its own, underneath the webview.
 * Left untold it is the system's (`windowBackgroundColor` on macOS), which is
 * never one of Onyx's themes, and it shows: around the antialiased rounded
 * corners of a decorated window, and for the frames between "the window is on
 * screen" and "the webview has painted". On obsidian that reads as a light rim
 * around the app. `src-tauri/src/surface.rs` paints it instead, and it needs to
 * know what colour to use.
 *
 * Rust can read the two *designed* themes — it parses `tokens.css` at compile
 * time — but not a theme document (SPEC §20), which can move the base surface
 * to anything a colour grammar allows. What a document means is decided here,
 * in one implementation, so the colour is resolved here and reported over IPC
 * (`api.setWindowSurface`).
 *
 * No DOM in this module, so `scripts/check-theme.mjs` can drive it in Node
 * against the same `effectiveVars()` the contrast audit uses. The DOM half is
 * `resolvedSurface()` in `theme.ts`, which is three lines of
 * `getComputedStyle`.
 */

import { parseCssColor } from "./color";

/**
 * The token every window's base surface is painted with.
 *
 * `tokens.css` ends `body { background: var(--ink-900) }`, and both `--bg-app`
 * and `--bg-eq` are gradients *over* it, so this is the colour at the edges of
 * all three documents. `src-tauri/src/surface.rs` names the same token and a
 * Rust test holds the two together.
 */
export const SURFACE_TOKEN = "ink-900";

/**
 * `#rrggbb` for the first candidate that is a colour, or `null` if none is.
 *
 * The candidates are tried in order of authority: what the document is really
 * painting first, the token second. A fully transparent value is skipped rather
 * than accepted — `transparent` is what a browser reports for "no background
 * here", which says nothing about what the window should be, and painting it as
 * black would be a guess.
 *
 * Opaque on purpose: the window surface is what everything else is composited
 * over, so an alpha on it has nothing to blend with. A translucent base surface
 * therefore contributes its colour and drops its alpha, which is the closest
 * true statement available.
 */
export function surfaceHex(candidates: Iterable<string | null | undefined>): string | null {
  for (const candidate of candidates) {
    if (typeof candidate !== "string") continue;
    const raw = candidate.trim();
    if (raw === "") continue;
    const c = parseCssColor(raw);
    if (!c || c.a === 0) continue;
    return `#${[c.r, c.g, c.b].map(hex).join("")}`;
  }
  return null;
}

const hex = (v: number): string =>
  Math.max(0, Math.min(255, Math.round(v)))
    .toString(16)
    .padStart(2, "0");
