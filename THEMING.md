# Theming — the contract

Two audiences, one file.

* **If you are an agent (or a person) writing an Onyx theme**, read Part 1. It
  is the whole contract: every key, every value type, every range, what each
  token visibly controls, a worked example, and exactly how a document fails.
  Paste this file next to the theme document you were given and you have
  everything Onyx will check.
* **If you are changing the theme engine**, Part 2 is the interface: what the
  front end may call, how canvases get a palette, and where the seams are.

Implemented in `src/lib/theme.ts`, `src/lib/themedoc.ts`, `src/lib/tokens.ts`,
`src/lib/cssvalue.ts`, `src/lib/contrast.ts` and `src/styles/tokens.css`.
Contract-level rules: SPEC §14 (theming), §15 (the six pickers), §20 (the theme
document).

---

# Part 1 — The theme document

## What Onyx is, in one paragraph

Onyx is a desktop audio player for mixing and mastering: a playlist, two decks
(A and B) for level-matched A/B and blind comparison, waveform lanes, level and
loudness meters, a correlation meter, and an interactive EQ curve in a window of
its own. It is used for hours at a time, often in a dark room, next to a DAW.
Its two designed themes are **dark** (obsidian and champagne) and **light**
(warm alabaster paper and aged bronze) — two designs, not one inverted. A theme
is judged by whether numbers, waveforms and meters can be read at a glance while
someone is working, not by whether a screenshot looks striking.

## The workflow

1. In Onyx: **Settings → Appearance → Theme code → Copy for agent**. That puts
   the current look on the clipboard as a commented JSONC document (about 420
   lines, 172 tokens) with a short brief in front of it.
2. Give it to a model with one sentence about what you want.
3. Paste the reply back into the same box and press **Apply** (`⌘/Ctrl+Enter`).

The reply may be a whole document or a partial one. It may keep or rewrite the
comments. It may still be wrapped in a Markdown code fence. All of that is read.

## The shape of a document

```jsonc
{
  "onyx": "theme",       // the marker. Missing → a warning, not an error
  "version": 1,          // the document format. > 1 is an error on this build
  "name": "Cold Graphite",   // ≤ 64 characters, cosmetic

  "appearance": { … },   // the six user settings; every field optional
  "base":  { … },        // tokens that are the same in both themes
  "dark":  { … },        // tokens for the dark theme
  "light": { … }         // tokens for the light theme
}
```

* **Those seven keys and no others.** Anything else is an error with a line
  number and a "did you mean".
* **Every block is optional, and every block is partial.** A document with four
  tokens in it is a valid document; everything it does not state keeps the
  designed value. Delete what you do not want to change rather than restating
  it — a smaller document is easier to read and easier to undo.
* **Both themes exist all the time.** The user can switch at any moment, and
  `system` follows the OS. Edit both unless you were told otherwise; a document
  that only touches `dark` leaves the light theme stock, which is a legitimate
  choice but rarely the intended one.
* Token keys may be written with or without the `--` prefix (`"text-hi"` and
  `"--text-hi"` are the same key). The export writes them without.

### What the reader accepts

JSON, plus: `//` and `/* */` comments, trailing commas, single-quoted strings,
unquoted identifier keys, and a leading/trailing Markdown code fence. Numbers
may be written bare where a string is expected (`"wf-core-a": 0.96`).

Bounds, because a pasted document is untrusted input: 256 KiB of source, nesting
depth 16, 2048 entries, 4096 characters per string. `__proto__`, `prototype` and
`constructor` are refused as keys.

## The `appearance` block

The same six settings the Simple tab exposes. Each is optional; a field you
state overrides the user's current choice when the theme is applied.

| key | legal values |
|---|---|
| `theme` | `"dark"` · `"light"` · `"system"` |
| `accent` | one hex colour, `#rgb` or `#rrggbb`, e.g. `"#4f8fbf"` |
| `uiFont` | `"system"` · `"grotesk"` · `"humanist"` · `"neutral"` |
| `numericFont` | `"system-mono"` · `"sf-mono"` · `"menlo"` · `"consolas"` · `"courier"` |
| `sizeScale` | `"compact"` · `"normal"` · `"large"` |

`accent` is *one* hex and Onyx derives the rest in OKLCH, per theme: hover,
pressed, deep, the ink that prints on an accent fill, and a hue-guarded deck B.
That is why the accent family is not in the exported document — see *Derived
tokens*.

To use a font that is not on those lists, do not invent a value here: set the
`font-ui` / `font-num` **tokens** in `"base"` instead. Only families the
platform already has — Onyx never fetches a webfont and the CSP forbids it.

## Value types

Every value is parsed into typed data and re-emitted by Onyx. Nothing you write
is treated as CSS text.

| type | what is accepted | example |
|---|---|---|
| `color` | `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, a CSS colour name, `rgb()`, `rgba()`, `hsl()`, `oklch()`, `transparent`, `var(--other-token)`, `rgb(var(--other-rgb) / 0.4)` | `"rgba(233, 240, 246, 0.94)"` |
| `rgb` | three 0–255 numbers separated by spaces, or `var(--other-rgb)` | `"91 201 143"` |
| `number` | a plain number, clamped to the token's range | `0.55` |
| `length` | px only | `"12px"` |
| `duration` | ms (`s` is accepted and converted) | `"200ms"` |
| `tracking` | em, px, or `0` | `"0.16em"` |
| `easing` | `linear`, `ease`, `ease-in`, `ease-out`, `ease-in-out`, `step-start`, `step-end`, `cubic-bezier(a,b,c,d)`, `steps(n)` | `"cubic-bezier(0.22, 1, 0.36, 1)"` |
| `font` | a family stack; letters, digits, spaces and hyphens only; ≤ 12 families, ≤ 48 characters each | `"Inter, Helvetica Neue, Arial, sans-serif"` |
| `background` | a colour, a gradient (`linear-`, `radial-`, `conic-gradient` and the repeating variants), or a comma-separated stack of them | `"linear-gradient(180deg, rgba(255,255,255,0.02), transparent)"` |
| `shadow` | up to four px lengths, a colour, optional `inset`; comma-separated for layers | `"0 24px 60px rgba(0,0,0,0.62)"` |
| `blend` | one keyword: `normal multiply screen overlay darken lighten color-dodge color-burn hard-light soft-light difference exclusion hue saturation color luminosity lighter source-over` | `"multiply"` |
| `image` | `--select-arrow` only: an inline `data:image/svg+xml` URL, ≤ 4096 characters, shapes only | see the export |

`var(--token)` takes exactly one token name, no fallback, and the token must
exist. Referring to another token is the idiomatic way to keep a palette
coherent: `"gold": "var(--accent-deep)"`, `"lu-int": "var(--accent)"`,
`"eq-curve": "rgb(var(--accent-rgb) / 0.95)"`.

## Hard rules — these reject the whole document

* No `;`, `{`, `}`, `@`, `\`, `<`, `>`, `/*` or `//` **inside a value**.
  (Comments between values are fine; inside a value they are not.)
* No `url()` except the inline SVG for `--select-arrow`, and that SVG may not
  contain `script`, `foreignObject`, `href`, `on…=`, `<use>`, `<image>`,
  `javascript:` or character entities.
* No remote origin of any kind. Nothing is fetched at runtime, ever.
* No key that is not in the catalogue below, and no top-level key other than the
  seven above.
* A `base` token stated in `dark`/`light`, or a per-theme token stated in
  `base`, is an error that names the block it belongs in.
* Numbers must be finite and under 10 000 in magnitude; a value may not exceed
  600 characters (4096 for the image token) or 240 parsed parts.

## Ranges — these clamp and warn

Out of range is not fatal; refusing a whole theme over one silly number would be
pedantry with a line number on it. The value is clamped and the editor says so.

| token(s) | range |
|---|---|
| any `number` token without its own range | 0 … 1 |
| any `length` token without its own range | 0 … 200 px |
| any `duration` | 0 … 4000 ms |
| any `tracking` | −0.2 … 1 em |
| `r-control` | 0 … 32 px |
| `r-panel` | 0 … 48 px |
| `titlebar-h` | 24 … 64 px |
| `transport-h` | 44 … 140 px |
| `meters-w` | 120 … 420 px |
| `mac-inset` | 0 … 160 px |
| `font-size-base` | 9 … 22 px |
| `wf-bar-step` | 2 … 40 (bar pitch in px) |
| `wf-bar-duty` | 0.15 … 1 |

## Derived tokens — legal, but leave them alone

`accent`, `accent-rgb`, `accent-hi`, `accent-hi-rgb`, `accent-press`,
`accent-deep`, `accent-deep-rgb`, `accent-ink`, `deck-b`, `deck-b-rgb` and
`zoom` are **computed at runtime** from `appearance.accent` and the size scale.
They are legal keys — state one and it wins — but they are deliberately absent
from the export, because a document that pinned them would freeze the accent
picker: the user would move a control and see nothing happen. Stating one
produces a warning that says exactly that.

To recolour the app, set `appearance.accent`, and `deck-b-src` if you want the
second deck somewhere else. Everything downstream follows.

---

## The key set

172 keys. 17 are stated once in `"base"`; 144 are stated per theme in `"dark"`
and `"light"`; 11 are derived (above). The defaults are in the document you
copied out of Onyx — this table is what each key *does*, which the values alone
cannot tell you.

Groups are the order the export writes them in: the things that change the
character of the app first, the fine tuning last.

### `"base"` — the same in both themes

| key | type | what it controls |
|---|---|---|
| `font-ui` | font | the UI family stack. The font picker overrides it, so change it here only for a family the picker does not offer |
| `font-num` | font | the read-out stack: LUFS, timecode, dB, sample counts. **Monospace, or the numbers dance while they update** |
| `font-size-base` | length | body size (9–22 px). Most components state their own size relative to this |
| `ls-label` | tracking | letter spacing on the all-caps section labels |
| `ls-num` | tracking | letter spacing on tabular read-outs. Non-zero here costs column stability |
| `r-control` | length | corner radius on buttons, inputs and chips (0–32 px) |
| `r-panel` | length | corner radius on panels and cards (0–48 px) |
| `titlebar-h` | length | title-bar height (24–64 px) |
| `transport-h` | length | transport-bar height (44–140 px) |
| `meters-w` | length | width of the meter column before it moves under the lanes (120–420 px) |
| `mac-inset` | length | left inset reserved for the macOS traffic lights (0–160 px) |
| `ease` | easing | the app's one easing curve |
| `t-hover` | duration | hover/press transitions |
| `t-panel` | duration | panel and overlay transitions |
| `t-slow` | duration | the slowest transitions (glows, fades) |
| `wf-bar-step` | number | waveform bar pitch in px: bar + gap (2–40). Bigger = calmer, coarser lanes |
| `wf-bar-duty` | number | the share of that pitch the bar itself gets (0.15–1); the rest is the gap |

### `"dark"` / `"light"` — surfaces

The room everything sits in. `ink-900` is the deepest surface and the one every
contrast measurement is taken against. It also leaves the webview: the colour the
*native window* is painted with, under the web layer, is whatever `ink-900`
resolves to in the theme in force — so moving it moves the rim of the window
itself, and a value that fights the rest of your theme will show at the window's
edges before it shows anywhere else.

| key | type | what it controls |
|---|---|---|
| `ink-900` | color | the app background — the deepest surface, and the native window's own background |
| `ink-850` | color | transport and rail background |
| `ink-800` | color | raised surface (cards, rows, the EQ plate) |
| `ink-700` | color | the highest surface |
| `surface-panel` | color | floating panels: settings, the playlist overlay |
| `surface-menu` | color | menus and dropdowns |
| `surface-toast` | color | toasts |
| `surface-badge` | color | small badges (format, deck, archive) |
| `surface-card` | color | cards |
| `scrim` | color | the wash behind a modal |
| `scrim-drop` | color | the wash shown while a file is dragged over the window |
| `stage-glow-a` | color | the accent-tinted glow in the top-left of the stage |
| `stage-glow-b` | color | the deck-B-tinted glow in the top-right |
| `bg-app` | background | the whole app background: the two glows layered over `ink-900` |
| `bg-eq` | background | the EQ window's background, same idea |
| `bg-titlebar` | background | the title bar's sheen |
| `bg-stage` | background | the stage (the waveform area) gradient |
| `bg-transport` | background | the transport bar gradient over `ink-850` |
| `bg-rail` | background | the playlist rail gradient |
| `well` | color | a recessed field (inputs, sliders' troughs) |
| `well-deep` | color | a deeper recess |
| `well-canvas` | background | the plate a canvas is drawn on |

### text

Four ink levels, all translucent, all composited over whatever they land on.

| key | type | what it controls |
|---|---|---|
| `text-hi` | color | primary text — **the pair with `ink-900` decides whether the app is readable** |
| `text-mid` | color | secondary text: metadata, inactive values |
| `text-lo` | color | labels and axis ticks |
| `text-faint` | color | the quietest ink: separators, disabled hints |

### accent

The family itself is derived from `appearance.accent`; these are the fixed parts
around it.

| key | type | what it controls |
|---|---|---|
| `ink-on-accent-dark` | color | text printed on a *light* accent fill |
| `ink-on-accent-light` | color | text printed on a *dark* accent fill |
| `gold` | color | the deep accent as used for hairline emphasis |
| `gold-2` | color | the bright accent as used for emphasis |
| `selection` | color | text/row selection wash |

### decks

Deck A is the accent; deck B must stay separable from it at a glance, because
telling the two lanes apart is the whole point of an A/B player.

| key | type | what it controls |
|---|---|---|
| `deck-a` | color | deck A's identity colour |
| `deck-b-src` | color | deck B's steel. The runtime rotates it away if the accent invades its hue |
| `deck-b-ink` | color | text printed on a deck B fill |

### meter scale

Four stops that must stay distinguishable at a glance and in peripheral vision.
Each colour has an `-rgb` triplet beside it, used where an alpha is applied;
change both or the meters and the numbers disagree.

| key | type | what it controls |
|---|---|---|
| `m-safe`, `m-safe-rgb` | color, rgb | below −18 dB |
| `m-warn`, `m-warn-rgb` | color, rgb | −18 to −6 dB |
| `m-hot`, `m-hot-rgb` | color, rgb | −6 to −1 dB |
| `m-clip`, `m-clip-rgb` | color, rgb | clipping |
| `m-clip-hi` | color | the clip read-out at its loudest state |
| `m-hot-ink` | color | text printed on a hot/solo chip |

### hairlines

| key | type | what it controls |
|---|---|---|
| `hairline` | color | the default 1 px separator |
| `hairline-strong` | color | emphasised separators, focused edges |
| `hairline-soft` | color | the quietest separator |

### overlay fills

Translucent washes stacked over a surface. They are ordered by weight: keep them
monotonic or hover states will read as *less* prominent than resting ones.

| key | type | what it controls |
|---|---|---|
| `fill-row` | color | alternating playlist rows |
| `fill-ghost` | color | ghost buttons at rest |
| `fill-quiet` | color | quiet chips |
| `fill-input` | color | input and select fields |
| `fill-sunken` | color | pressed / sunken controls |
| `fill-hover` | color | hover |
| `fill-menu` | color | menu item hover |
| `fill-hover-hi` | color | hover on an already-emphasised control |
| `fill-track` | color | slider and progress tracks |
| `masked-stripe` | color | the diagonal stripe over a blind-test-masked lane |

### waveform

The lanes. `lane-*-rgb` are triplets because the bars are drawn on canvas at
per-layer alphas; the `wf-*-a` numbers *are* those alphas.

| key | type | what it controls |
|---|---|---|
| `wf-outer-a` | number | peak-envelope ink level (0–1) |
| `wf-core-a` | number | RMS-core ink level (0–1) |
| `wf-cap-a` | number | the bar cap |
| `wf-glow-a` | number | the glow under an audible lane |
| `wf-decode-a` | number | the still-decoding region |
| `wf-playhead-a` | number | the playhead's opacity when audible |
| `wf-blend` | blend | how waveform layers add up: `lighter` on a dark theme, `multiply` on a light one. **Additive blending on paper looks like fog** |
| `wf-mid` | color | the zero line |
| `wf-skeleton` | color | the placeholder lane before audio is decoded |
| `wf-scrim` | color | the wash over the un-played half of a lane |
| `wf-gap-fill` | color | a gap (silence/undecoded) fill |
| `wf-gap-line` | color | the gap's edge |
| `wf-gap-text` | color | the gap's label |
| `wf-hover-line` | color | the hover scrub line |
| `wf-playhead-idle` | color | the playhead in the lane that is not audible |
| `lane-a-rgb` | rgb | the colour deck A's bars are drawn in |
| `lane-b-rgb` | rgb | the colour deck B's bars are drawn in |
| `lane-masked-rgb` | rgb | bars in a blind test, where the deck must not be identifiable |
| `lane-idle-o` | number | how far the lane that is not audible recedes (0–1) |
| `loop-fill` | color | the loop region's fill |
| `loop-edge` | color | the loop region's edges |
| `loop-grip` | color | the loop handles |
| `align-chip` | color | the A/B offset chip |
| `align-chip-ink` | color | its text |
| `align-line` | color | the alignment guide line |
| `tip-bg` | color | canvas tooltip background |
| `tip-line` | color | its border |
| `tip-text` | color | its text — measured for contrast against `tip-bg` |

### level meter

| key | type | what it controls |
|---|---|---|
| `mt-track` | color | the meter's trough |
| `mt-grid` | color | dB grid lines |
| `mt-grid-zero` | color | the 0 dB line |
| `mt-label` | color | dB labels |
| `mt-label-zero` | color | the 0 dB label |
| `mt-rms` | color | the RMS underlay |
| `mt-chan` | color | the channel divider |
| `mt-safe`, `mt-safe-lo` | color | the safe segment, and its quieter lower part |
| `mt-warn` | color | the warn segment |
| `mt-hot` | color | the hot segment |
| `mt-clip` | color | the clip segment and the clip flag |

### loudness strip

| key | type | what it controls |
|---|---|---|
| `lu-track` | color | the strip's trough |
| `lu-safe`, `lu-warn`, `lu-hot`, `lu-clip` | color | the four loudness zones |
| `lu-target` | color | the target-loudness marker |
| `lu-target-label` | color | its label |
| `lu-short` | color | the short-term bar |
| `lu-int` | color | the integrated read-out |

### correlation meter

| key | type | what it controls |
|---|---|---|
| `corr-track` | color | the scale |
| `corr-neg` | color | the out-of-phase end |
| `corr-mid` | color | the middle of the scale |
| `corr-pos` | color | the in-phase end |
| `corr-centre` | color | the centre tick |
| `corr-needle-neg` | color | the needle when correlation is negative |
| `corr-needle-pos` | color | the needle when it is positive |

### EQ curve and analyser

| key | type | what it controls |
|---|---|---|
| `eq-grid-minor`, `eq-grid-major`, `eq-grid-zero` | color | the frequency/dB grid, and the 0 dB line |
| `eq-freq-label`, `eq-db-label` | color | axis labels |
| `eq-spec-top`, `eq-spec-bottom`, `eq-spec-edge`, `eq-spec-peak` | color | the spectrum analyser behind the curve: its gradient, its outline and its peak hold |
| `eq-band`, `eq-band-hot` | color | a band's region, at rest and while it is being dragged |
| `eq-curve` | color | the composite EQ curve — **the one line the window exists to show** |
| `eq-curve-off` | color | the curve while the EQ is bypassed |
| `eq-fill-top`, `eq-fill-bottom` | color | the fill under the curve |
| `eq-fill-off-top`, `eq-fill-off-bottom` | color | the same while bypassed |
| `eq-node-glow`, `eq-node-fill`, `eq-node-ring` | color | a band node: its halo, its body, its ring |
| `eq-node-fill-off`, `eq-node-ring-off` | color | a bypassed band's node |
| `eq-tip-line` | color | the readout guide line |
| `solo-band`, `solo-line`, `solo-chip`, `solo-chip-ink` | color | the band-solo sweep: its region, its centre line, its chip and that chip's text |

### window chrome

| key | type | what it controls |
|---|---|---|
| `winctl-close` | color | Windows' own close-button red. A platform affordance — leave it alone unless you know why you are moving it |
| `winctl-close-ink` | color | the glyph on it |
| `shadow-float` | shadow | the shadow under panels, menus and toasts |
| `select-arrow` | image | the `<select>` chevron. Inline SVG data URL only; recolour the stroke to match your text ink |

---

## A worked example

A partial document — 40-odd keys out of 172 — that turns the dark theme into
cold graphite and the light theme into blueprint paper, keeps both readable, and
leaves everything it does not mention at its designed value. This is the shape
of a good answer: an accent, the surfaces, the four inks, the hairlines, deck B
pushed away from the new accent's hue, and the meter scale re-tuned to match.

```jsonc
{
  "onyx": "theme",
  "version": 1,
  "name": "Cold Graphite",

  "appearance": {
    "theme": "dark",
    "accent": "#4f8fbf",       // one hex; hover/pressed/deep/ink follow
    "numericFont": "sf-mono"
  },

  "base": {
    "r-control": "3px",
    "r-panel": "12px",
    "ls-label": "0.16em",
    "wf-bar-step": 6,          // wider bars: this look is about calm
    "wf-bar-duty": 0.55,
  },

  "dark": {
    "ink-900": "#0b0e11",      // graphite, very slightly blue
    "ink-850": "#0e1216",
    "ink-800": "#11161b",
    "ink-700": "#161c22",
    "surface-panel": "rgba(18, 23, 28, 0.96)",
    "surface-menu": "rgba(20, 26, 32, 0.97)",

    "text-hi": "rgba(233, 240, 246, 0.94)",   // 4.5:1 on ink-900 — check this
    "text-mid": "rgba(233, 240, 246, 0.6)",
    "text-lo": "rgba(233, 240, 246, 0.34)",
    "text-faint": "rgba(233, 240, 246, 0.18)",

    "hairline": "rgba(190, 215, 235, 0.08)",
    "hairline-strong": "rgba(190, 215, 235, 0.14)",

    "deck-b-src": "#c98f5a",   // warm, because the accent is now cold
    "deck-b-ink": "#1a1206",

    "m-safe": "#5bc98f",  "m-safe-rgb": "91 201 143",
    "m-warn": "#d8c98a",  "m-warn-rgb": "216 201 138",
    "m-hot":  "#e08a4f",  "m-hot-rgb":  "224 138 79",
    "m-clip": "#e2504a",  "m-clip-rgb": "226 80 74",

    "wf-scrim": "rgba(6, 9, 12, 0.7)",
    "wf-outer-a": 0.5,
    "wf-core-a": 0.98,
    "eq-curve": "rgb(var(--accent-rgb) / 0.96)",
    "tip-bg": "rgba(11, 14, 17, 0.94)",
    "tip-text": "rgba(233, 240, 246, 0.94)"
  },

  "light": {
    "ink-900": "#e8ecef",      // blueprint paper, not white
    "ink-850": "#eef1f4",
    "ink-800": "#f4f6f8",
    "ink-700": "#fafbfc",
    "text-hi": "rgba(20, 28, 34, 0.94)",
    "text-mid": "rgba(20, 28, 34, 0.68)",
    "text-lo": "rgba(20, 28, 34, 0.48)",
    "hairline": "rgba(30, 45, 60, 0.14)",
    "deck-b-src": "#8a5a22",
    "deck-b-ink": "#fdf6ee",
    "tip-bg": "rgba(252, 253, 253, 0.96)",
    "tip-text": "rgba(20, 28, 34, 0.94)",
    "wf-scrim": "rgba(238, 241, 244, 0.46)"
  }
}
```

The full version of this theme is `src/lib/fixtures/llm-theme.jsonc`, which the
contract check and the screenshot harness both apply for real.

## Failure modes — what Onyx says, and what it does

**One error rejects the document and changes nothing.** There is no partial
theme, ever. Warnings do not block anything.

| what you wrote | what happens | what it says |
|---|---|---|
| `"txt-hi": "#fff"` | **error** | `“txt-hi” is not an Onyx token — did you mean “text-hi”?` |
| `"r-panel"` inside `"dark"` | **error** | `“r-panel” is the same in both themes — move it into the “base” block` |
| `"ink-900"` inside `"base"` | **error** | `“ink-900” is stated per theme — move it into “dark” and/or “light”` |
| `"text-hi": "rgba(255,255,255,0.9); } * { display: none }"` | **error** | `";" is not allowed in a value` |
| `"select-arrow": "url(https://example.com/a.svg)"` | **error** | `only an inline data:image/svg+xml URL is allowed — no remote images` |
| `"eq-curve": "var(--eq-curv)"` | **error** | `var(--eq-curv) is not a token in this theme` |
| `"wf-core-a": "loud"` | **error** | `expected a number, got "loud"` |
| `"version": 2` | **error** | `this theme says version 2, and this build of Onyx understands 1` |
| a missing brace, a stray `]` | **error** | the reader's message, with the line and column it gave up at |
| `"r-panel": "400px"` | *warning*, value clamped | `400px is outside 0…48; clamped to 48px` |
| `"accent-hi": "#fff"` | *warning*, value applied | `“accent-hi” is normally derived from appearance.accent; stating it here pins it and the accent picker will no longer move it` |
| no `"onyx": "theme"` marker | *warning* | `is it an Onyx theme?` |
| text so dark it cannot be read | *warning*, theme applies | `dark: body text on the app background is 1.42:1, below the 4.5:1 this pair needs to stay readable` |

Every error carries a **line and column**; clicking it in the editor selects
that line. Errors are listed in document order.

If a theme that *did* apply turns out to be unusable, three ways out, none of
which depend on being able to see the UI:

* **`⌘/Ctrl + ⌥/Alt + ⇧ + R`** from any window — resets appearance and document;
* the native **Appearance ▸ Reset Appearance** menu item;
* the standalone theme editor window, which never wears the theme it is editing.

A persisted theme that no longer parses at launch is dropped with a message, and
the app comes up in the designed themes rather than not coming up.

## Contrast: measured, warned about, never enforced

Before a document lands, both themes are audited on these fourteen pairs, with
translucent inks composited over the surface they actually sit on:

| pair | tokens | minimum |
|---|---|---|
| body text on the app background | `text-hi` on `ink-900` | 4.5:1 |
| body text on a panel | `text-hi` on `surface-panel` | 4.5:1 |
| body text on a menu | `text-hi` on `surface-menu` | 4.5:1 |
| secondary text on the app background | `text-mid` on `ink-900` | 3:1 |
| secondary text on a panel | `text-mid` on `surface-panel` | 3:1 |
| labels and axis ticks | `text-lo` on `ink-800` | 2:1 |
| canvas tooltips | `tip-text` on `tip-bg` | 4.5:1 |
| accent text and hairlines | `accent` on `ink-900` | 3:1 |
| text on an accent fill | `accent-ink` on `accent` | 3:1 |
| the clip colour | `m-clip` on `ink-900` | 3:1 |
| deck B against the stage | `deck-b` on `ink-900` | 3:1 |
| deck A's waveform bars | `lane-a-rgb` on `ink-900` | 3:1 |
| deck B's waveform bars | `lane-b-rgb` on `ink-900` | 3:1 |
| the EQ curve on its plate | `eq-curve` on `ink-900` | 3:1 |

Onyx warns and applies anyway: someone mastering in a dark room may want a
display quieter than any guideline allows. But if you are writing a theme for
someone else, treat a warning as a bug in your theme.

## What makes a good Onyx theme

* **Both themes, always.** Light is paper, not an inverted dark: on paper the
  canvas alphas go *up*, the blend goes to `multiply`, and the accent darkens
  rather than brightening.
* **Keep deck A and deck B different hues.** Two lanes that read as one colour
  make the A/B feature useless.
* **Keep the four meter stops separable**, including for a red-green colour
  deficiency: they differ in lightness as well as hue in the shipped themes.
* **Do not make the room louder than the content.** Surfaces are near-neutral
  and quiet on purpose; the accent is for one or two things at a time.
* **Numbers must stay legible and stable** — monospace read-outs, no tracking on
  `ls-num`, text ink at 4.5:1.
* **Change less than you think.** The palette is deeply cross-referenced with
  `var()`; moving `ink-900`, the four inks, the accent and `deck-b-src` already
  re-themes most of the app.

---

# Part 2 — The engine interface

For contributors. Part 1 is the contract a theme must satisfy; this is how the
app satisfies it.

## Where it lives

| file | owns |
|---|---|
| `src/styles/tokens.css` | the two designed themes. **The only file in `src/` that contains a hex or an `rgba()`** |
| `src/lib/tokens.ts` | the catalogue: parses `tokens.css` at runtime (`?raw`) so defaults cannot drift, adds group/prose/range metadata |
| `src/lib/cssvalue.ts` | the typed value parser. Tokenises, whitelists, re-emits. No DOM |
| `src/lib/jsonc.ts` | the tolerant reader, with positions and hard limits. No DOM |
| `src/lib/contrast.ts` | the WCAG audit over the parsed document. No DOM |
| `src/lib/themedoc.ts` | what a document *means*: parse, validate, export, the agent brief. No DOM |
| `src/lib/theme.ts` | the DOM: attributes, the inline custom-property layer, the OS watch, the escape hatch. Parses nothing |
| `src/lib/themeio.ts` | the seam the UI uses: validate / apply / revert / reset / clipboard |
| `src/components/ThemeEditor.tsx` | the editor, used compactly in Settings and full-size in its own window |
| `src/theme/` + `theme.html` | the editor window's entry point |
| `src-tauri/src/settings.rs` | storage only: bounded text, no control characters, `themeDoc` in schema 4 |
| `src-tauri/src/appmenu.rs` | the native Appearance menu — the escape hatch the theme cannot paint over |

## The mechanism

`src/lib/theme.ts` is the only thing in the front end that may set a theme. It
writes, on `document.documentElement`:

| attribute         | values                              |
| ----------------- | ----------------------------------- |
| `data-theme`      | `dark` \| `light` — **resolved**, never `system` |
| `data-theme-pref` | `dark` \| `light` \| `system` — what the user chose |
| `data-ui-font`    | a font token (`system`, `grotesk`, `humanist`, `neutral`) |
| `data-num-font`   | a font token (`system-mono`, `sf-mono`, `menlo`, `consolas`, `courier`) |
| `data-scale`      | `compact` \| `normal` \| `large` (applied as `zoom`, capped at the 420 × 560 floor) |

plus an **inline custom-property layer**: the accent family (`--accent`,
`--accent-hi`, `--accent-press`, `--accent-deep`, `--accent-ink`, their `-rgb`
triplets, `--deck-b` / `--deck-b-rgb`) and whatever the theme document states.
`tokens.css` still declares both designed themes underneath, which is what makes
a partial document work and reverting a matter of *removing* properties. The
whole app — all three windows — re-themes in one style recalculation.

Nothing else sets `data-theme`. No component, no stylesheet, no inline style
declares a colour.

## What the settings UI calls

```ts
import {
  applyAppearance,      // (appearance) => Appearance     apply / live preview
  startAppearanceSync,  // () => () => void      mirror snapshot.appearance, follow the OS
  currentAppearance,    // () => Appearance      what is applied right now
  resolvedTheme,        // () => "dark" | "light"
  parseHex,             // (string) => "#rrggbb" | null   validate a typed accent
  accentFamily,         // (hex, theme) => { accent, hi, press, deep, ink }  for swatches
  normaliseAppearance,  // (anything) => Appearance   same rules as Rust
  DEFAULT_APPEARANCE,   // matches Appearance::default() in src-tauri/src/settings.rs
  onThemeChange,        // (fn) => unsubscribe    canvas repaint hook
} from "../lib/theme";
```

Typical flow: `applyAppearance()` for the live preview, then persist through
`setAppearance()`; Rust broadcasts the new snapshot and every window re-applies
it. `applyAppearance()` is idempotent, synchronous and total: it validates every
field, never throws, and calling it on every pointer-move of a colour picker is
fine. It takes a *whole* appearance — a field you leave out falls back to the
default, not to the value currently applied, because that is what Rust does with
a half-written `settings.json`. Spread `currentAppearance()` to change one thing.

It also mirrors what it applied into `localStorage` (`onyx.appearance`, which
carries the theme document too), which is what makes

- the first frame of a window paint the right theme instead of flashing
  obsidian, and
- the detached EQ and theme windows follow a change made in the main one, live.

`startAppearanceSync()` is already called by `App.tsx`, `EqWindow.tsx` and
`ThemeWindow.tsx`. It adopts `snapshot.appearance` and `snapshot.themeDoc` when
the backend sends them, follows the OS while the preference is `system`, and
ignores a snapshot that carries no appearance block (an older backend must not
silently un-theme the window).

## What the theme editor calls

```ts
import {
  defaultThemeText,          // () => string          the designed look, as a document
  currentThemeSource,        // () => string          what is on screen, verbatim if a doc is in force
  themeForAgent,             // (text?) => string     the same, prefixed with AGENT_BRIEF
  validateTheme,             // (text) => ParseOutcome       never throws, applies nothing
  applyTheme,                // (text) => Promise<ApplyResult>  validate → paint → persist
  revertTheme,               // () => Promise<void>   drop the document, keep the §15 choices
  resetAppearanceEverywhere, // () => void            the escape hatch
  copyWithToast, summarise,
} from "../lib/themeio";
```

The invariants, all enforced in `themeio.ts` so both editors cannot drift:

* **Nothing is applied until everything validates.** `parseTheme` returns
  `doc: null` on a single error and `applyTheme` returns before touching the
  DOM.
* **Paint before IPC.** The window wears the theme in the same frame; the
  persist follows. That is what makes Apply feel like a switch.
* **A failed persist puts the previous look back**, because a theme that
  survives until the next launch and then vanishes is worse than one that never
  landed.
* **The escape hatch does not depend on any of it**: `resetAppearanceLocally()`
  runs first and unconditionally, then the backend call, whose failure is logged
  rather than thrown — the user pressed it because they could not read the
  screen, and a toast they cannot read is not an answer.

Two lower-level entry points exist for the windows themselves:
`applyThemeDoc(doc, text, appearance?)` installs a validated document, and
`ignoreThemeDoc()` — called once, at module scope, by the theme editor window
and nothing else — keeps that window in the designed themes so a bad theme
cannot eat the place you fix it from. The §15 appearance still applies there.

## Canvas

A 2D context cannot read a CSS variable, so canvas code asks `paint()`:

```ts
const p = paint();                      // cheap, cached, call it every frame
p.color("--eq-curve");                  // the token, as the theme declares it
p.tint("--lane-a-rgb", 0.46);           // an rgb triplet at an alpha
p.num("--wf-core-a");                   // a numeric token (bar ink levels)
p.fade("--eq-node-glow", 0);            // the same colour, alpha scaled
p.font(10);  p.fontUi(9);               // §15 font choices, for canvas text
p.blend();                              // `lighter` on obsidian, `multiply` on paper
```

Every token a canvas may use is listed in `PAINT_TOKENS`, so a typo is a type
error and a missing declaration is reported once, loudly, at startup. A theme
document changes what these return, which is why the palette cache is
invalidated on every apply, not only on a theme switch.

Painters inside the 60 Hz frame loop get a new palette for free. A painter with a
**static** layer must repaint on a theme change; `useSurface()` (`src/lib/canvas.ts`)
already subscribes via `onThemeChange()` and calls your `onResize`, so any canvas
built on it is covered. A canvas that manages its own lifecycle must call
`onThemeChange()` itself — a stale canvas after a theme switch is the bug this
whole layer exists to prevent.

## Accent overrides

One hex in, a family out, derived in OKLCH so a variant keeps the accent's hue
instead of sliding towards grey (`accentFamily()`). The transforms are relative
and were measured from the hand-tuned champagne of SPEC §4, so the default
`#c9a227` reproduces `#e8d9a0 / #f3e8bf / #cdb478 / #c9a227` exactly — the dark
identity does not move — while any other accent gets the same treatment. Light
has its own ramp: on paper the accent *darkens* and keeps its chroma.

Lightness is clamped to a per-theme legibility window and chroma to a ceiling, so
a near-black, near-white or fluorescent accent still reads. `--accent-ink` flips
between the theme's two extremes by contrast, deck B rotates out of the way if
the accent invades its hue, and the waveform lane draws in `--accent-deep` in the
light theme because a mid-tone bronze bar under a scrim is not enough ink on
paper.

## Persistence

`settings.json` schema 4 holds `appearance` and `themeDoc` (the document's
source text, verbatim, ≤ 256 KiB). Rust validates *storage* — text, bounded, no
control characters other than tab/CR/LF — and nothing about the schema: two
validators would disagree within a release. A document that no longer parses is
dropped at startup, reported through `takeThemeDocProblem()` /
`onThemeDocProblem()`, and is never a reason to reset the rest of the file.

## Checks

```bash
npm run check:theme      # the contract: catalogue vs tokens.css, export→import→export
                         # fixed point, reader tolerances, hostile values, contrast
                         # maths, suggestions, an LLM-shaped fixture, and mock↔Rust
                         # storage parity from one shared fixture

# both need a mock build being served (the URL defaults to localhost:4173):
#   npm run build:mock && npx vite preview --outDir dist-mock --port 4173
npm run shots:themedoc -- <url>   # the workflow, photographed end to end
npm run shots:theme -- <url>      # both themes, accents, canvas staleness
```

Both exit non-zero if anything they asserted was untrue; the browser, the window
sizes and the console watch come from `scripts/lib/shots-base.mjs`, shared with
`npm run shots`.

`check:theme` runs inside `npm run build` and `npm run build:mock`, transpiling
the real modules rather than a copy of them. Mock ↔ Rust storage parity is
driven from `src-tauri/tests/fixtures/theme_doc_contract.json`, which both
backends read, so a divergence fails the build rather than a screenshot.

## Mock preview

Mock builds only (`npm run build:mock`) read `?theme=…&accent=…&scale=…&uiFont=…&numFont=…`
from the URL and expose `window.__onyxTheme`. `scripts/shots-theme.mjs` uses both
to photograph every theme and to prove no canvas goes stale; a shipped build
exposes neither.

## What the harness cannot prove

`scripts/shots-themedoc.mjs` drives the whole workflow (shots 46–55) in a
headless Chromium against the mock backend, and asserts state rather than
pixels. Four things are outside its reach and need a real desktop:

* **The detached windows are browser popups.** Real `WebviewWindow` decorations,
  the window-manager theme Onyx pushes at startup, and the macOS traffic-light
  inset (`mac-inset`) are not exercised.
* **The native Appearance menu cannot be clicked.** `appmenu.rs` is only
  reachable from a running Tauri app, so the menu's reset and "Theme Editor…"
  items are covered by their shared Rust entry point (`reset_appearance_in`) and
  by the keyboard chord, not by the menu itself.
* **Font resolution is the OS's.** `font-ui` / `font-num` are family *stacks*;
  which face a platform actually picks — and whether the numeric column really
  stays stable in it — can only be seen on that platform.
* **Nothing is audible**, and the meters in a screenshot are the mock's.
