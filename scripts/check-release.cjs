const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const crypto = require('node:crypto');
const { prepareRelease } = require('./prepare-release.cjs');
const publishRelease = require('./publish-release.cjs');

function fixture(version) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'onyx-release-'));
  for (const name of [`Onyx_${version}_universal.dmg`, `Onyx_${version}_universal.app.zip`,
    `Onyx_${version}_x64-setup.exe`, `onyx-${version}-source.zip`]) fs.writeFileSync(path.join(directory, name), 'fixture');
  const source = `onyx-${version}-source.zip`;
  const hash = crypto.createHash('sha256').update('fixture').digest('hex');
  fs.writeFileSync(path.join(directory, `${source}.sha256`), `${hash}  ${source}\n`);
  fs.writeFileSync(path.join(directory, 'macos-signing.txt'), 'unsigned\n');
  fs.writeFileSync(path.join(directory, 'windows-signing.txt'), 'unsigned\n');
  return directory;
}
function api({ published = false, existing = false, sha = 'verified', missingTag = false, complete = false, digest = 'sha256:' + crypto.createHash('sha256').update('fixture').digest('hex') } = {}) {
  const calls = [];
  const release = {id: 7, tag_name:'v1.0.0', draft:!published, html_url:'https://example.invalid/draft'};
  const repos = {
    getCommit: async () => { if (missingTag) throw Object.assign(new Error('Not Found'), {status:404}); return {data:{sha}}; },
    listReleases: 'releases', listReleaseAssets: 'assets',
    createRelease: async input => { calls.push(['create',input]); return {data:release}; },
    updateRelease: async input => { calls.push(['update',input]); return {data:release}; },
    deleteReleaseAsset: async input => { calls.push(['delete',input]); },
    uploadReleaseAsset: async input => { calls.push(['upload',input.name]); },
  };
  const github = {rest:{repos}, paginate: async endpoint => endpoint === 'releases'
    ? (existing || published ? [release] : []) : (!existing && !published ? [] : complete ? ['Onyx_1.0.0_universal.dmg','Onyx_1.0.0_universal.app.zip','Onyx_1.0.0_x64-setup.exe','onyx-1.0.0-source.zip','onyx-1.0.0-source.zip.sha256','macos-signing.txt','windows-signing.txt','SHA256SUMS'].map((name,id)=>({id,name})) : [{id:42,name:'Onyx_1.0.0_universal.dmg',digest}])};
  return {github,calls};
}
(async () => {
  const directory = fixture('1.0.0');
  const context = {repo:{owner:'owner',repo:'onyx'}};
  try {
    const manifest = prepareRelease(directory, '1.0.0');
    assert.equal(manifest.files.length, 8);
    const input = {context,directory,tag:'v1.0.0',sha:'verified'};
    let remote = api();
    await publishRelease({...input,github:remote.github});
    assert.equal(remote.calls[0][0], 'create');
    assert.equal(remote.calls[0][1].draft, false);
    assert.equal(remote.calls[0][1].prerelease, false);
    assert.equal(remote.calls.filter(c=>c[0]==='upload').length, 8);
    remote = api({existing:true});
    await publishRelease({...input,github:remote.github});
    assert.equal(remote.calls[0][0], 'update');
    assert.equal(remote.calls.filter(c=>c[0]==='delete').length, 0);
    for (const options of [{published:true,sha:'moved'}, {sha:'moved'}]) {
      remote = api(options);
      await assert.rejects(publishRelease({...input,github:remote.github}));
      assert.equal(remote.calls.length, 0, 'refusal must precede mutations');
    }
    remote = api({missingTag:true});
    await publishRelease({...input,branchPush:true,github:remote.github});
    assert.equal(remote.calls[0][1].draft,false,'first branch push publishes directly');
    assert.equal(remote.calls[0][1].target_commitish,'verified','new tag uses verified commit');
    remote = api({missingTag:true});
    await assert.rejects(publishRelease({...input,github:remote.github}));
    assert.equal(remote.calls.length,0,'explicit tag must exist');
    for (const branchPush of [false,true]) {
      remote = api({published:true,complete:true});
      await publishRelease({...input,branchPush,github:remote.github});
      assert.equal(remote.calls.length,0,'published rerun is idempotent');
    }
    remote = api({published:true});
    await publishRelease({...input,github:remote.github});
    assert.equal(remote.calls.filter(c=>c[0]==='upload').length,7,'resume missing public assets');
    assert.equal(remote.calls.filter(c=>c[0]==='delete'||c[0]==='update').length,0);
    remote = api({published:true,digest:'sha256:mismatch'});
    await assert.rejects(publishRelease({...input,github:remote.github}),/Existing asset differs/);
    assert.equal(remote.calls.length,0,'never combine old assets with rebuilt checksums');
    remote = api({published:true,sha:'previous'});
    await publishRelease({...input,branchPush:true,github:remote.github});
    assert.equal(remote.calls.length,0,'later branch push preserves released version');
    fs.unlinkSync(path.join(directory,'Onyx_1.0.0_x64-setup.exe'));
    remote = api();
    await assert.rejects(publishRelease({...input,github:remote.github}), /Incomplete/);
    assert.equal(remote.calls.length, 0);
  } finally { fs.rmSync(directory,{recursive:true,force:true}); }
  const beta = fixture('1.0.0-beta.1');
  try {
    const remote = api();
    await publishRelease({context,directory:beta,tag:'v1.0.0-beta.1',sha:'verified',github:remote.github});
    assert.equal(remote.calls[0][1].prerelease,true);
    fs.writeFileSync(path.join(beta,'onyx-1.0.0-beta.1-source.zip.sha256'),'bad checksum');
    assert.throws(()=>prepareRelease(beta,'1.0.0-beta.1'), /checksum mismatch/);
  } finally {fs.rmSync(beta,{recursive:true,force:true});}
  console.log('Release checks passed: complete packages, checksums, direct publication, first branch push, idempotent rerun, prerelease, missing platform, published release, moved tag.');
})().catch(error=>{console.error(error);process.exitCode=1;});
