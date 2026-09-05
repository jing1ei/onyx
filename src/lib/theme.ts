/**
 * Theming — SPEC §14/§15.
 *
 * # What this module is
 *
 * The single place that turns an `Appearance` (theme, accent, fonts, size
 * scale) into something the DOM can render, and the single place canvas code
 * asks for a colour. Two designed themes, one token layer
 * (`src/styles/tokens.css`), no colour literals anywhere else.
 *
 * # The mechanism: data attributes on `document.documentElement`
 *
 * `applyAppearance()` writes, on `<html>`:
 *
 *   data-theme       "dark" | "light"      the *resolved* theme, never "system"
 *   data-theme-pref  "dark" | "light" | "system"   what the user actually chose
 *   data-ui-font     a font token (see FONT tokens in tokens.css)
 *   data-num-font    a font token
 *   data-scale       "compact" | "normal" | "large"
 *
 * plus the accent family as custom properties (`--accent`, `--accent-hi`,
 * `--accent-press`, `--accent-deep`, `--accent-ink`, the matching `-rgb`
 * triplets and `--deck-b`). Everything else is CSS: `tokens.css` declares one
 * block per theme and the whole app re-themes in one style recalculation.
 *
 * Nothing else in the front end may set a theme. `startAppearanceSync()`
 * mirrors `snapshot.appearance` for whichever window calls it, follows the OS
 * when the preference is `system`, and mirrors the last applied appearance into
 * `localStorage` so that
 *
 *  - a window paints the right theme on its *first* frame instead of flashing
 *    obsidian and then going light, and
 *  - the detached EQ window (SPEC §12) follows a theme change made in the main
 *    window even when the change is an OS one and its own webview was told to
 *    prefer dark.
 *
 * # Canvas
 *
 * A 2D context cannot read a CSS variable, so `paint()` resolves the token
 * layer explicitly, caches the result, and invalidates that cache on every
 * theme change. Painters that run inside the 60 Hz loop get the new palette on
 * their next frame for free; painters with a *static* layer (the waveform bars)
 * must subscribe with `onThemeChange()` — a stale canvas after a theme switch
 * is the bug this exists to prevent.
 */

import { accentFamily } from "./accent";
import type { Appearance, AppearanceLike, ResolvedTheme, SizeScale, ThemeSetting } from "./appearance";
import { DEFAULT_APPEARANCE, normaliseAppearance, sameAppearance } from "./appearance";
import type { Rgba } from "./color";
import { formatRgba, parseCssColor, parseHex, parseRgbTriplet, scaleAlpha } from "./color";
import { logError, logWarn } from "./log";
import { onSnapshot, useStore } from "./store";
import { SURFACE_TOKEN, surfaceHex } from "./surface";
import type { ThemeDoc } from "./themedoc";
import { inlineVars, parseTheme } from "./themedoc";

/* Re-exported so the rest of the app keeps one theming import — `theme.ts` is
   still the front door, the model just lives in a module Node can load. */
export { accentFamily, parseHex, DEFAULT_APPEARANCE, normaliseAppearance };
export type { Appearance, AppearanceLike, ResolvedTheme, SizeScale, ThemeSetting };

/**
 * Zoom factor per size scale. Applied as CSS `zoom` on `<html>`, which is the
 * only way to scale a design measured in px without rewriting every rule — and
 * it is *capped* so the layout can never be given less room than the 420 × 560
 * minimum it is designed for (SPEC §15: `large` must not break 420 px).
 */
const SCALE_ZOOM: Record<SizeScale, number> = {
  compact: 0.94,
  normal: 1,
  large: 1.08,
};
const MIN_W = 420;
const MIN_H = 560;


/* ── applying ─────────────────────────────────────────────────────────────── */

const STORAGE_KEY = "onyx.appearance";

let current: Appearance = DEFAULT_APPEARANCE;
let resolved: ResolvedTheme = "dark";
let installed = false;
/** Owns every window-level listener; aborted by `teardownAppearance()`. */
let listening: AbortController | null = null;

const listeners = new Set<(theme: ResolvedTheme) => void>();

function prefersDark(): boolean {
  try {
    return !window.matchMedia("(prefers-color-scheme: light)").matches;
  } catch {
    return true;
  }
}

/**
 * The OS scheme, or the one the *other* window resolved.
 *
 * The EQ window is a second webview and its native theme may be pinned, so its
 * own `prefers-color-scheme` cannot be trusted to answer "what is the OS doing
 * right now". A resolved value written by whichever window heard about the
 * change first wins for a few seconds; after that we are back to asking this
 * webview, which is right in the normal case.
 */
const RELAY_TTL_MS = 15_000;

function systemTheme(): ResolvedTheme {
  const cached = readCache();
  if (
    cached &&
    cached.theme === "system" &&
    cached.resolved &&
    Date.now() - (cached.at ?? 0) < RELAY_TTL_MS
  ) {
    return cached.resolved;
  }
  return prefersDark() ? "dark" : "light";
}

interface Cached extends AppearanceLike {
  resolved?: ResolvedTheme;
  at?: number;
  /** set by the mock preview's URL override, so a popup inherits it */
  override?: boolean;
  /** the theme document's source text (SPEC §20), so a second window and the
      first frame of a reload both get the pasted look and not a flash of the
      designed one */
  doc?: string | null;
}

function readCache(): Cached | null {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    return raw ? (JSON.parse(raw) as Cached) : null;
  } catch {
    return null;
  }
}

function writeCache(a: Appearance, r: ResolvedTheme): void {
  try {
    const payload: Cached = { ...a, resolved: r, at: Date.now(), doc: docText };
    if (override) payload.override = true;
    /* Only when something other than the clock changed. Two windows share this
       key and each one applies what the other writes (`installRelay`), so a
       write that carries nothing but a new timestamp is a `storage` event that
       causes another write — the two webviews would hand the same appearance
       back and forth for as long as they were both open. */
    const prev = readCache();
    if (prev && sameCache(prev, payload)) return;
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(payload));
  } catch {
    /* private mode, or no storage in this webview: the theme still applies */
  }
}

const sameCache = (a: Cached, b: Cached): boolean =>
  a.theme === b.theme &&
  a.accent === b.accent &&
  a.uiFont === b.uiFont &&
  a.numericFont === b.numericFont &&
  a.sizeScale === b.sizeScale &&
  a.resolved === b.resolved &&
  !!a.override === !!b.override &&
  (a.doc ?? null) === (b.doc ?? null);

/** Zoom, capped so the 420 × 560 layout floor survives every size scale. */
function applyZoom(root: HTMLElement, scale: SizeScale): void {
  const want = SCALE_ZOOM[scale];
  const w = window.innerWidth || MIN_W;
  const h = window.innerHeight || MIN_H;
  const zoom = Math.min(want, Math.max(1, w / MIN_W), Math.max(1, h / MIN_H));
  root.style.setProperty("--zoom", String(zoom));
  // `zoom` rather than a transform: it scales layout, so line lengths, hit
  // targets and the breakpoint ladder all stay consistent with each other.
  if (zoom === 1) root.style.removeProperty("zoom");
  else root.style.setProperty("zoom", String(zoom));
}

/**
 * Apply an appearance. Idempotent, synchronous, and safe to call as often as a
 * settings panel likes — this is the function the settings UI calls for a live
 * preview, with the value it is about to send to `set_appearance`.
 */
export function applyAppearance(input: AppearanceLike | null | undefined): Appearance {
  const next = normaliseAppearance(input);
  const root = document.documentElement;
  const theme: ResolvedTheme = next.theme === "system" ? systemTheme() : next.theme;

  root.dataset.themePref = next.theme;
  root.dataset.uiFont = next.uiFont;
  root.dataset.numFont = next.numericFont;
  root.dataset.scale = next.sizeScale;
  // last, so the accent family below is derived against the theme that is
  // actually in force
  root.dataset.theme = theme;

  /* Everything is computed before anything is written. `inlineVars` cannot
     throw on a validated document, but the ordering is the contract: an apply
     either replaces the whole inline layer or touches none of it (SPEC §20).

     `docIgnored` is the theme editor's window: it *knows* which document is in
     force (the editor shows it) but never wears it, so a document that paints
     every surface the same colour cannot take away the window you undo it
     from. See `ignoreThemeDoc()`. */
  const vars = inlineVars(docIgnored ? null : doc, theme, next);
  const changed =
    theme !== resolved ||
    !installed ||
    !sameAppearance(current, next) ||
    !sameVars(written, vars);

  for (const name of written.keys()) {
    if (!vars.has(name)) root.style.removeProperty(`--${name}`);
  }
  for (const [name, value] of vars) root.style.setProperty(`--${name}`, value);
  written = vars;
  applyZoom(root, next.sizeScale);

  current = next;
  resolved = theme;
  /* The cache is "the last appearance this app applied", which is what makes
     the *other* window follow along (`installRelay`) — including a live preview
     from the settings panel, so the EQ window previews with it. */
  writeCache(next, theme);
  invalidatePaint();
  /* Not only on a dark/light switch: a new accent, a new size scale or a new
     theme document all change what `paint()` returns, and a canvas with a
     static layer (the waveform bars) would otherwise keep the old palette
     until the next resize. */
  if (changed) for (const fn of listeners) fn(theme);
  return next;
}

function sameVars(a: Map<string, string>, b: Map<string, string>): boolean {
  if (a.size !== b.size) return false;
  for (const [k, v] of a) if (b.get(k) !== v) return false;
  return true;
}

/* ── the theme document (SPEC §20) ────────────────────────────────────────
   The document is the *inline* layer over the stylesheet: `tokens.css` still
   states the two designed themes, and a document overrides some of what they
   say. That is what makes a partial theme work, what makes reverting a matter
   of removing properties, and what stops a document from having to be complete
   to be valid. */

let doc: ThemeDoc | null = null;
/** The document's source text, mirrored to the other window through the cache. */
let docText: string | null = null;
/** The token names this module last wrote, so the next apply can remove them. */
let written: Map<string, string> = new Map();
/** Set when a persisted document could not be read, for the UI to report. */
let docProblem: string | null = null;
/** The editor window keeps the document on record but never renders it. */
let docIgnored = false;

const docProblemListeners = new Set<(why: string) => void>();

/** The document in force in this window, if any. */
export const currentThemeDoc = (): ThemeDoc | null => doc;

/** Its source text — what the editor shows and what the backend persists. */
export const currentThemeText = (): string | null => docText;

/**
 * Render this window in the designed themes whatever document is in force.
 *
 * Called once, at module scope, by the theme editor window and by nothing
 * else. The document is still tracked — `currentThemeText()` is what the
 * editor loads into its box — it is simply never written to `<html>` here, so
 * the window you recover from a bad theme in cannot be eaten by that theme.
 *
 * The *appearance* (dark/light, accent, fonts, size) still applies: it is five
 * validated values, not an arbitrary token layer, and an editor that ignored
 * the user's light theme would be its own kind of wrong.
 */
export function ignoreThemeDoc(): void {
  docIgnored = true;
  applyAppearance(current);
}

/** Is this window deliberately not wearing the document? (Tests, diagnostics.) */
export const themeDocIgnored = (): boolean => docIgnored;

/**
 * Be told when a saved document had to be dropped, so the UI can say so.
 *
 * The first one usually happens before React exists (`initAppearance()` runs
 * at module scope), which is what [`takeThemeDocProblem`] is for; later ones
 * arrive with a snapshot, and those need a listener.
 */
export function onThemeDocProblem(fn: (why: string) => void): () => void {
  docProblemListeners.add(fn);
  return () => docProblemListeners.delete(fn);
}

/** Why the persisted document was ignored, once, for the toast that says so. */
export function takeThemeDocProblem(): string | null {
  const p = docProblem;
  docProblem = null;
  return p;
}

/**
 * Install a validated document (or `null` for "the designed themes") and
 * repaint, optionally moving the appearance the document asked for in the same
 * pass.
 *
 * Atomic: the caller has already validated the whole document, and the apply
 * below computes every custom property before it writes any of them, so a
 * theme either lands completely or not at all (SPEC §20).
 */
export function applyThemeDoc(
  next: ThemeDoc | null,
  text: string | null,
  appearance?: AppearanceLike,
): Appearance {
  doc = next;
  docText = next ? text : null;
  return applyAppearance(appearance ?? current);
}

/**
 * Read a persisted document. A corrupt one is *not* fatal and is never
 * partially applied: the app falls back to the designed themes and remembers
 * why, so the window comes up looking like Onyx rather than not coming up
 * (SPEC §20).
 */
function adoptDocText(text: string | null | undefined, source: string): boolean {
  if (typeof text !== "string" || text.trim() === "") {
    const had = doc !== null;
    doc = null;
    docText = null;
    return had;
  }
  if (text === docText) return false;
  const outcome = parseTheme(text);
  if (!outcome.doc) {
    const why = outcome.problems.find((p) => p.level === "error");
    docProblem =
      `The saved theme could not be read (${source}): ` +
      `${why ? `line ${why.line}: ${why.message}` : "invalid"}. ` +
      "Onyx is using its built-in appearance.";
    logWarn(docProblem);
    for (const fn of docProblemListeners) {
      try {
        fn(docProblem);
      } catch {
        /* a listener that throws must not cost the fallback */
      }
    }
    const had = doc !== null;
    doc = null;
    docText = null;
    return had;
  }
  doc = outcome.doc;
  docText = text;
  return true;
}

export const currentAppearance = (): Appearance => current;
export const resolvedTheme = (): ResolvedTheme => resolved;

/**
 * The colour this window is painting its outermost pixels with, as `#rrggbb`.
 *
 * The native window is painted the same colour underneath the webview
 * (`src-tauri/src/surface.rs`): an untold window keeps the system's own
 * background, which shows as a light rim around a dark window's rounded corners
 * and as a grey flash before the first frame. Rust knows the two designed
 * themes, but only this side knows what a theme document (SPEC §20) resolved to,
 * so this is what `src/main.tsx` reports after every apply.
 *
 * `body`'s computed `background-color` first, because that is literally what is
 * being painted (`tokens.css` sets it from the surface token, and the engine has
 * already substituted every `var()` by the time it is read); the token itself as
 * the fallback, for a window whose body has not been styled yet. `null` means
 * "nothing worth reporting" — Rust then keeps the designed colour, which is the
 * right answer rather than a guess.
 */
export function resolvedSurface(): string | null {
  try {
    const root = document.documentElement;
    return surfaceHex([
      document.body ? getComputedStyle(document.body).backgroundColor : null,
      getComputedStyle(root).getPropertyValue(`--${SURFACE_TOKEN}`),
    ]);
  } catch {
    /* no computed style in this environment: the window keeps the designed one */
    return null;
  }
}

/** Repaint hook for canvases with a layer that is not redrawn every frame. */
export function onThemeChange(fn: (theme: ResolvedTheme) => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

/**
 * Apply the last known appearance before the first paint.
 *
 * Called from both entry points (`src/main.tsx`, `src/eq/main.tsx`) at module
 * scope. The authoritative value arrives later over IPC; until it does, the one
 * this window last saw is a far better guess than "dark", which is what makes
 * opening the EQ window in the light theme not flash.
 */
export function initAppearance(): Appearance {
  const cached = readCache();
  /* The cached values are used, the cached *override flag* is not applied here:
     honouring it would make a mock preview's `?theme=…` sticky for the lifetime
     of the browser profile — every later load would ignore `snapshot.appearance`
     and the settings panel would look like it had stopped persisting. It is only
     remembered, because the write below drops it, and `applyPreviewOverride()`
     needs it to let a popup inherit its opener's override. */
  inherited = !!cached?.override;
  /* Before the first apply, so the first frame carries the pasted theme too.
     A corrupt one is dropped here and reported by `takeThemeDocProblem()`. */
  adoptDocText(cached?.doc, "saved in this window");
  const applied = applyAppearance(cached ?? DEFAULT_APPEARANCE);
  if (!installed) {
    installed = true;
    /* Every window-level listener this module installs is owned by one
       controller, so `teardownAppearance()` can take all of them off again in
       one call. They used to be installed and never removed — the `resize`
       listener in particular, which was passed as an anonymous function no one
       kept a handle to, so a webview that came and went (the EQ window, the
       editor, a preview popup, a dev-server hot reload) left a listener behind
       holding this module's closure. */
    listening = new AbortController();
    const { signal } = listening;
    installOsWatch(signal);
    installRelay(signal);
    installEscapeHatch(signal);
    window.addEventListener("resize", onResize, { signal });
    /* A hot update replaces this module with a new copy whose `installed` latch
       starts false, so without this a dev session accumulates one set of
       listeners per edit — and the old copies keep applying zoom from their own
       stale `current`. Vite strips `import.meta.hot` out of a production build,
       so nothing ships. */
    (import.meta as ImportMetaWithHot).hot?.dispose(teardownAppearance);
  }
  return applied;
}

/** The sliver of Vite's HMR API used above; `undefined` in a built bundle. */
interface ImportMetaWithHot {
  hot?: { dispose(cb: () => void): void };
}

/** The `resize` handler, named so it can be removed and reasoned about. */
function onResize(): void {
  applyZoom(document.documentElement, current.sizeScale);
}

/**
 * Drop every window-level listener `initAppearance()` installed.
 *
 * The counterpart of the `installed` latch, and the reason it is a latch rather
 * than a one-way door: a window that is going away — the EQ window closing,
 * the editor window closing, a Vite hot update replacing this module — hands
 * the listeners back instead of leaving them attached to a document nobody
 * will paint again. Calling it twice, or before `initAppearance()`, is a no-op,
 * and a later `initAppearance()` installs a fresh set.
 */
export function teardownAppearance(): void {
  listening?.abort();
  listening = null;
  installed = false;
}

/* ── the escape hatch (SPEC §20) ──────────────────────────────────────────
   A theme document can make every surface the same colour. When that happens
   the settings panel is still there and still clickable — and invisible. So the
   way out cannot live in the themed UI:

     · this key handler, bound at the *capture* phase on `window` in every
       window, at module scope, before React mounts and regardless of whether it
       mounted at all. A theme cannot intercept a key, and this one is not
       rebindable, not in `keys.ts`'s table and not swallowed by a focused
       text field;
     · the native Appearance ▸ Reset Appearance menu item, drawn by the window
       manager (`src-tauri/src/appmenu.rs`);
     · the theme editor window, which is never re-skinned by the document it is
       editing.

   All three end at the same place: `reset_appearance` in Rust, which clears the
   document *and* the appearance in one write, plus the local apply below so the
   window is legible again in the same frame rather than one IPC round trip
   later. */

/** `Ctrl`/`Cmd` + `Alt` + `Shift` + `R`. Matches `appmenu::RESET_ACCELERATOR`. */
export const isResetChord = (e: KeyboardEvent): boolean =>
  (e.ctrlKey || e.metaKey) && e.altKey && e.shiftKey && (e.key === "R" || e.key === "r");

type ResetHook = () => void;
let onReset: ResetHook | null = null;

/**
 * Register what to do *besides* repainting — persisting the reset through the
 * backend. Set once per window by `main.tsx` / `eq/main.tsx`; the local repaint
 * below happens with or without it, so the chord still works in a window whose
 * IPC is wedged.
 */
export function setResetHook(fn: ResetHook | null): void {
  onReset = fn;
}

/** Drop the document and go back to the designed appearance, right now. */
export function resetAppearanceLocally(): Appearance {
  doc = null;
  docText = null;
  const applied = applyAppearance(DEFAULT_APPEARANCE);
  return applied;
}

function installEscapeHatch(signal: AbortSignal): void {
  window.addEventListener(
    "keydown",
    (e) => {
      if (!isResetChord(e)) return;
      /* Deliberately *not* guarded by `lib/ime.ts`, unlike every other key
         handler in the app: this is the way out of a theme that has made the UI
         invisible, and it has to work while a candidate window is open. A
         three-modifier chord is not part of any composition, so nothing is
         taken from the input method by letting it through. */
      e.preventDefault();
      e.stopPropagation();
      resetAppearanceLocally();
      try {
        onReset?.();
      } catch (err) {
        logWarn("the appearance reset could not be persisted", err);
      }
    },
    // Capture, so a component that swallows keydown — the editor's textarea,
    // a modal — cannot swallow this one.
    { capture: true, signal },
  );
}

function installOsWatch(signal: AbortSignal): void {
  try {
    const query = window.matchMedia("(prefers-color-scheme: light)");
    query.addEventListener("change", () => {
      if (current.theme !== "system") return;
      // this webview heard it first: relay the resolved value for the other one
      const next = prefersDark() ? "dark" : "light";
      writeCache(current, next);
      applyAppearance(current);
    }, { signal });
  } catch {
    /* no matchMedia: `system` degrades to whatever the webview reports once */
  }
}

/** The other window changed the theme, pasted a document, or resolved a new OS scheme. */
function installRelay(signal: AbortSignal): void {
  window.addEventListener("storage", (e) => {
    if (e.key !== STORAGE_KEY) return;
    const cached = readCache();
    if (!cached) return;
    /* The document travels with the appearance: applying a theme in the main
       window has to re-skin the EQ window, and in a mock build this relay is
       the only channel there is. `undefined` (an older cache) leaves the
       document alone; `null` removes it. */
    if (cached.doc !== undefined) adoptDocText(cached.doc, "the other window");
    applyAppearance(cached);
  }, { signal });
}

/* ── keeping up with the engine ───────────────────────────────────────────── */

/**
 * Mirror `snapshot.appearance` into the DOM, for whichever window calls it.
 *
 * Both windows do: the appearance lives in `settings.json`, Rust broadcasts
 * `onyx://state` to every webview, so the EQ window re-themes with the main one
 * without either knowing the other exists (SPEC §14).
 *
 * A snapshot with no appearance block is *not* an instruction to go back to the
 * defaults — an older backend, or a mock that does not model settings yet, must
 * not silently un-theme the window.
 */
export function startAppearanceSync(): () => void {
  const adopt = (snap: unknown): void => {
    const state = snap as { appearance?: AppearanceLike; themeDoc?: string | null } | null;
    const a = state?.appearance;
    if (!a) return;
    if (override) return;
    /* `undefined` means "this backend does not carry a theme document" and is
       not an instruction to drop the one this window is showing; `null` and
       `""` are. */
    if (state?.themeDoc !== undefined) adoptDocText(state.themeDoc, "saved settings");
    applyAppearance(a);
    writeCache(current, resolved);
  };
  adopt(useStore.getState().snapshot);
  return onSnapshot(adopt);
}

/* ── preview override (mock builds only) ──────────────────────────────────
   `?theme=light&accent=%23b0652a&scale=large` on the mock preview URL. It is
   how `scripts/shots-theme.mjs` photographs a theme without a settings back
   end, and it is gated on the build-time mock flag so a shipped build cannot be
   re-themed by a URL. */

let override: Appearance | null = null;
/** the override flag this window found in the cache when it started up */
let inherited = false;

export function applyPreviewOverride(mock: boolean): Appearance | null {
  if (!mock) return null;
  let params: URLSearchParams;
  try {
    params = new URLSearchParams(window.location.search);
  } catch {
    return null;
  }
  const has = ["theme", "accent", "scale", "uiFont", "numFont"].some((k) => params.has(k));
  if (!has) {
    /* A popup (the mock's EQ window) has no query string of its own, so it
       inherits the opener's override — the flag `initAppearance()` saw in the
       cache. Only a popup: a plain reload has to go back to being driven by
       `snapshot.appearance`, which is the seam this file exists to keep honest. */
    const cached = readCache();
    if (inherited && cached && typeof window.opener === "object" && window.opener !== null) {
      override = normaliseAppearance(cached);
      return applyAppearance(override);
    }
    return null;
  }
  const next = normaliseAppearance({
    ...(readCache() ?? DEFAULT_APPEARANCE),
    theme: params.get("theme") ?? undefined,
    accent: params.get("accent") ?? undefined,
    sizeScale: params.get("scale") ?? undefined,
    uiFont: params.get("uiFont") ?? undefined,
    numericFont: params.get("numFont") ?? undefined,
  });
  override = next;
  const applied = applyAppearance(next);
  writeCache(applied, resolved);
  return applied;
}

/* ── the canvas palette ───────────────────────────────────────────────────── */

/**
 * Every token a canvas paints with. Listed rather than free-form so a typo is a
 * type error, and so `paint()` can prove at startup that the token layer
 * actually declares all of them.
 */
export const PAINT_TOKENS = [
  /* waveform bar geometry: the one part of the waveform's shape that is
     themeable (SPEC §20) */
  "--wf-bar-step",
  "--wf-bar-duty",
  /* the two font stacks: a canvas cannot inherit a font either, so the §15
     font choices have to be resolved the same way a colour is */
  "--font-num",
  "--font-ui",
  /* waveform lanes */
  "--wf-mid",
  "--wf-skeleton",
  "--wf-scrim",
  "--wf-gap-fill",
  "--wf-gap-line",
  "--wf-gap-text",
  "--wf-hover-line",
  "--wf-playhead-idle",
  "--wf-outer-a",
  "--wf-core-a",
  "--wf-cap-a",
  "--wf-glow-a",
  "--wf-decode-a",
  "--wf-playhead-a",
  "--wf-blend",
  "--lane-a-rgb",
  "--lane-b-rgb",
  "--lane-masked-rgb",
  "--loop-fill",
  "--loop-edge",
  "--loop-grip",
  "--align-chip",
  "--align-chip-ink",
  "--align-line",
  /* tooltips drawn on canvas */
  "--tip-bg",
  "--tip-line",
  "--tip-text",
  /* level meter */
  "--mt-track",
  "--mt-grid",
  "--mt-grid-zero",
  "--mt-label",
  "--mt-label-zero",
  "--mt-rms",
  "--mt-chan",
  "--mt-safe",
  "--mt-safe-lo",
  "--mt-warn",
  "--mt-hot",
  "--mt-clip",
  "--m-safe",
  "--m-warn",
  "--m-hot",
  "--m-clip",
  /* loudness strip */
  "--lu-track",
  "--lu-safe",
  "--lu-warn",
  "--lu-hot",
  "--lu-clip",
  "--lu-target",
  "--lu-target-label",
  "--lu-short",
  "--lu-int",
  /* correlation */
  "--corr-track",
  "--corr-neg",
  "--corr-mid",
  "--corr-pos",
  "--corr-centre",
  "--corr-needle-neg",
  "--corr-needle-pos",
  /* EQ curve */
  "--eq-grid-minor",
  "--eq-grid-major",
  "--eq-grid-zero",
  "--eq-freq-label",
  "--eq-db-label",
  "--eq-spec-top",
  "--eq-spec-bottom",
  "--eq-spec-edge",
  "--eq-spec-peak",
  "--eq-band",
  "--eq-band-hot",
  "--eq-curve",
  "--eq-curve-off",
  "--eq-fill-top",
  "--eq-fill-bottom",
  "--eq-fill-off-top",
  "--eq-fill-off-bottom",
  "--eq-node-glow",
  "--eq-node-fill",
  "--eq-node-fill-off",
  "--eq-node-ring",
  "--eq-node-ring-off",
  "--eq-tip-line",
  "--solo-band",
  "--solo-line",
  "--solo-chip",
  "--solo-chip-ink",
] as const;

export type PaintToken = (typeof PAINT_TOKENS)[number];

export interface Paint {
  /** bumped on every theme change; a painter can cache derived work per version */
  version: number;
  light: boolean;
  /** the token's colour, exactly as the theme declares it */
  color(token: PaintToken): string;
  /** the token's raw value — for the keyword tokens (`--wf-blend`) */
  raw(token: PaintToken): string;
  /** the token's value as a number — the bar-ink levels */
  num(token: PaintToken): number;
  /**
   * A colour token at an alpha of your choosing — the waveform bars' ink.
   *
   * Written for the `--*-rgb` triplet tokens, but any colour syntax works: the
   * alpha multiplies whatever the token already had, so a translucent token
   * stays translucent.
   */
  tint(token: PaintToken, alpha: number): string;
  /** the same thing, named for the travelling gradients that scale an alpha */
  fade(token: PaintToken, scale: number): string;
  /** `ctx.font` for a canvas read-out, in the user's numeric stack */
  font(px: number, weight?: string): string;
  /** `ctx.font` for a canvas label, in the user's UI stack */
  fontUi(px: number, weight?: string): string;
  /** the blend mode this theme's canvases add light with */
  blend(): GlobalCompositeOperation;
}

let paintVersion = 0;
let values: Map<string, string> | null = null;
let parsed: Map<string, Rgba | null> = new Map();
let missingReported = false;
/** tokens the canvas could not read; reported once each, not once a frame */
const unreadable = new Set<string>();

function invalidatePaint(): void {
  paintVersion += 1;
  values = null;
  parsed = new Map();
  // A new theme gets a fresh chance to be complained about: the token that was
  // unreadable may be exactly the one this document fixed.
  unreadable.clear();
}

function resolveAll(): Map<string, string> {
  const style = getComputedStyle(document.documentElement);
  const map = new Map<string, string>();
  const missing: string[] = [];
  for (const token of PAINT_TOKENS) {
    const v = style.getPropertyValue(token).trim();
    if (!v) missing.push(token);
    map.set(token, v);
  }
  if (missing.length > 0 && !missingReported) {
    missingReported = true;
    // Not a throw: a missing token must cost one surface its colour, not the
    // window. It is a bug in `tokens.css` and it should be loud in the log.
    logError(`theme tokens missing from the token layer: ${missing.join(", ")}`);
  }
  return map;
}

/**
 * A token's colour with its alpha scaled — the one place the canvas turns a
 * theme value into something `ctx.fillStyle` will take.
 *
 * The parse is `color.ts`'s, so every syntax the theme editor accepts works
 * here: `oklch()` and `hsl()` used to fall through a hex/rgb-only parser and
 * come back **opaque**, which painted a solid block wherever a gradient was
 * supposed to fade to nothing, and a triplet token pointed at a colour
 * (`--lane-a-rgb: var(--accent)`, which the validator allows) produced
 * `rgba(#e8d9a0, 0.46)` — not a colour at all, so the canvas silently kept the
 * previous fill. Parsed once per theme version and cached: this runs per frame.
 */
function withAlpha(token: PaintToken, scale: number): string {
  const raw = palette.raw(token);
  if (!raw) return "#000";
  if (!parsed.has(token)) parsed.set(token, parseRgbTriplet(raw) ?? parseCssColor(raw));
  const c = parsed.get(token);
  if (!c) {
    // Nothing in `PAINT_TOKENS` can legally be a gradient, so this is a bug in
    // a theme value or in the grammar — report it once and keep the surface
    // painted in *something* rather than dropping the draw call.
    if (!unreadable.has(token)) {
      unreadable.add(token);
      logError(`the canvas cannot read ${token}: "${raw}"`);
    }
    return raw;
  }
  return formatRgba(scaleAlpha(c, scale));
}

const palette: Paint = {
  get version() {
    return paintVersion;
  },
  get light() {
    return resolved === "light";
  },
  raw(token) {
    if (!values) values = resolveAll();
    return values.get(token) ?? "";
  },
  color(token) {
    // The resolved string goes straight to the context: a 2D canvas parses
    // every CSS colour syntax a custom property can hold, so there is nothing
    // to convert in the common case.
    return palette.raw(token) || "#000";
  },
  num(token) {
    const v = parseFloat(palette.raw(token));
    return Number.isFinite(v) ? v : 0;
  },
  tint(token, alpha) {
    return withAlpha(token, alpha);
  },
  fade(token, scale) {
    return withAlpha(token, scale);
  },
  font(px, weight) {
    const stack = palette.raw("--font-num") || "monospace";
    return `${weight ? `${weight} ` : ""}${px}px ${stack}`;
  },
  fontUi(px, weight) {
    const stack = palette.raw("--font-ui") || "sans-serif";
    return `${weight ? `${weight} ` : ""}${px}px ${stack}`;
  },
  blend() {
    // `lighter` on obsidian, `multiply` on paper: adding light to a bright
    // field is how a light theme ends up looking like fog.
    const v = palette.raw("--wf-blend");
    return (v || "source-over") as GlobalCompositeOperation;
  },
};

/** The canvas palette for the theme in force. Cheap; call it every frame. */
export const paint = (): Paint => palette;
