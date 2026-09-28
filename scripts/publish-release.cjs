const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const { prepareRelease } = require('./prepare-release.cjs');

module.exports = async function publishRelease({ github, context, directory, tag, sha, branchPush = false }) {
  if (!/^v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(tag)) throw new Error('Invalid tag');
  const version = tag.slice(1);
  const prepared = prepareRelease(directory, version);
  const repo = context.repo;
  const releases = await github.paginate(github.rest.repos.listReleases, { ...repo, per_page: 100 });
  let release = releases.find(item => item.tag_name === tag);
  let taggedSha;
  try {
    taggedSha = (await github.rest.repos.getCommit({ ...repo, ref: `tags/${tag}` })).data.sha;
  } catch (error) {
    if (!branchPush || error.status !== 404) throw error;
  }
  // A later branch push still builds artifacts, but never replaces a version
  // already shipped. Bump the version to publish the next release.
  if (release && !release.draft && taggedSha !== sha) {
    if (branchPush) {
      console.log(`Release ${tag} already exists; keeping published assets. New packages remain in workflow artifacts.`);
      return release.html_url;
    }
    throw new Error(`Release ${tag} is already published from another commit; use a new version`);
  }
  if (taggedSha && taggedSha !== sha) throw new Error('Release tag moved since verification; refusing upload');
  const body = [
    `Onyx ${version}`, '',
    '- macOS Big Sur 11.3+: universal Intel / Apple silicon disk image and app ZIP.',
    '- Windows 10/11 x64: setup installer. WebView2 installation requires internet if missing.',
    '- Source ZIP and SHA256SUMS are included.', '',
    `Signing: macOS **${prepared.mac}**; Windows **${prepared.windows}**.`, '',
    'Built automatically. See docs/releasing.md for platform verification and signing details.',
    `Built from commit ${sha}.`,
  ].join('\n');
  if (!release) {
    release = (await github.rest.repos.createRelease({ ...repo, tag_name: tag, target_commitish: sha,
      name: `Onyx ${version}`, draft: false, prerelease: version.includes('-'), body })).data;
  } else if (release.draft) {
    release = (await github.rest.repos.updateRelease({ ...repo, release_id: release.id,
      name: `Onyx ${version}`, draft: false, prerelease: version.includes('-'), body })).data;
  }
  const assets = await github.paginate(github.rest.repos.listReleaseAssets, { ...repo, release_id: release.id, per_page: 100 });
  const missing = prepared.files.filter(name => !assets.some(asset => asset.name === name));
  if (missing.length === 0) return release.html_url;
  // A rebuild may differ byte-for-byte. Never mix its checksums with old
  // downloads: only resume when all existing files match this complete set.
  for (const asset of assets.filter(asset => prepared.files.includes(asset.name))) {
    const digest = 'sha256:' + crypto.createHash('sha256')
      .update(fs.readFileSync(path.join(directory, asset.name))).digest('hex');
    if (asset.digest !== digest) throw new Error(`Existing asset differs or has no digest: ${asset.name}; restore the original run artifacts before resuming`);
  }
  for (const name of prepared.files) {
    const old = assets.find(asset => asset.name === name);
    // Resume an interrupted public upload by adding missing assets only.
    // Existing downloads remain immutable, including on a rebuild of this SHA.
    if (old) continue;
    const data = fs.readFileSync(path.join(directory, name));
    await github.rest.repos.uploadReleaseAsset({ ...repo, release_id: release.id, name, data,
      headers: { 'content-type': 'application/octet-stream', 'content-length': data.length } });
  }
  return release.html_url;
};
