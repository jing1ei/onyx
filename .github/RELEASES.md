# Automatic releases

Every push to `main` builds and tests Onyx, then creates or refreshes the
public release **1.0.0** at tag `v1.0.0`. Manual runs on `main` do the same.
All required platforms must pass before publication. This deliberately updates
the same version instead of creating a new version for each commit.

Downloads use `App-1.0.0-OS-architecture.ext`: `macOS` or `Windows`, with
`universal`, `arm64`, `x64`, or `x86` as appropriate. Source archives use
`App-1.0.0-source.zip`. SHA256SUMS.txt covers every attached asset.

The publisher checks the build commit against current `main`, uploads into a
draft, downloads and verifies every asset, then moves `v1.0.0` to the built
commit and publishes. A failed upload stays draft and can be retried. Older
builds cannot overwrite a newer main commit. Only the publication job receives
`contents: write`; pull requests do not publish.

This policy supersedes older tag-only or immutable-release instructions.

## Required packages

- `Onyx-1.0.0-macOS-universal.dmg`
- `Onyx-1.0.0-macOS-universal.app.zip`
- `Onyx-1.0.0-Windows-x64.exe`
- `Onyx-1.0.0-source.zip`
