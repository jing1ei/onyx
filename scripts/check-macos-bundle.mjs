import assert from 'node:assert/strict';
import {execFileSync} from 'node:child_process';
import {join} from 'node:path';

const app = process.argv[2];
assert.ok(app, 'Usage: node scripts/check-macos-bundle.mjs path/to/Onyx.app');
const plist = join(app, 'Contents/Info.plist');
const readPlist = key => execFileSync('/usr/libexec/PlistBuddy', ['-c', `Print :${key}`, plist], {encoding:'utf8'}).trim();
assert.equal(readPlist('LSMinimumSystemVersion'), '11.3');
const binary = join(app, 'Contents/MacOS', readPlist('CFBundleExecutable'));
const arches = execFileSync('lipo', ['-archs', binary], {encoding:'utf8'}).trim().split(/\s+/);
assert.deepEqual(arches.sort(), ['arm64','x86_64']);
for (const arch of arches) {
  const loadCommands = execFileSync('otool', ['-arch', arch, '-l', binary], {encoding:'utf8'});
  const version = loadCommands.match(/\bminos\s+(\d+\.\d+)/)?.[1] ?? loadCommands.match(/LC_VERSION_MIN_MACOSX[\s\S]*?version\s+(\d+\.\d+)/)?.[1];
  assert.ok(version, `missing deployment target for ${arch}`);
  const [major,minor] = version.split('.').map(Number);
  assert.ok(major < 11 || (major === 11 && minor <= 3), `${arch} requires macOS ${version}`);
  const links = execFileSync('otool', ['-arch', arch, '-L', binary], {encoding:'utf8'});
  assert.ok(!/\/(opt\/homebrew|usr\/local|Users)\/.*\.dylib/.test(links), 'bundle links a developer-only library');
  console.log(`${arch}: minimum macOS ${version}, system libraries only`);
}
console.log('Universal bundle compatibility metadata passed.');
