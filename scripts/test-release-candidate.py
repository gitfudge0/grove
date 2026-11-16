#!/usr/bin/env python3
"""Release promotion must reject incomplete or mismatched prepared installers."""
import importlib.util
import json
import os
import subprocess
from pathlib import Path
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location('release_candidate', Path(__file__).with_name('release-candidate.py'))
candidate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(candidate)
SHA = 'a' * 40
VERSION = '1.2.1'


class CandidateTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        for platform, suffix in candidate.PLATFORMS.items():
            folder = self.root / ('candidate-' + platform)
            folder.mkdir()
            (folder / f'Grove-{VERSION}-{suffix}').write_bytes(b'installer fixture')
            candidate.record(folder, platform, SHA, VERSION)
        self.folder = self.root / 'candidate-linux-amd64'
        self.manifest_path = self.folder / 'candidate.json'
        self.installer = self.folder / f'Grove-{VERSION}-linux-amd64.deb'

    def test_complete_candidate(self):
        self.assertEqual(len(candidate.verify(self.root, SHA, VERSION)), 4)

    def test_wrong_commit_version_or_platform(self):
        original = json.loads(self.manifest_path.read_text())
        for field, value in [('sha', 'b' * 40), ('version', '1.2.0'),
                             ('platform', 'macos-arm64'), ('schema_version', 2),
                             ('file', '../another-installer')]:
            with self.subTest(field=field):
                self.manifest_path.write_text(json.dumps(dict(original, **{field: value})))
                with self.assertRaises(ValueError):
                    candidate.verify(self.root, SHA, VERSION)
        self.manifest_path.write_text(json.dumps(original))

    def test_modified_installer(self):
        self.installer.write_bytes(b'changed installer')
        with self.assertRaisesRegex(ValueError, 'checksum'):
            candidate.verify(self.root, SHA, VERSION)

    def test_missing_platform(self):
        self.manifest_path.unlink()
        self.installer.unlink()
        self.folder.rmdir()
        with self.assertRaisesRegex(ValueError, 'four platform'):
            candidate.verify(self.root, SHA, VERSION)

    def test_extra_file(self):
        (self.folder / 'stale.deb').write_bytes(b'stale')
        with self.assertRaisesRegex(ValueError, 'contents'):
            candidate.verify(self.root, SHA, VERSION)

    def test_record_rejects_old_or_missing_installer(self):
        self.manifest_path.unlink()
        self.installer.rename(self.folder / 'Grove-1.2.0-linux-amd64.deb')
        with self.assertRaises(ValueError):
            candidate.record(self.folder, 'linux-amd64', SHA, VERSION)

    def test_download_cli_checks_complete_candidate_before_returning_paths(self):
        # Exercise the real subprocess/download orchestration with a local gh stub.
        # This proves helper wiring, not the GitHub API or artifact service.
        sha = candidate.command('git', 'rev-parse', 'HEAD')
        current_version = candidate.version()
        for platform, suffix in candidate.PLATFORMS.items():
            folder = self.root / ('candidate-' + platform)
            (folder / 'candidate.json').unlink()
            old = folder / f'Grove-{VERSION}-{suffix}'
            new = folder / f'Grove-{current_version}-{suffix}'
            if old != new:
                old.rename(new)
            candidate.record(folder, platform, sha, current_version)
        with tempfile.TemporaryDirectory() as scratch:
            scratch = Path(scratch)
            stub = scratch / 'gh'
            stub.write_text('#!' + sys.executable + '\n' + '''
import json, os, shutil, sys
from pathlib import Path
if sys.argv[1] == 'api':
    print(json.dumps(dict(path='.github/workflows/release.yml',
        head_repository=dict(full_name='owner/grove'), head_branch='main',
        event='push', status='completed', conclusion='success',
        head_sha=os.environ['TEST_CANDIDATE_SHA'])))
elif sys.argv[1:3] == ['run', 'download']:
    destination = Path(sys.argv[sys.argv.index('--dir') + 1])
    for folder in Path(os.environ['TEST_CANDIDATE_SOURCE']).iterdir():
        shutil.copytree(folder, destination / folder.name)
else:
    sys.exit('Unexpected gh command')
''')
            stub.chmod(0o755)
            env = dict(os.environ, PATH=str(scratch) + os.pathsep + os.environ['PATH'],
                       GITHUB_REPOSITORY='owner/grove', TEST_CANDIDATE_SHA=sha,
                       TEST_CANDIDATE_SOURCE=str(self.root))
            script = Path(__file__).with_name('release-candidate.py').resolve()
            def download(destination):
                return subprocess.run([sys.executable, str(script), 'download', '123',
                    'v' + current_version, str(destination)], env=env, text=True,
                    capture_output=True, check=False)
            result = download(scratch / 'downloaded')
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(len(result.stdout.splitlines()), 4)
            for file in result.stdout.splitlines():
                self.assertTrue(Path(file).is_file())
            broken = self.root / 'candidate-linux-amd64' / f'Grove-{current_version}-linux-amd64.deb'
            broken.write_bytes(b'tampered')
            rejected = download(scratch / 'rejected')
            self.assertNotEqual(rejected.returncode, 0)
            self.assertEqual(rejected.stdout, '')

    def test_tag_must_match_version(self):
        candidate.check_tag('v1.2.1', VERSION)
        for tag in ['v1.2.0', 'main', 'v1.2.1-rc', '--help']:
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                candidate.check_tag(tag, VERSION)

    def test_only_successful_same_repository_main_build_is_trusted(self):
        run = dict(path='.github/workflows/release.yml',
                   head_repository=dict(full_name='owner/grove'), head_branch='main',
                   event='push', status='completed', conclusion='success', head_sha=SHA)
        candidate.validate_run(run, 'owner/grove', SHA)
        candidate.validate_run(dict(run, event='workflow_dispatch'), 'owner/grove', SHA)
        for field, value in [('path', '.github/workflows/ci.yml'),
                             ('head_repository', dict(full_name='fork/grove')),
                             ('head_branch', 'feature'), ('event', 'pull_request'),
                             ('status', 'in_progress'), ('conclusion', 'cancelled'),
                             ('head_sha', 'b' * 40)]:
            with self.subTest(field=field), self.assertRaises(ValueError):
                candidate.validate_run(dict(run, **{field: value}), 'owner/grove', SHA)


if __name__ == '__main__':
    unittest.main()
