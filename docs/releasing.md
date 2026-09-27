# Building and releasing

[← Overview](../README.md) · [Developer setup](../CONTRIBUTING.md)

## Local packages

Run `./scripts/build-mac.sh` on macOS for a universal `.app` and `.dmg`.
Output: `target/universal-apple-darwin/release/bundle/`.
The script sets deployment target 11.3 and checks both CPU slices and the
bundle's minimum system version.

Run `.\scripts\build-windows.ps1` on Windows for an x64 NSIS installer.
Add `-Msi` for WiX output. Packages appear under
`target/x86_64-pc-windows-msvc/release/bundle/`.
The installer embeds the WebView2 bootstrapper; installation of a missing runtime
requires internet. It is not an offline runtime bundle.

Linux: `npx tauri build --bundles deb` is a development smoke test, not a
shipping-platform claim. Install the GTK/WebKit and ALSA build dependencies
listed in CONTRIBUTING.md.

## Automatic builds and public releases

Every push to `main` or `master` runs CI and builds production packages for
macOS (universal Intel / Apple silicon) and Windows x64. When both platforms
and verification pass, the workflow **publishes the package version directly**
as a GitHub release, creating its `v<version>` tag at the verified commit.
There is no draft or manual publishing step. Version 1.0.0 is ready for the
first push.

Download from **Releases**, or from **Actions → Release → the successful run →
Artifacts** (`release-macos`, `release-windows`, and `release-source`, retained
for 14 days). The release includes the Mac DMG and app ZIP, Windows installer,
source ZIP, signing status, and SHA-256 checksums.

Published versions are not overwritten. A later branch push with the same
version still builds and uploads workflow artifacts, but keeps the existing
release. To publish an update, change the version together in `package.json`,
`package-lock.json`, workspace `Cargo.toml`, `Cargo.lock`, and
`src-tauri/tauri.conf.json`; update CHANGELOG.md and run `npm run check:version`.

Pushing a matching version tag also builds and publishes. Actions → Release →
Run workflow accepts an existing tag. Rerunning an already published commit preserves existing assets and uploads
any missing ones after an interrupted upload, provided existing asset digests
match the rebuilt set. If a rebuild differs, restore the original workflow
artifacts before resuming; packages from different builds are never mixed. A moved tag is rejected; never move a released tag.

### First push to a new repository

Extract the ZIP and open its `onyx-1.0.0` folder. Create an empty GitHub
repository, then run the following inside the folder, replacing
`YOUR-OWNER/YOUR-REPO`:

```sh
git init -b main
git add .
git commit -m "Initial release 1.0.0"
git remote add origin https://github.com/YOUR-OWNER/YOUR-REPO.git
git push -u origin main
```

Keep `.github` at the repository root, not inside an extra enclosing folder.
GitHub Actions must be enabled, and organization policies must allow the
configured actions and hosted runners. The publish job requests
`contents: write` using the automatic `GITHUB_TOKEN`. No personal token or
signing secrets are required for unsigned packages. No tag push is required.

Test installers, file opening, playback, A/B, EQ, and device switching on
supported hardware before distributing binaries. CI runners do not establish
behavior on Intel Big Sur 11.3 or Windows 10. A network/API failure during
public asset upload can leave an incomplete release; inspect the failed run
and rerun that same commit to upload missing assets. Existing downloads are preserved.

## Signing

Local Mac signing uses `APPLE_SIGNING_IDENTITY`; see the build script for
notarisation options. The GitHub release workflow accepts these secrets:

| Purpose | Secrets |
|---|---|
| Developer ID signing | `APPLE_CERTIFICATE` (base64 `.p12`), `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY` |
| Notarisation | `APPLE_ID`, `APPLE_PASSWORD` (app-specific password), `APPLE_TEAM_ID` |

No Apple secrets produces an unsigned build. Partial signing or notarisation
credentials fail the build. Signed builds verify the app and DMG signatures;
notarised builds also verify the app's stapled ticket and Gatekeeper acceptance.
Tauri notarises the app inside the DMG, not the DMG itself.

Windows Authenticode settings live in `bundle.windows` in tauri.conf.json
(certificate thumbprint, digest, timestamp). Tauri updater keys are separate
from executable signing. Without signing configuration, builds are unsigned.
Windows signing also requires provisioning the certificate or signing service
on the runner; this repository does not configure one. The workflow checks
both the executable and installer signatures before upload.

Do not publish a debug build as a release. Advisory scans (`cargo audit`,
`npm audit`) should use current databases; historical counts are not a verdict.

## Platform verification

Test installers, audio output, device changes, file opening, and window behavior
on Intel Big Sur 11.3+ and Windows 10 before distributing binaries. Browser
previews and cross-compilation do not replace those checks.

## Source archive

Run `npm run package:source` after verification (requires Python 3). The ZIP contains source,
lockfiles, tests, documentation, and required assets. It excludes Git history,
local settings, dependencies, and build output. A SHA-256 file accompanies it.
