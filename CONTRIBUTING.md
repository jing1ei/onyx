# Development

Onyx uses a React/TypeScript frontend, a Tauri shell, and the Rust audio engine
in `crates/onyx-core`. Product contracts live in [SPEC.md](SPEC.md); theme
implementation guidance lives in [THEMING.md](THEMING.md).

## Setup and checks

Install Node.js ^20.19 or >=22.12, Rust stable, and the platform prerequisites
listed in [README.md](README.md#development). On Debian/Ubuntu, install
`libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev libayatana-appindicator3-dev
libasound2-dev libssl-dev pkg-config build-essential`.

```sh
npm ci
npm run tauri dev
# Browser preview without an audio device:
npm run dev:mock
```

Before submitting changes:

```sh
npm run build
npm run build:mock
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
```

The frontend builds run type checking and the version, IPC, EQ, A/B, and theme
contract checks. These exercise the real modules and shared fixtures. Rust
checks cover the engine, persistence, malformed media, and archives. The wide
alignment sweep is ignored by default; run it explicitly in release mode when
changing alignment. Passing tests does not verify audible output or hardware
behavior.

For browser interaction and visual checks:

```sh
npx playwright install chromium webkit
npx vite preview --outDir dist-mock --port 4173
# In another terminal:
npm run check:ui
npm run check:motion
ONYX_BROWSER=webkit npm run check:ui
ONYX_LEGACY_WEBKIT=1 npm run check:ui
npm run shots
npm run shots:theme
npm run shots:themedoc
```

Screenshots are written to ignored `shots/`. Inspect them as well as the check
results. Native window behavior, audio devices, sample-rate changes, signing,
and installers require their respective platforms. Release build instructions
are in [README.md](README.md#building-releases).

## Implementation constraints

- The audio callback must not allocate, deallocate, block, or log. Retired PCM
  buffers go through the recycling queue rather than being freed on the audio
  thread. Preserve the allocation-counting regression tests.
- Keep Rust and the mock backend behavior consistent. An IPC change updates
  `commands.rs`, its registration, `src/lib/api.ts`, `src/lib/mock.ts`, and the
  command table in SPEC. Shared fixtures cover A/B assignment and theme storage.
- Keep the blind mapping hidden until a test ends. Deck-changing commands must
  use the backend blind guard; frontend hiding alone is insufficient.
- Preserve the exact conditions for bit transparency in SPEC §2.5. Gain, EQ,
  inversion, audition, monitor folds, mute, and sample-rate conversion all
  affect the claim.
- Do not nest state locks or hold them across event emission or blocking calls.
  Snapshot engine values before locking settings. Keep disk writes off the
  audio thread and event loop.
- Use `src/styles/tokens.css` for designed colors. The theme catalogue and Rust
  window-surface code consume that same stylesheet. Follow THEMING.md rather
  than duplicating literals in components or canvas painters.
- Use `useFrameEffect` for canvas subscriptions. Per-frame transport and meter
  data stay outside React state. Cancel listeners and ignore asynchronous work
  after disposal.
- Test keyboard access, reduced motion, narrow windows, and error recovery.
  Browser preview verifies UI behavior, not CoreAudio/WASAPI or OS window rules.

Keep dependency locks, media fixtures, the bundled SoundFont, and its license.
[THIRD-PARTY.md](THIRD-PARTY.md) records asset provenance. The project itself
has no license grant; contributions require the owner's authorization.

## README screenshots

After `npm run build:mock`, serve `dist-mock` on port 4173, then run
`npm run shots:readme`. This uses the real UI with demo data and writes the five
tracked PNGs in `docs/screenshots/`. Keep the preview-data caption in README.
Inspect every screenshot before committing. These images document visible
features; they do not verify native windows or real audio output.
