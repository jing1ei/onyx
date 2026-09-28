# Security policy

## Reporting a vulnerability

**Do not open a public issue for a security problem.**

Use GitHub's private vulnerability reporting instead — the **Security** tab of
this repository → *Report a vulnerability*. It is private to the maintainers
until an advisory is published.

Include what you would want to receive: what an attacker can do, the file or
input that does it (a minimal one, attached, is worth a page of prose), and
whether it needs a user action such as opening a file or a zip.

There is no bounty, and no timetable I can promise. Expect an acknowledgement
rather than a fix on a schedule.

## Supported versions

The most recent release. Onyx is a desktop application with no server side and
no auto-updater: fixes reach you when you download the next build.

| Version | Supported |
|---|---|
| 1.0.x | yes |
| < 1.0 | no |

## What counts

Onyx opens files other people made, inside a webview, and treats both as
untrusted. The boundary is documented in the README's
[Security posture](README.md#security-posture) section — read it first, because
it says what is already deliberate.

In scope, and interesting:

- Anything that escapes the archive reader: a zip that writes outside its temp
  directory, follows a symlink, or exhausts the disk or memory limits
  (`src-tauri/src/archive.rs`, `src-tauri/tests/hostile_archives.rs`).
- Malformed media that reaches memory unsafety, an unbounded allocation or a
  process crash rather than an ordinary error toast
  (`src-tauri/src/safe_decode.rs`, `src-tauri/tests/malformed_input.rs`).
- Anything that widens the IPC surface of SPEC §3.1 from the renderer, or that
  gets the renderer a filesystem, dialog, opener or shell capability it is not
  granted in `src-tauri/capabilities/`.
- A path outside the playlist reaching `reveal_in_finder`, or any other
  OS-facing command accepting input it should have refused.
- A theme document (§20) that escapes the sanitiser — a token value that
  becomes script, or reaches anything but a CSS custom property.

Out of scope:

- Bugs in a *release build's* signing state: builds are unsigned unless the
  release was cut with the signing secrets set. That is a release-process
  property, documented in the release notes, not a vulnerability.
- Denial of service through a file you chose to open at a size the README's
  documented budgets already cover (whole files are decoded into RAM, 1 GiB
  per deck by default).
- The blind-test RNG's predictability. It is a clock-seeded xorshift64\*, said
  so in the README, and is not built to resist an adversary.
- Anything requiring an attacker who already runs code as your user.
