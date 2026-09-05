# Contributing to Onyx — orientation for whoever picks this up next

This is the document you read *first*, once. It is not a second spec: it tells
you what Onyx is, where each concern lives, which invariants you must not
break and why, how to prove a change without a display, and — just as
important — what in this repository has never been observed running.

The four documents that came before it, and what each one is for:

| document | authority |
|---|---|
| [SPEC.md](SPEC.md) | **the contract.** The IPC surface, the behaviour rules, the feature definitions. If §3.1 does not list a command, it does not exist. Cite it, don't restate it. |
| [README.md](README.md) | user- and contributor-facing: what it does, prerequisites, every build/check command, release procedure, and the *Known limitations* list this document's §5 summarises. |
| [THEMING.md](THEMING.md) | the theme-document contract, written to be handed to a language model. Load-bearing: the paste-a-theme workflow only works if the first edit is a correct one. |
| [THIRD-PARTY.md](THIRD-PARTY.md) | what Onyx redistributes and under which licence. |

Everything below is derived from the code as it stands. Where a fact could
move, this document names the **command that prints the current truth**
instead of stating a number.

---

## 1. Orientation

### What it is, and who for

A fast, professional playback and A/B comparison desktop app (Resonic-class)
for mix and mastering engineers. The promise is a single sentence: **open a
file, hear it, compare it against another one, faster than any DAW or file
manager can.** Every design decision follows from that, and several of them
look wrong until you accept it:

* **Open = replace, drop = append, one tap = play** (SPEC §2.1–§2.3). A single
  click on a playlist row loads deck A and plays from zero. No double-click, no
  confirmation. That is why putting material on **deck B** needs four explicit
  routes of its own (SPEC §2.8) — a plain click is already spoken for.
* **Click-to-sound under ~50 ms** (SPEC §2.4). `decode::open` returns after the
  header parse; playback starts on ~150 ms of audio while the decoder is still
  running.
* **Bit-transparent by default** (SPEC §2.5, §10). The engine follows the
  *source* sample rate so deck A is never resampled, and level matching is
  opt-in and attenuate-only. An engineer must be able to trust that what they
  hear is the file unless they asked for otherwise.
* **Whole files live in RAM.** Not an oversight — instant A/B at a shared
  playhead, sample-accurate offset alignment and instant seeking all assume
  resident audio. See README *Known limitations* for the budget and its cost.

### The shape of the system

```
crates/onyx-core/     the audio engine. Knows nothing about Tauri, windows,
                      settings or the UI. Unit-testable and reusable alone.
  align  decode  pcm  waveform  container  midi  opus  dsp/  engine  types

src-tauri/            the app layer. Owns state, the IPC surface, windows,
                      persistence, the frame pump, archives, the OS.
  commands  lib.rs  state  loader  playlist  archive  blind  cache  settings
  persist  frame  abrules  eqwindow  themewindow  surface  appmenu  clientlog
  safe_decode

src/                  React 19 + TypeScript. Three webview entry points.
  index.html → src/main.tsx        the main window
  eq.html    → src/eq/main.tsx     the detached EQ window   (SPEC §12)
  theme.html → src/theme/main.tsx  the theme editor window  (SPEC §20)
  lib/        api.ts (the IPC mirror) · types.ts · mock.ts (the second backend)
              theme/token/colour modules, all deliberately DOM-free
  styles/     tokens.css is the only place colours are written
```

All three documents are emitted from one Vite build (`vite.config.ts`
`rollupOptions.input`); `eq.html` and `theme.html` are real webviews created
**from Rust** (`eqwindow.rs`, `themewindow.rs`) via the `eq_window_*` /
`theme_window_*` commands. Neither webview can conjure a window: the
capability files in `src-tauri/capabilities/` grant `listen`/`unlisten`/`emit`
and, for `main` only, the custom title bar's drag and window buttons. Nothing
else. Application commands are not gated by capabilities — the
`#[tauri::command]` surface is the whole attack surface (SPEC §5.1).

### Where does my change belong?

| the change | where |
|---|---|
| DSP, decoding, metering, alignment maths, anything the audio callback touches | `crates/onyx-core/` |
| a new capability the UI can ask for | a command in `src-tauri/src/commands.rs` (§4.1 below) |
| "what happens when a file loads / the rate changes / a deck is replaced" | `src-tauri/src/loader.rs` — it owns the ordering rules |
| A/B assignment semantics | `src-tauri/src/abrules.rs`, and the fixture both backends read |
| a colour, anywhere | `src/styles/tokens.css` and its catalogue entry in `src/lib/tokens.ts`. Never a literal in a component or a canvas call |
| layout, gestures, read-outs | `src/components/` |
| anything the browser preview must also do | `src/lib/mock.ts`, in the **same commit** (§3.2) |

Rust never parses a theme, and the front end never decodes audio. Those two
lines are load-bearing; crossing either creates a second implementation of
something that already has one.

---

## 2. How to work

### Build and run

Prerequisites are in [README § Development](README.md#development) — Node
20.19+/22.12+, Rust stable, and the platform webview toolchain. Then:

```bash
npm install
npm run tauri dev        # vite on :1420 + the Rust app, both hot-reloading
npm run dev:mock         # UI only, in a plain browser, against the mock backend
```

This is a cargo **workspace**: build output is `./target`, *not*
`src-tauri/target`. `target/ dist/ dist-mock/ artifacts/ shots/ .tmp/` are all
regenerable and all `.gitignore`d.

Release builds are one script per platform, each of which preflights its whole
toolchain and reports *every* missing item before starting:

```bash
./scripts/build-mac.sh              # universal .app + .dmg. Must run ON macOS.
.\scripts\build-windows.ps1         # NSIS installer; add -Msi for an .msi too
npx tauri build --bundles deb       # Linux: config/bundle smoke test only
```

See [README § Building releases](README.md#building-releases) for what each
produces and what signing costs. **Neither mac nor Windows script has ever
been run** — see §5.

A *release* is not built by hand: push a tag and
[`.github/workflows/release.yml`](.github/workflows/release.yml) builds the
universal `.dmg` on a macOS runner and the NSIS installer on a Windows one, and
attaches both to a draft release — after checking that the tag matches every
version string in the tree and that the suite is green. The two scripts above
remain the way to get a bundle on your own machine, and the way to debug the
bundling when the workflow is the thing that broke.

### Every check, and what it guards

Run all of these before you call a change done. None needs an audio device.

| command | guards |
|---|---|
| `cargo test --workspace` | the engine and the app layer, including the real-time-safety, bit-transparency, format, gapless, loudness/true-peak, alignment, hostile-input and IPC-registration tests. One test is `#[ignore]`d (the wide alignment sweep, meant for `--release`) |
| `cargo clippy --workspace --all-targets` | kept warning-free. Treat a new warning as a failure |
| `cargo fmt --all` | formatting |
| `npm run typecheck` | `tsc --noEmit` |
| `npm run check:eq` | the drawn EQ curve against the coefficients the engine actually runs. Two implementations of one response, in two languages |
| `npm run check:ab` | deck assignment, deck clearing and the blind-test refusals: **the mock backend against the Rust source**, from a shared fixture |
| `npm run check:theme` | the theme contract — catalogue vs `tokens.css` both ways, export→import→export identity, the tolerant reader, the sanitiser, the contrast maths, and mock↔Rust storage parity |
| `npm run check:version` | one version in seven places — `package.json`, `package-lock.json` (twice), `Cargo.toml`'s `[workspace.package]`, `Cargo.lock`'s two workspace members and `tauri.conf.json`. Given a tag (`node scripts/check-version.mjs v1.2.3`) it checks that too, which is how the release workflow refuses to build a mislabelled installer |
| `npm run check:ipc` | the command surface, four ways: SPEC §3.1's table, `generate_handler!`, the `api.ts` wrappers and the `case` labels of the mock's `switch (cmd)` — held equal in every direction, and every command driven through the transpiled mock so a label that dispatches nothing fails too |
| `npm run build` | `typecheck` + the five checks above + the Vite bundle into `dist/` |
| `npm run build:mock` | the same, mock IPC, into `dist-mock/` |
| `npx tauri build --no-bundle` | that the real app still links |

The four contract `check:*` scripts (`eq`, `ab`, `theme`, `ipc`) **transpile the
real modules** and exercise them; none contains a transcription of the logic it
checks. Keep it that way — a copy of a rule inside its own checker proves
nothing. (`check:version` is the exception that proves it: it compares files to
each other and has no logic of its own to copy.)

You do not have to remember to run them.
[`.github/workflows/ci.yml`](.github/workflows/ci.yml) runs every one on every
push and pull request — the front-end half and both bundles on Linux, and
`cargo fmt --check`, clippy-as-errors and `cargo test --workspace` on macOS,
Windows *and* Linux, which is more platforms than a contributor usually has.
It also builds the `.deb` on each push, so a break in the bundle configuration
surfaces on the commit that caused it rather than on a tag.

### Reviewing UI changes with no display

You almost certainly have no display and no audio device. Three tools, in
increasing cost:

**1. The mock backend.** `src/lib/mock.ts` is a second, complete
implementation of the IPC surface — a plausible `AppSnapshot`, a 60 Hz frame
stream, synthetic waveforms, and the *behavioural* parts too (level matching
reports `ready: false` until "analysis lands", the ABX mapping is never
serialised, the blind guard refuses what the engine refuses). It is selected by
`__ONYX_MOCK__`, a build-time constant, so a normal build tree-shakes it out
entirely.

**2. The screenshot harnesses.** They are assertion suites that also take
pictures. All three need a mock build being served, and take the URL
positionally (falling back to `ONYX_SHOTS_URL`, then `http://localhost:4173`):

```bash
npm run build:mock
npx vite preview --outDir dist-mock --port 4173 &
npm run shots -- http://localhost:4173         # layout, gestures, the four routes onto deck B
npm run shots:theme -- http://localhost:4173   # both themes, accents, canvas staleness, persistence
npm run shots:themedoc -- http://localhost:4173 # the theme document, end to end
```

One numbered set of files in `shots/` across the three, no gaps, no collisions.
Each exits non-zero if anything it asserted was untrue, and **a run that prints
anything but `CONSOLE: clean` is evidence of a bug, not a release** — any
console error or warning from any page fails the run. The browser, viewports,
toast shutter, popup adoption, blank-frame size floor and exit verdict are
shared through `scripts/lib/shots-base.mjs`; the harnesses hold only their
arguments, so they cannot drift apart on plumbing. `--tag <name>` (on
`shots:theme` only) writes frozen before/after frames into `shots/diff/`.

They assert *state*, not pixels, wherever a screenshot could not tell the
difference. Read the *What the harness cannot prove* section of
[THEMING.md](THEMING.md) before trusting one: the detached windows are browser
popups, the native menu cannot be clicked, font resolution is the OS's, and
nothing is audible.

**3. A deploy preview, for a human.** `dist-mock/` is a plain static bundle
(`index.html`, `eq.html`, `theme.html`, `assets/`) and can be deployed as-is
for someone to click through. It must be served **at the site root** — Vite's
default `base` emits absolute `/assets/…` URLs. Mock builds also read
`?theme=…&accent=…&scale=…&uiFont=…&numFont=…` from the URL and expose
`window.__onyxTheme` / `window.__onyxDiag`; a shipped build exposes neither.

---

## 3. Invariants you must not break

Each of these exists because of a specific failure. Each has a test. If your
change makes one of these tests fail, the change is wrong — not the test.

### 3.1 The audio callback allocates nothing, locks nothing, logs nothing

`RtCore::process` in `crates/onyx-core/src/engine.rs` runs on the OS audio
thread under a hard deadline. Allocation and deallocation can take a lock
inside the allocator; `log::warn!` is allocation *and* locking *and* I/O. A
missed deadline is an audible click, and clicks are the one thing this app's
users will never forgive.

The non-obvious consequence: **the callback must never drop an
`Arc<SharedPcm>`.** If it holds the last reference, `Drop` frees a
multi-megabyte buffer inside the callback. So `RtCore::recycle` pushes retired
buffers onto a bounded queue (`self.garbage`) that the host thread drains, and
if that queue is somehow full it `std::mem::forget`s the buffer — a bounded
leak is strictly better than a missed deadline. Note that
`crossbeam-queue::push` *returns* the value on overflow, so `let _ =` there
would drop it in exactly the place we are avoiding.

*Enforced by* the `the_callback_never_allocates*` family and
`the_stream_error_callback_neither_allocates_nor_logs` in `engine.rs`: they arm
a counting global allocator **and** install an allocating logger (`test_alloc`
/ `test_log` in `lib.rs`) around a realistic block — two decks, live EQ,
crossfade in flight, a full command queue — and *replace* decks inside the
measured region, because dropping a retired buffer is the deallocation a
callback performs by accident. `replacing_a_deck_recycles_instead_of_freeing`
pins the recycle path itself. Real-time faults are therefore **counted, not
logged**; `src-tauri/src/frame.rs` drains the counters and does the logging
(SPEC §13).

### 3.2 The mock backend and Rust must agree — a divergence has already shipped a bug

**This is the most important paragraph in this document.** Onyx has two
implementations of its own backend. They drifted on one rule — *assigning a
track to deck B turns A/B on* — and the drift hid a real bug. In the preview,
assigning to B "worked": the mock loaded the deck but left A/B off, so lane B
was never drawn, and every screenshot-based verification of deck B **passed**.
On the real macOS build the user reported *"deck b is not assignable, only a"*.

So the rules live in checked-in fixtures that both sides are held to:

* `src-tauri/tests/fixtures/ab_assign_contract.json` →
  Rust `src-tauri/tests/ab_assign_contract.rs` (`cargo test`) **and**
  TS `scripts/check-ab-parity.mjs` (`npm run check:ab`), which drives the real
  transpiled `mock.ts` end to end and reads the other rules — playlist removal,
  and `AppState::blind_guard`'s refusals — straight out of the Rust source.
* `src-tauri/tests/fixtures/theme_doc_contract.json` → `settings.rs` and
  `check-theme.mjs`, for theme-document storage.

**Rule: any behaviour change in `src-tauri/` that the preview can observe
changes `src/lib/mock.ts` in the same commit.** A preview whose backend
disagrees with the engine proves nothing.

The *existence* half of that rule is mechanical, and is now checked as such:
`scripts/check-ipc.mjs` (`npm run check:ipc`, inside both builds) holds SPEC
§3.1, `generate_handler!`, the `api.ts` wrappers and the mock's dispatch labels
equal in every direction. The mock throws on an unknown command, but until that
check existed nothing counted its cases, so a command added to Rust and to
`api.ts` without a mock case passed every gate there was and threw
`mock: unknown command "…"` the first time the preview called it. What a mock
case *does* is still yours to get right — only that it exists, and dispatches,
is automatic.

### 3.3 The theme-token contract is generated, never written twice

`src/styles/tokens.css` is the single source of truth for both designed themes.
`src/lib/tokens.ts` imports it with Vite's `?raw` and *parses* it, so the theme
document you export cannot claim a default the stylesheet does not have; only
the metadata a machine cannot infer (group, one-line purpose, numeric range) is
hand-written there. `src-tauri/src/surface.rs` `include_str!`s the same sheet.
A token added to `tokens.css` and not mentioned in the catalogue still appears
in the document, typed by inference, under "other" — the contract can be
missing prose, never a token.

*Enforced by* `npm run check:theme`, in `npm run build`: catalogue vs sheet in
both directions, every default parses, parsing is a fixed point,
export→import→export is identity, and the imported default applies the same
custom properties the stylesheet already has. Never hard-code a colour in a
component, a canvas call or Rust; see [THEMING.md](THEMING.md) for what to call
instead.

### 3.4 Blind-test integrity

The mapping for the current trial is **never serialised while a test is
active** (`src-tauri/src/blind.rs`): the front end cannot leak what it never
receives, so the mapping fields are `None` until the run finishes. Separately,
`AppState::blind_guard` refuses every deck-changing command mid-test with one
sentence naming the action and the way out, because a deck swapped or re-trimmed
mid-trial turns the remaining votes into noise *without anything looking wrong*.
Results carry a one-tailed exact binomial p-value: a score with no p-value next
to it invites exactly the wrong conclusion.

*Enforced by* the tests in `blind.rs` and `state.rs`, and by `check:ab`, which
compares the mock's refusals against the Rust guard one by one.

### 3.5 Bit transparency

`bitTransparent` may be reported only when *all* of SPEC §2.5 holds: engine rate
== source rate, PCM stored at that rate, EQ transparent, no audition bandpass,
`monitorMode == "stereo"`, deck not inverted, not muted, volume 1.0, trim 0. And
the default path must be transparent *in fact*, not "1.0 multiplied in": gain
stages early out.

*Enforced by* `the_default_path_is_bit_transparent` (sample-for-sample equality
through the whole chain) in `engine.rs`, the `*_is_bit_transparent` tests in
`dsp/eq.rs`, `a_source_at_the_device_rate_is_bit_transparent` in
`tests/resampler_quality.rs`, and a per-format assertion inside
`tests/format_coverage.rs`. `level_matching_is_unity_while_disabled` in
`state.rs` covers the opt-in half.

### 3.6 No nested locks, and no lock held across an `emit` or a blocking call

`AppState` holds several independent `parking_lot` mutexes (playlist, decks,
A/B, blind, settings). Two are never held at once, and one is never held
across a Tauri `emit` or any blocking call — that is how this app would
deadlock its own window between the frame pump, a command and a decode watcher.
The pattern to copy is `AppState::remember_settings`: read *everything* out of
the engine and the A/B config first, then take the settings lock. Comments
there say so explicitly; keep them true. SPEC §9.5 makes this an audit
requirement. Every command is `async` for the same family of reasons: a
synchronous Tauri command runs on the main thread, and opening a file or
re-clocking the device must never block the event loop.

---

## 4. Three recipes the earlier agents had to work out

### 4.1 Adding an IPC command, end to end

Nothing here is checked by the compiler — a renamed command or argument is a
runtime *"command not found"* in a shipped app — so it is checked by tests that
read both sides. Do all six steps, in one commit:

1. **SPEC §3.1.** Add the row: snake_case command name, camelCase payload keys,
   `Result<T, String>`. If the return type is new, add its shape to §3.3. A
   command not in §3.1 does not exist; §3.1 is the list, not a summary of one.
2. **`src-tauri/src/commands.rs`.** `#[tauri::command] pub async fn <name>(…)`
   returning `Res<T>`. `async` is mandatory. No `unwrap` on anything reachable
   from user input; never return `Ok` after a partial failure. If it changes a
   deck, call `state.blind_guard("<Capitalised subject>")` first (§3.4).
3. **`src-tauri/src/lib.rs`.** Add `commands::<name>` to `generate_handler![…]`.
   Forgetting this is the classic failure and is exactly what the test catches.
4. **`src/lib/types.ts`.** Mirror any new payload or return shape. Rust is the
   authority; TS mirrors it.
5. **`src/lib/api.ts`.** One thin wrapper, calling the shared
   `call("<name>", { … })` helper. **Every** command gets a wrapper even if
   nothing calls it yet — a stray ad-hoc `invoke("…")` in a component is how
   casing drift starts. No retries, no global error handling: the caller decides
   what a failure means (usually a toast).
6. **`src/lib/mock.ts`.** Add the `case "<name>":` to the `switch` in `invoke`.
   Implement the *behaviour*, not a stub, if the preview or a harness can
   observe it (§3.2). A command with no mock case fails `npm run check:ipc`; a
   command that genuinely cannot be reached from the preview goes in that
   script's `MOCK_EXEMPT` map, with the reason beside it (the map is empty
   today, and an exemption for a command that *does* have a case fails too).

Then `cargo test --workspace` proves it. In `src-tauri/src/lib.rs`'s test
module, `every_front_end_command_is_registered_and_nothing_extra_is` parses the
`call("…")` sites out of `api.ts` and the `generate_handler!` block out of
`lib.rs` and holds the two sets equal in **both** directions — a registered
command with no wrapper fails too — and checks a matching `pub async fn` exists
in `commands.rs`. `every_front_end_argument_name_exists_on_the_rust_command`
camel→snake-cases each argument key and asserts the parameter exists, because a
typo there arrives as `null` on the Rust side.

`npm run check:ipc` proves the other two sides in the same shape: SPEC §3.1's
table (including the count its prose states) and the mock's `case` labels, read
out of the `switch (cmd)` inside `invoke` with the TypeScript compiler's own
parser rather than a regex — a label in a comment or in some other switch would
satisfy a grep while dispatching nothing — and then every registered command is
invoked against the transpiled mock, because the claim is that the preview
*answers* it.

No capability file needs touching: application commands are not gated by
capabilities.

### 4.2 Adding an audio format

Read SPEC §17 first. The rule is that **every format Onyx claims is named
explicitly**, never inherited from a default feature set.

1. **`crates/onyx-core/Cargo.toml`.** Add the Symphonia feature to the
   `[dependencies.symphonia]` list — `default-features = false` is deliberate,
   so that a Symphonia release changing its defaults cannot silently drop a
   format we promise. The comment block above the list documents what each
   feature buys; extend it. If Symphonia demuxes but does not decode the codec
   (as with Opus), the decoder is ours and must be registered into
   `decode::codecs()`, not `symphonia::default::get_codecs()`. Consider
   `[profile.dev.package.…] opt-level = 2` in the workspace `Cargo.toml` if the
   new decoder is slow in debug — under-running `tauri dev` is the symptom.
2. **`crates/onyx-core/src/container.rs`.** This module is *labelling*, not
   demuxing: Symphonia still picks the decoder. Add a `Container` variant, its
   short upper-case `label()` (the `MP4` of the UI's `MP4 · AAC` badge), a
   `sniff()` branch keyed on magic bytes, and set `may_carry_video()` if the
   container carries picture. Everything works on a bounds-checked byte prefix
   (`SNIFF_BYTES`) and returns `None` rather than guessing — the sniffer is the
   first thing a hostile file meets. The badge must be honest: a `.wav` that is
   really an MP3 reads `MP3`, so **content decides the label, never the
   extension**.
3. **`decode::SUPPORTED_EXTENSIONS`.** The accepted-extension filter, behind
   `is_supported_path`, the file dialog and the drag-and-drop filter (via
   `playlist::openable_extensions`, which adds the archive extensions).
4. **`src-tauri/tauri.conf.json` `bundle.fileAssociations`** — *only* if Onyx
   should claim the type in Finder/Explorer. This is deliberately the **smaller**
   set (SPEC §5): video containers and codec-shaped extensions decode but are
   not claimed. A test asserts every claimed extension is one
   `openable_extensions()` really opens, so the association list can never
   over-promise.
5. **Fixtures.** `scripts/make-format-fixtures.sh` is the record of how the
   checked-in fixtures in `crates/onyx-core/tests/fixtures/` were generated with
   ffmpeg (they are committed because no CI machine has encoders). Every one is
   the same tiny programme — 440 Hz left, 660 Hz right, 0.5 s — so a test can
   assert channel identity as well as rate, channels and duration. Follow that
   exactly; the assertions depend on it.
6. **Tests.** In `crates/onyx-core/tests/format_coverage.rs` add one `#[test]`
   using the shared `check()` helper with an `Expect { rate, channels,
   container, codec, lossless }`: it probes, asserts the metadata, decodes at
   the file's own rate, requires `bit_transparent`, waits for the decode, and
   checks frame count, per-channel RMS, waveform peaks, loudness analysis and
   the dominant frequency of each leg. Then, if the format is **lossy or
   carries an edit list**, add a case to `crates/onyx-core/tests/gapless.rs`:
   encoder priming at the head and padding at the tail must be discarded to the
   sample, so the frame count is the nominal duration exactly *and* the decoded
   waveform correlates at offset zero against a sine generated from scratch. Get
   that wrong and every loop point, A/B comparison and gapless transition
   inherits the error — a click at the loop, a flam against the other deck.
   Hostile files are already covered by `src-tauri/tests/malformed_input.rs` and
   the sniffer's own `hostile_prefixes_never_panic`; add to them if the new
   parser has a new way to be lied to.
7. Update README's *What it plays* and SPEC §17.

### 4.3 The `surface.rs` seam — the one non-obvious piece of plumbing

**The problem.** A native window has a background colour of its own,
underneath the webview. Left untold, that colour is the system's
(`windowBackgroundColor` on macOS) — never obsidian, never alabaster. It is
invisible where the webview covers it, and the webview covers nearly
everything. Where it is *not* covered is the outside edge: macOS clips a
decorated window to rounded corners and antialiases that curve against the
window's own background, and for the moment between "window on screen" and
"webview's first frame" the background is all there is. Both read as **a pale
rim around a dark app**, which is what the user reported.

**The seam.** The colour cannot be a constant — two designed themes, plus
`system`, plus a theme document that can move any token — and Rust must not
parse a theme, because the theme grammar has exactly one implementation and it
is in TypeScript. So the resolved colour travels *from* the webview *to* Rust:

* `src/lib/surface.ts` — DOM-free, so `check-theme.mjs` can drive it in Node —
  turns candidate CSS values into an opaque `#rrggbb`, skipping `transparent`
  (a browser's way of saying "no background here", which says nothing about
  what the window should be). `SURFACE_TOKEN` is `ink-900`, the colour at the
  edge of all three documents, since `--bg-app` and `--bg-eq` are gradients
  *over* it.
* `src/main.tsx` reports it through `api.setWindowSurface(color, resolvedTheme())`
  on every path that can move it — theme change, OS appearance change under
  `system`, an applied or cleared document, the reset chord — via
  `onThemeChange`, plus once at module scope. **Only the main window reports:**
  it always exists and always wears the document; the theme editor
  deliberately never wears the document it is editing, so Rust gives that
  window the *designed* colour instead, or you would recover a bad theme from a
  window wearing it.
* `src-tauri/src/surface.rs` remembers the report *tagged with the theme it was
  resolved for*, and falls back to `designed(theme)`, parsed out of
  `tokens.css` at compile time with `include_str!`, for a window born before
  any webview has spoken — or for a report tagged with the *other* theme. The
  main window is declared in `tauri.conf.json` and therefore exists before any
  Rust runs, so its creation-time colour is a literal in the config, held equal
  to `tokens.css` by a test; the resolved colour is pushed in `setup`, before
  the run loop turns.

**What it does not claim to fix, and why it can't.** macOS draws its own 1 px
stroke around a *decorated* window. It belongs to AppKit, is shared with Finder
and every native app, and no window background colour removes it. The only fix
is `decorations: false`, which costs the traffic lights and native edge-resize
and would need custom resize zones written from scratch. **That trade has
deliberately not been taken.** The module's own doc comment records what the
pinned versions actually support, read out of the locked sources rather than
assumed — including that `wry`'s matching *webview* setter is implemented for
iOS only and is a silent no-op on macOS, which nothing here relies on. Read
that comment before touching this file.

---

## 5. What is deliberately unverified — absence of evidence, not evidence of absence

Everything in Onyx is backed by tests, checks and screenshots. **Nothing has
ever been heard.** This sandbox has no audio device, no display and no macOS,
and the app has never been run on any platform. Do not read a green suite as
"it works"; read it as "it is consistent with the contract".

CI moves one of these boundaries and no others. GitHub's macOS and Windows
runners now compile, link, clippy, test and bundle this tree, so "it builds on a
real Apple toolchain" and "the NSIS installer is produced" become facts the
first time [the workflows](.github/workflows/) run green — see them as evidence
about the *build*, never about the sound. A runner has no display and no output
device either. Everything below stays true until somebody installs a build and
listens to it.

Specifically unobserved:

* **Sound.** No audio device exists. MIDI has been rendered, decoded, measured
  for length, loudness, true peak and tail decay — and never *listened to*. The
  perceptual quality of the bundled GM bank is not something a test can speak
  to.
* **Device enumeration returns an empty list.** The "no devices found" path is
  therefore the *only* one exercised against reality. Picking a device, fixing a
  rate, changing the buffer, reading a real latency figure, and rebuilding the
  stream while playing are covered by tests and the mock preview only — never by
  CoreAudio, WASAPI or ALSA.
* **The native window frame.** `titleBarStyle: Overlay`, the traffic-light
  inset, what the frame looks like in Light and in Auto, and an OS appearance
  switch while running: all written to spec, none seen.
* **Real multi-window behaviour.** The EQ and editor windows are exercised end
  to end — but as *browser popups*. A popup cannot stand in for the OS: window
  restoration on launch, `always_on_top` pinning, Dock and Mission Control
  behaviour, focus stealing, and multi-monitor placement are unobserved. Treat
  the `Float` toggle as written-to-spec.
* **`.dmg`, the universal binary, `lipo`, signing, notarisation, `spctl`** have
  never run; nor has the Windows path (no MSVC link, no NSIS installer, no
  WebView2, no WASAPI). Both scripts have been re-read against the current
  config and their paths corrected. Neither has been *executed*. A Linux
  `.deb` build is what actually proves the bundle config, and it is not a
  shipping target.

**The open item.** The pale hairline the user reported is answered by §4.3 —
and only partly. Whether it is gone cannot be known here. If it remains, it is
AppKit's own stroke on a decorated window, removable only by going frameless at
the cost of the traffic lights and native edge-resize. That decision is open and
belongs to the user, not to an agent.

[README § Known limitations](README.md#known-limitations) is the authoritative,
longer list, and includes the *design* limits that are real and will not change
(RAM-resident audio, offset-only alignment, analysis needing a complete decode,
a rate change re-decoding, stereo fold, no ReplayGain/cue sheets/library, a
clock-seeded RNG for blind tests). Read it before promising anything.

---

## 6. How this codebase expects to be worked on

**Commits.** The history is the style guide (`git log`). A subject line is a
full, declarative sentence in the imperative — *what is now true*, not what was
touched: "No deck is audible at a rate the engine no longer runs at", "Key the
loudness cache to the decode, not just to the file", "Believe the file, or say
what is wrong with it". Docs-only or script-only commits take a bare `docs:` /
`scripts:` prefix. The body is where the work is: what the bug was, why the fix
is *this* fix, and what now enforces it — naming the test or harness assertion.
No ticket numbers, no `feat:`/`fix:` taxonomy.

**Docs are contract, and they move with the code.** SPEC.md is authoritative:
if you change behaviour, change SPEC in the same commit, or you have created a
divergence rather than a feature. Same for THEMING.md when the token layer
moves, README when a command or limitation changes, and this file when a new
invariant or seam appears. There is exactly one spec file — the v2 and v3
addenda were merged and deleted because reading one alone was actively
misleading.

**Assert the current truth, never a number that will rot.** The docs
deliberately do not state test counts, byte sizes or line numbers; they name
the command that prints the answer (`ls -l target/release/bundle/deb/*.deb`,
`cargo test --workspace`). Follow that. A stale number is worse than no number,
because it is believed.

**A comment that lies is a defect.** The comments in this tree are unusually
long because they carry the *reason* — the bug that motivated the code, the
trade-off taken, the version whose behaviour was read out of the locked
sources. Several are load-bearing (`recycle`, `surface.rs`, `loader.rs`'s three
ordering rules, `remember_settings`'s lock discipline). If you change the code
under one, change the comment. If you find one that no longer matches the code,
you have found a bug — fix it or say so, but do not step over it.

**Prefer failing the build to failing a screenshot.** When two implementations
of one rule exist, put the rule in a fixture and hold both to it. That is the
lesson of the deck-B bug (§3.2), and it is the reason `check:eq`, `check:ab`,
`check:theme` and `check:ipc` run inside `npm run build`.
