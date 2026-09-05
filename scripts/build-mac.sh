#!/usr/bin/env bash
#
# Build the macOS release of Onyx: a universal (Apple silicon + Intel) .app and
# a .dmg. Run this on macOS — cross-compiling a Cocoa/WKWebView app from Linux
# is not supported by Tauri.
#
#   ./scripts/build-mac.sh
#
# `scripts/build-windows.ps1` is this script's opposite number and checks the
# same things in the same order; keep the two in step.
#
set -euo pipefail

cd "$(dirname "$0")/.."

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "error: the macOS bundle can only be produced on macOS (found $(uname -s))." >&2
  exit 1
fi

# ── preflight ────────────────────────────────────────────────────────────────
# Check every tool up front and report *all* that are missing, rather than
# dying on the first one and making the reader run the script four times to
# discover four problems. `set -e` would otherwise turn a missing `rustup`
# into a bare "command not found" from the middle of a target loop.
missing=0
need() {                                  # need <command> <how to install it>
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "error: '$1' not found — $2" >&2
    missing=1
  fi
}

need xcrun  "install the Xcode command line tools: xcode-select --install"
need rustup "install Rust: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
need cargo  "install Rust (see above); if it is installed, add ~/.cargo/bin to PATH"
need node   "install Node.js 20.19+ or 22.12+: brew install node"
need npm    "install Node.js (npm ships with it)"

# Vite 7 requires Node ^20.19 || >=22.12 and exits with an opaque syntax error
# on older runtimes, so name the real problem here instead.
if command -v node >/dev/null 2>&1; then
  node_version="$(node -v)"                      # e.g. v22.23.2
  node_version="${node_version#v}"
  node_major="${node_version%%.*}"
  node_rest="${node_version#*.}"
  node_minor="${node_rest%%.*}"
  if (( node_major < 20 )) \
    || (( node_major == 20 && node_minor < 19 )) \
    || (( node_major == 21 )) \
    || (( node_major == 22 && node_minor < 12 )); then
    echo "error: Node $node_version is too old for Vite 7 — need 20.19+ or 22.12+." >&2
    missing=1
  fi
fi

if (( missing )); then
  echo >&2
  echo "Install what is listed above and run this script again." >&2
  exit 1
fi

# ── the things the build itself assumes are on disk ──────────────────────────
# The General MIDI bank of SPEC §18 is linked into the binary with
# `include_bytes!`, so a checkout without it fails deep inside `cargo build`
# with a path nobody recognises; the licence text is a bundle resource named by
# tauri.conf.json and would fail later still, during bundling. The front end is
# three documents (§5), each with its own capability file (§5.1) — a window
# whose capability is missing builds fine and then cannot subscribe to an
# event, which is a bug report rather than a build error.
absent=0
require_file() {                          # require_file <path> <why it matters>
  if [[ ! -s "$1" ]]; then
    echo "error: $1 is missing or empty — $2" >&2
    absent=1
  fi
}

require_file crates/onyx-core/assets/gm/GeneralUser-GS.sf2 \
  "the bundled General MIDI bank (SPEC §18, recorded in THIRD-PARTY.md), linked into the binary — restore it with: git checkout -- crates/onyx-core/assets/gm/GeneralUser-GS.sf2"
require_file crates/onyx-core/assets/gm/GeneralUser-GS-LICENSE.txt \
  "tauri.conf.json ships it as a bundle resource"
for doc in index.html eq.html theme.html; do
  require_file "$doc" "vite.config.ts builds it as an entry point (SPEC §5)"
done
for cap in default eq theme; do
  require_file "src-tauri/capabilities/$cap.json" \
    "each window needs its own capability whitelist (SPEC §5.1)"
done

if (( absent )); then
  echo >&2
  echo "Restore the files listed above and run this script again." >&2
  exit 1
fi

# Two architectures of a release build plus the bundling step need room; the
# workspace target/ directory reaches roughly 10–15 GB. Warn rather than
# refuse — the number is an estimate, not a contract.
free_gb="$(df -g . 2>/dev/null | awk 'NR==2 {print $4}' || true)"
if [[ -n "${free_gb:-}" ]] && (( free_gb < 20 )); then
  echo "warning: only ${free_gb} GB free on this volume; a universal build needs ~15 GB." >&2
fi

# ── toolchain ────────────────────────────────────────────────────────────────
for target in aarch64-apple-darwin x86_64-apple-darwin; do
  if ! rustup target list --installed | grep -qx "$target"; then
    echo "▸ installing Rust target $target"
    rustup target add "$target"
  fi
done

# ── front end ────────────────────────────────────────────────────────────────
if [[ ! -d node_modules ]]; then
  echo "▸ npm install"
  npm install
fi

# ── optional signing / notarisation ──────────────────────────────────────────
# Unsigned builds run fine locally (right-click → Open the first time) but will
# be quarantined when downloaded. To ship, export these before running:
#
#   export APPLE_SIGNING_IDENTITY="Developer ID Application: Your Name (TEAMID)"
#   export APPLE_CERTIFICATE="$(base64 -i certificate.p12)"   # CI only
#   export APPLE_CERTIFICATE_PASSWORD="…"                     # CI only
#
# and, for notarisation (Tauri staples the ticket automatically):
#
#   export APPLE_ID="you@example.com"
#   export APPLE_PASSWORD="app-specific-password"
#   export APPLE_TEAM_ID="TEAMID"
#
# or, with a stored App Store Connect API key:
#
#   export APPLE_API_ISSUER="…" APPLE_API_KEY="…" APPLE_API_KEY_PATH="AuthKey_….p8"

echo "▸ npm run tauri build -- --target universal-apple-darwin --bundles app,dmg"
npm run tauri build -- --target universal-apple-darwin --bundles app,dmg

# NOTE: this is a cargo *workspace* (root Cargo.toml with members src-tauri and
# crates/onyx-core), so the target directory is at the repo root, not under
# src-tauri/.
out="target/universal-apple-darwin/release/bundle"
app="$out/macos/Onyx.app"          # named after bundle.productName ("Onyx")

# Globs rather than `ls`: a path with a space in it (a repo checked out under
# "~/Audio Projects/…") comes back as two broken words from `ls | head`.
first_match() {
  local match
  for match in "$@"; do
    if [[ -e "$match" ]]; then
      printf '%s' "$match"
      return
    fi
  done
}

dmg="$(first_match "$out"/dmg/*.dmg)"

# The executable inside the bundle is named after the cargo binary
# (src-tauri/Cargo.toml `package.name` = "onyx"), not after productName, so
# discover it instead of hard-coding a name that can silently be wrong.
exe="$(first_match "$app"/Contents/MacOS/*)"

# All three documents have to be *in* the bundle, not merely buildable: a
# missing eq.html turns `E` into a blank window at runtime.
shipped=0
for doc in index.html eq.html theme.html; do
  if [[ -s "dist/$doc" ]]; then
    shipped=$(( shipped + 1 ))
  fi
done
if (( shipped != 3 )); then
  echo "warning: dist/ carries $shipped of the 3 documents — the EQ (§12) or theme editor (§20) window would come up blank." >&2
fi

echo
echo "✓ done"
echo "  app : $app"
echo "  dmg : ${dmg:-"(not produced — check the log above)"}"
echo
if [[ -n "$exe" ]]; then
  echo "  Architectures:"
  lipo -archs "$exe" | sed 's/^/    /'
fi

# The file types Finder will offer "Open With ▸ Onyx" for, read out of the
# config rather than repeated here: a second copy of the list is a second thing
# to rot, and the bundle only ever reads the config.
echo "  File types registered by this bundle:"
# shellcheck disable=SC2016  # ${…} below is a JS template literal, not a shell expansion
node -e '
  const conf = require("./src-tauri/tauri.conf.json");
  for (const a of conf.bundle.fileAssociations ?? []) {
    console.log(`    ${a.name}: ${a.ext.map((e) => "." + e).join(" ")}`);
  }
' || echo "    (could not read src-tauri/tauri.conf.json)"
echo "  Verify the signature (if you signed) with:"
echo "    codesign -dv --verbose=4 \"$app\""
echo "    spctl -a -vvv -t install \"$app\""
