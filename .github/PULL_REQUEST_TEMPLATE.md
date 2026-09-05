## What this changes

<!-- One paragraph. If it changes behaviour a user can hear or see, say what
     they will hear or see. -->

## Why

<!-- The problem, not the patch. Link the issue if there is one. -->

## Checks

Run what your change touches; CI runs all of it anyway.

- [ ] `cargo test --workspace`
- [ ] `cargo clippy --workspace --all-targets` — warning-free
- [ ] `cargo fmt --all`
- [ ] `npm run build` — tsc + `check:version` + `check:ipc` + `check:eq` + `check:ab` + `check:theme`
- [ ] Screenshots, if this is a UI change: `npm run shots`, `shots:theme`, `shots:themedoc` against a mock preview

## Invariants

CONTRIBUTING.md §3 lists what must not break. Tick the ones your change goes near.

- [ ] The audio callback still allocates nothing, locks nothing, logs nothing (§3.1)
- [ ] The mock backend and Rust still agree (§3.2) — a new command means `api.ts`, `generate_handler!`, the mock's dispatch and SPEC §3.1
- [ ] Theme tokens are still generated, not written twice (§3.3)
- [ ] Blind-test integrity is intact (§3.4)
- [ ] Bit transparency is intact (§3.5)
- [ ] No nested locks, and no lock held across an `emit` or a blocking call (§3.6)

## Anything unverified

<!-- This tree is honest about what has not been observed running. If your
     change is written to spec and untested on hardware, say so here rather
     than leaving it for the release notes. -->
