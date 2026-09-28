const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');

function prepareRelease(directory, version) {
  if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(version)) throw new Error('Invalid release version');
  const expected = [
    `Onyx_${version}_universal.dmg`, `Onyx_${version}_universal.app.zip`,
    `Onyx_${version}_x64-setup.exe`, `onyx-${version}-source.zip`,
    `onyx-${version}-source.zip.sha256`, 'macos-signing.txt', 'windows-signing.txt',
  ];
  const actual = fs.readdirSync(directory).filter(name => name !== 'SHA256SUMS').sort();
  if (JSON.stringify(actual) !== JSON.stringify([...expected].sort())) {
    throw new Error(`Incomplete or unexpected release files: ${actual.join(', ')}`);
  }
  const hashes = new Map();
  for (const name of expected) {
    const file = path.join(directory, name);
    const stat = fs.lstatSync(file);
    if (!stat.isFile() || stat.size === 0) throw new Error(`Empty or invalid artifact: ${name}`);
    hashes.set(name, crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex'));
  }
  const source = `onyx-${version}-source.zip`;
  if (fs.readFileSync(path.join(directory, `${source}.sha256`), 'utf8').trim() !== `${hashes.get(source)}  ${source}`) {
    throw new Error('Source checksum mismatch');
  }
  const mac = fs.readFileSync(path.join(directory, 'macos-signing.txt'), 'utf8').trim();
  const windows = fs.readFileSync(path.join(directory, 'windows-signing.txt'), 'utf8').trim();
  if (!['unsigned', 'signed', 'signed and notarized'].includes(mac) || !['unsigned', 'signed'].includes(windows)) {
    throw new Error('Invalid signing status');
  }
  fs.writeFileSync(path.join(directory, 'SHA256SUMS'), expected.map(name => `${hashes.get(name)}  ${name}\n`).join(''));
  return { files: [...expected, 'SHA256SUMS'], mac, windows };
}
module.exports = { prepareRelease };
if (require.main === module) {
  console.log(prepareRelease(process.argv[2], process.argv[3]));
}
