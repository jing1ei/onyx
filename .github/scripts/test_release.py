"""Offline regression tests for complete, repeatable release publication."""
import hashlib
import importlib.util
from pathlib import Path
import shutil
import tempfile
import sys
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True

spec = importlib.util.spec_from_file_location('release', Path(__file__).with_name('release.py'))
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.config = {'project': 'Fixture', 'notes': 'Fixture release', 'assets': {
            f'Fixture-{release.VERSION}-macOS-universal.zip': 'mac.zip',
            f'Fixture-{release.VERSION}-Windows-x64.zip': 'win.zip',
            f'Fixture-{release.VERSION}-source.zip': 'source.zip'}}
        self.source = self.root / 'input'
        self.source.mkdir()
        for name in self.config['assets'].values():
            (self.source / name).write_bytes(name.encode())
        self.calls = []
        self.existing = False      # a release (and tag) for this version exists
        self.published = False     # ...and it is already public
        self.tag_sha = 'built'
        self.fail_upload = False
        self.corrupt = False
        self.heads = ['built', 'built']
        self.untagged = False
        self.reuse_uploads = False
        self.asset_ids = {
            name: i + 100 for i, name in enumerate(
                sorted(list(self.config['assets']) + ['SHA256SUMS.txt']))
        }

    def ready(self):
        return release.prepare(self.source, self.root / 'ready', self.config)

    def api(self, endpoint, method='GET', payload=None):
        self.calls.append((method, endpoint, payload))
        if endpoint.endswith('/git/ref/heads/main'):
            return {'object': {'sha': self.heads.pop(0)}}
        if '/matching-refs/' in endpoint:
            return [{'ref': 'refs/tags/' + release.TAG, 'object': {'sha': self.tag_sha}}] if self.existing and not self.untagged else []
        if '/releases' in endpoint:
            return {'id': 7, 'draft': True}
        return {}

    def pages(self, endpoint):
        if '/assets?' in endpoint:
            return [{'name': 'old-name.zip', 'id': 42}] + [
                {'name': name, 'id': asset_id,
                 'digest': 'sha256:' + release.digest(self.root / 'ready' / name) if self.reuse_uploads else None}
                for name, asset_id in self.asset_ids.items()
            ]
        return [{
            'id': 7, 'tag_name': 'untagged-fixture' if self.untagged else release.TAG,
            'draft': not self.published, 'name': release.VERSION, 'target_commitish': 'built'
        }] if self.existing else []

    def gh(self, *args, **kwargs):
        self.calls.append(('gh', args, None))
        if args[:2] == ('release', 'upload') and self.fail_upload:
            raise RuntimeError('Upload failed')
        return ''

    def gh_bytes(self, endpoint):
        self.calls.append(('download', endpoint, None))
        asset_id = int(endpoint.rsplit('/', 1)[1])
        name = next(name for name, value in self.asset_ids.items() if value == asset_id)
        if self.corrupt and name.endswith('Windows-x64.zip'):
            return b'corrupt'
        return (self.root / 'ready' / name).read_bytes()

    def publish(self):
        directory = self.ready()
        with patch.object(release, 'api', self.api), patch.object(release, 'pages', self.pages), patch.object(release, 'gh', self.gh), patch.object(release, 'gh_bytes', self.gh_bytes):
            release.publish(directory, self.config, 'built', 'owner/repo')

    def assert_unpublished(self):
        self.assertFalse(any(method == 'PATCH' and payload and payload.get('draft') is False
                             for method, _, payload in self.calls))
        self.assertFalse(any(method in ('POST', 'PATCH') and '/git/refs' in endpoint
                             for method, endpoint, _ in self.calls))

    def test_normalized_names_and_checksums(self):
        directory = self.ready()
        self.assertEqual({p.name for p in directory.iterdir()}, set(self.config['assets']) | {'SHA256SUMS.txt'})
        for line in (directory / 'SHA256SUMS.txt').read_text().splitlines():
            digest, name = line.split('  ')
            self.assertEqual(digest, hashlib.sha256((directory / name).read_bytes()).hexdigest())

    def test_missing_platform_does_not_prepare_partial_release(self):
        (self.source / 'win.zip').unlink()
        with self.assertRaises(ValueError): self.ready()
        self.assertFalse((self.root / 'ready').exists())

    def test_empty_package_is_rejected(self):
        (self.source / 'win.zip').write_bytes(b'')
        with self.assertRaises(ValueError): self.ready()

    def test_ambiguous_artifact_is_rejected(self):
        (self.source / 'duplicate').mkdir()
        shutil.copyfile(self.source / 'win.zip', self.source / 'duplicate/win.zip')
        with self.assertRaises(ValueError): self.ready()

    def test_create_complete_public_release(self):
        self.publish()
        self.assertEqual(self.calls[-1][0], 'PATCH')
        self.assertEqual(self.calls[-1][2]['name'], release.VERSION)
        self.assertFalse(self.calls[-1][2]['draft'])
        upload = next(i for i,c in enumerate(self.calls) if c[0]=='gh' and c[1][:2]==('release','upload'))
        download = next(i for i,c in enumerate(self.calls) if c[0]=='download')
        tag = next(i for i,c in enumerate(self.calls) if c[0]=='POST' and c[1].endswith('/git/refs'))
        self.assertLess(upload, download)
        self.assertLess(download, tag)

    def test_draft_release_is_resumed_without_moving_its_tag(self):
        self.existing = True
        self.publish()
        self.assertFalse(any(m=='POST' and e.endswith('/releases') for m,e,_ in self.calls))
        self.assertFalse(any('/git/refs' in e and m in ('POST', 'PATCH') for m,e,_ in self.calls))
        self.assertTrue(any(m=='DELETE' and e.endswith('/assets/42') for m,e,_ in self.calls))
        first = next(p for m,e,p in self.calls if m=='PATCH' and '/releases/' in e)
        self.assertTrue(first['draft'])
        self.assertFalse(self.calls[-1][2]['draft'])

    def test_untagged_draft_resumes_and_verifies_assets_by_id(self):
        self.existing = self.untagged = True
        self.publish()
        self.assertFalse(any(m == 'POST' and e.endswith('/releases') for m, e, _ in self.calls))
        update = next(p for m, e, p in self.calls if m == 'PATCH' and '/releases/' in e)
        self.assertEqual(update['tag_name'], release.TAG)
        self.assertTrue(any(m == 'download' for m, _, _ in self.calls))
        self.assertFalse(any(m == 'gh' and e[:2] == ('release', 'download') for m, e, _ in self.calls))
        self.assertFalse(self.calls[-1][2]['draft'])

    def test_retry_reuses_matching_uploads_and_still_checks_bytes(self):
        self.existing = self.untagged = self.reuse_uploads = True
        self.publish()
        self.assertFalse(any(m == 'gh' and e[:2] == ('release', 'upload') for m, e, _ in self.calls))
        self.assertEqual(sum(m == 'download' for m, _, _ in self.calls), len(self.asset_ids))
        self.assertFalse(self.calls[-1][2]['draft'])

    def test_published_release_is_never_modified(self):
        self.existing = self.published = True
        self.publish()
        self.assertFalse(any(m in ('POST', 'PATCH', 'DELETE') or m == 'gh' for m,_,_ in self.calls))

    def test_tag_on_another_commit_is_refused(self):
        self.existing = True
        self.tag_sha = 'older'
        with self.assertRaisesRegex(ValueError, 'another commit'): self.publish()
        self.assert_unpublished()

    def test_failed_upload_never_moves_tag_or_publishes(self):
        self.fail_upload = True
        with self.assertRaises(RuntimeError): self.publish()
        self.assert_unpublished()

    def test_corrupt_upload_never_moves_tag_or_publishes(self):
        self.corrupt = True
        with self.assertRaisesRegex(ValueError, 'Uploaded checksum'): self.publish()
        self.assert_unpublished()

    def test_stale_build_never_mutates_release(self):
        self.heads = ['newer']
        with self.assertRaisesRegex(ValueError, 'newer main'): self.publish()
        self.assertEqual(len(self.calls), 1)

    def test_main_change_during_upload_never_moves_tag(self):
        self.heads = ['built', 'newer']
        with self.assertRaisesRegex(ValueError, 'newer main'): self.publish()
        self.assert_unpublished()

    def test_api_failure_does_not_create_release(self):
        directory = self.ready()
        with patch.object(release, 'api', side_effect=RuntimeError('network')), patch.object(release, 'gh') as gh:
            with self.assertRaises(RuntimeError):
                release.publish(directory, self.config, 'built', 'owner/repo')
            gh.assert_not_called()

    def test_local_corruption_never_contacts_github(self):
        directory = self.ready()
        (directory / f'Fixture-{release.VERSION}-Windows-x64.zip').write_bytes(b'bad')
        with patch.object(release, 'api') as api:
            with self.assertRaises(ValueError):
                release.publish(directory, self.config, 'built', 'owner/repo')
            api.assert_not_called()

    def test_repository_contract_has_fixed_version_and_platform(self):
        config = release.load_config()
        for name in config['assets']:
            self.assertTrue(name.startswith(f"{config['project']}-{release.VERSION}-"), name)
            self.assertRegex(name, r'-(macOS-(universal|arm64|x64)|Windows-(arm64|x64|x86)|source)\.')


if __name__ == '__main__':
    unittest.main()
