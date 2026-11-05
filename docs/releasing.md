# Building and releasing

[← Overview](../README.md) · [Developer setup](../CONTRIBUTING.md)

## Local packages

Run `./scripts/build-mac.sh` on macOS for a universal `.app` and `.dmg`
(output: `target/universal-apple-darwin/release/bundle/`), or
`./scripts/build-mac.sh --arm64` for an Apple-silicon-only build in about half
the time (output: `target/aarch64-apple-darwin/release/bundle/`). The script
sets deployment target 11.3 and checks the CPU slices and the bundle's minimum
system version.

Run `.\scripts\build-windows.ps1` on Windows for an x64 NSIS installer.
Add `-Msi` for WiX output. Packages appear under
`target/x86_64-pc-windows-msvc/release/bundle/`.
The installer embeds the WebView2 bootstrapper; installation of a missing runtime
requires internet. It is not an offline runtime bundle.

Linux: `npx tauri build --bundles deb` is a development smoke test, not a
shipping-platform claim. Install the GTK/WebKit and ALSA build dependencies
listed in CONTRIBUTING.md.

## Automatic releases

Every push to `main` runs CI and the Release workflow
(`.github/workflows/release.yml`):

1. **verify** — version consistency, front-end checks, `cargo fmt`, `clippy`,
   `cargo test`, and the source ZIP.
2. **build** — the universal macOS `.dmg` / `.app.zip` and the Windows x64
   installer, each verified before upload.
3. **publish** — `.github/scripts/release.py` collects the packages named in
   `.github/release.json`, writes `SHA256SUMS.txt`, uploads everything into a
   draft release, downloads it again to verify every checksum, creates the
   `v<version>` tag at the built commit, and publishes.

Release assets are named `Onyx-<version>-macOS-universal.dmg`,
`Onyx-<version>-macOS-universal.app.zip`, `Onyx-<version>-Windows-x64.exe`, and
`Onyx-<version>-source.zip`, plus the two signing-status files and
`SHA256SUMS.txt`. Workflow artifacts are kept for 14 days.

Rules the publisher enforces:

- **A published version is immutable.** A later push with the same version
  still builds and keeps its packages as workflow artifacts, but leaves the
  public release untouched.
- **Tags never move.** If `v<version>` already points at another commit, the
  publish job fails; bump the version.
- **Only the newest `main` publishes.** A build that has been overtaken by a
  newer commit stops before changing anything.
- **Interrupted uploads resume.** A failed upload leaves a draft; rerunning the
  same commit completes it.

To ship an update, change the version together in `package.json`,
`package-lock.json`, workspace `Cargo.toml`, `Cargo.lock` (run `cargo check`),
and `src-tauri/tauri.conf.json`; add a CHANGELOG entry; run
`npm run check:version`; push to `main`.

### First push to a new repository

Extract the ZIP, open its `onyx-<version>` folder, create an empty GitHub repository,
then run inside the folder, replacing `YOUR-OWNER/YOUR-REPO`:

```sh
git init -b main
git add .
git commit -m "Onyx 1.0.0"
git remote add origin https://github.com/YOUR-OWNER/YOUR-REPO.git
git push -u origin main
```

Keep `.github` at the repository root. GitHub Actions must be enabled; the
publish job uses the automatic `GITHUB_TOKEN` with `contents: write`. No
personal token or signing secrets are needed for unsigned packages.

## Signing

Local Mac signing uses `APPLE_SIGNING_IDENTITY`; see the build script for
notarisation options. The GitHub release workflow accepts these secrets:

| Purpose | Secrets |
|---|---|
| Developer ID signing | `APPLE_CERTIFICATE` (base64 `.p12`), `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY` |
| Notarisation | `APPLE_ID`, `APPLE_PASSWORD` (app-specific password), `APPLE_TEAM_ID` |

No Apple secrets produces an **ad-hoc-signed** bundle (`build-mac.sh` signs
it locally so `codesign --verify` passes); it is not notarised, so Gatekeeper
still requires right-click → Open the first time. Partial signing or
notarisation credentials fail the build. Signed builds verify the app and DMG
signatures;
notarised builds also verify the app's stapled ticket and Gatekeeper acceptance.
Tauri notarises the app inside the DMG, not the DMG itself.

Windows Authenticode settings live in `bundle.windows` in tauri.conf.json
(certificate thumbprint, digest, timestamp). Tauri updater keys are separate
from executable signing. Without signing configuration, builds are unsigned.
Windows signing also requires provisioning the certificate or signing service
on the runner; this repository does not configure one. The workflow checks
both the executable and installer signatures before upload.

Do not publish a debug build as a release.

## Platform verification

Test installers, audio output, device changes, file opening, and window behavior
on Intel Big Sur 11.3+ and Windows 10 before distributing binaries. Browser
previews and cross-compilation do not replace those checks.

## Source archive

Run `npm run package:source` after verification (requires Python 3). The ZIP contains source,
lockfiles, tests, documentation, and required assets. It excludes Git history,
local settings, dependencies, and build output. A SHA-256 file accompanies it.
