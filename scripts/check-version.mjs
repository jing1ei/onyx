#!/usr/bin/env node
//
// The version lives in four places and a release is cut from a git tag, so
// there are five things that can disagree:
//
//   package.json                "version"
//   package-lock.json           "version", twice (root + the root package entry)
//   Cargo.toml                  [workspace.package] version   (both crates inherit it)
//   src-tauri/tauri.conf.json   "version"  — the one the bundler stamps on the
//                               .dmg and the NSIS installer, and the one a user
//                               sees in "About"
//   the tag                     v<version>, when called with one
//
// A mismatch is invisible until an installer is already on someone's disk
// carrying the wrong number, so this check is a release gate
// (.github/workflows/release.yml) as well as an ordinary check.
//
//   node scripts/check-version.mjs            # the four files agree
//   node scripts/check-version.mjs v1.2.3     # …and they agree with the tag
//
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const read = (p) => readFileSync(join(root, p), "utf8");
const json = (p) => JSON.parse(read(p));

const found = [];

const pkg = json("package.json");
found.push(["package.json", "version", pkg.version]);

const lock = json("package-lock.json");
found.push(["package-lock.json", "version", lock.version]);
found.push(["package-lock.json", 'packages[""].version', lock.packages?.[""]?.version]);

found.push(["src-tauri/tauri.conf.json", "version", json("src-tauri/tauri.conf.json").version]);

// [workspace.package] version — read with a regex rather than a TOML parser so
// this script keeps the repository's "no dependency for a 40-line check" rule.
const workspace = read("Cargo.toml").match(
  /\[workspace\.package\][\s\S]*?^version\s*=\s*"([^"]+)"/m,
);
found.push(["Cargo.toml", "[workspace.package] version", workspace?.[1]]);

// Cargo.lock has to have been regenerated after a bump, or the release build
// fails on a dirty lockfile with --locked.
for (const name of ["onyx", "onyx-core"]) {
  const m = read("Cargo.lock").match(
    new RegExp(`name = "${name}"\\nversion = "([^"]+)"`),
  );
  found.push(["Cargo.lock", `${name} version`, m?.[1]]);
}

const tag = process.argv[2];
if (tag) {
  if (!/^v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(tag)) {
    console.error(`FAIL: "${tag}" is not a release tag — expected v<major>.<minor>.<patch>.`);
    process.exit(1);
  }
  found.push(["git tag", tag, tag.slice(1)]);
}

const width = Math.max(...found.map(([file]) => file.length));
for (const [file, field, value] of found) {
  console.log(`  ${file.padEnd(width)}  ${value ?? "(not found)"}   ${field}`);
}

const missing = found.filter(([, , v]) => !v);
if (missing.length) {
  console.error(
    `\nFAIL: no version found in ${missing.map(([f, k]) => `${f} (${k})`).join(", ")}.`,
  );
  process.exit(1);
}

const versions = new Set(found.map(([, , v]) => v));
if (versions.size !== 1) {
  console.error(`\nFAIL: ${versions.size} different versions: ${[...versions].join(", ")}.`);
  console.error("Bump every one of them together — see the release checklist in README.md.");
  process.exit(1);
}

console.log(`\nOK: version ${[...versions][0]}, the same in all ${found.length} places.`);
