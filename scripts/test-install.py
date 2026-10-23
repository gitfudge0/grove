#!/usr/bin/env python3
"""Exercise installer command boundaries without building or changing an app."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / 'install.sh'


class InstallerTests(unittest.TestCase):
    def run_installer(self, *, local=True, host='aarch64-apple-darwin',
                      system='Darwin', arch='arm64', args=(), download_fails=False, build_target=''):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            commands = root / 'bin'
            commands.mkdir()
            log = root / 'commands'
            script = root / 'install.sh'
            script.write_text(SCRIPT.read_text())
            if local:
                (root / 'Cargo.toml').touch()
            stubs = {
                'uname': f'if [ "$1" = -s ]; then echo {system}; else echo {arch}; fi\n',
                'sysctl': 'echo 1\n',
                'rustc': f'echo "host: {host}"\n',
                'rustup': 'exit 0\n',
                'cargo': 'if [ "$2" = --version ]; then echo "cargo-bundle v0.12.0"; exit 0; fi\nexit 77\n',
                'curl': ('exit 22\n' if download_fails else
                         'case "$*" in *url_effective*) echo https://github.com/gitfudge0/grove/releases/tag/v1.2.3;; esac\n'),
                'dpkg': 'exit 0\n',
                'sudo': 'exit 0\n',
                'git': 'exit 78\n',
                'cp': 'case "$3" in */osx/) mkdir -p "$3/Grove.app";; esac\n',
                'rm': 'exit 0\n',
                'mkdir': 'case "$*" in *Applications*) exit 0;; *) exec /bin/mkdir "$@";; esac\n',
                'hdiutil': 'exit 0\n',
                'xattr': 'exit 0\n',
                'security': 'echo gitfudge\n',
                'codesign': 'exit 0\n',
            }
            for name, body in stubs.items():
                path = commands / name
                path.write_text('#!/bin/bash\nprintf "%s" "' + name +
                                '" >> "$TEST_LOG"\nprintf " <%s>" "$@" >> "$TEST_LOG"\nprintf "\\n" >> "$TEST_LOG"\n' + body)
                path.chmod(0o755)
            env = dict(os.environ, PATH=f'{commands}:/usr/bin:/bin',
                       TEST_LOG=str(log), HOME=str(root), CARGO_TARGET_DIR='artifacts',
                       CARGO_BUILD_TARGET=build_target)
            result = subprocess.run(['/bin/bash', str(script), *args], env=env,
                                    text=True, capture_output=True)
            return result, log.read_text() if log.exists() else ''

    def test_native_local_profile_uses_host_artifacts(self):
        result, log = self.run_installer()
        self.assertEqual(result.returncode, 77)
        self.assertIn('<--profile> <local>', log)
        self.assertNotIn('<--target>', log)
        self.assertNotIn('rustup', log)

    def test_rosetta_keeps_native_target(self):
        result, log = self.run_installer(host='x86_64-apple-darwin', arch='x86_64')
        self.assertEqual(result.returncode, 77)
        self.assertIn('rustup <target> <add> <aarch64-apple-darwin>', log)
        self.assertIn('<--target> <aarch64-apple-darwin>', log)

    def test_build_target_cannot_override_native_mac_architecture(self):
        result, log = self.run_installer(build_target='x86_64-apple-darwin')
        self.assertEqual(result.returncode, 77)
        self.assertIn('<--target> <aarch64-apple-darwin>', log)

    def test_linux_local_profile(self):
        result, log = self.run_installer(system='Linux', arch='x86_64')
        self.assertEqual(result.returncode, 77)
        self.assertIn('<--profile> <local> <--format> <deb>', log)
        self.assertNotIn('<--target>', log)

    def test_release_profile_is_explicit(self):
        result, log = self.run_installer(args=['--release'])
        self.assertEqual(result.returncode, 77)
        self.assertIn('<--profile> <release>', log)

    def test_remote_linux_uses_published_package_without_toolchain(self):
        result, log = self.run_installer(local=False, system='Linux', arch='x86_64')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('/download/v1.2.3/Grove-1.2.3-linux-amd64.deb', log)
        self.assertIn('sudo <dpkg> <-i>', log)
        self.assertNotIn('cargo', log)
        self.assertNotIn('git ', log)

    def test_remote_mac_download_mount_and_sign(self):
        result, log = self.run_installer(local=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('/download/v1.2.3/Grove-1.2.3-macos-arm64.dmg', log)
        self.assertIn('hdiutil <attach> <-readonly> <-nobrowse>', log)
        self.assertIn('hdiutil <detach>', log)
        self.assertIn('codesign <--force> <--deep> <--sign> <gitfudge>', log)
        self.assertNotIn('cargo', log)

    def test_download_failure_never_compiles(self):
        result, log = self.run_installer(local=False, download_fails=True)
        self.assertEqual(result.returncode, 22)
        self.assertNotIn('cargo', log)
        self.assertNotIn('git ', log)
        self.assertNotIn('cp ', log)
        self.assertNotIn('/Applications/Grove.app', log)
        self.assertNotIn('codesign ', log)
        self.assertNotIn('sudo ', log)

    def test_source_is_explicit(self):
        result, log = self.run_installer(local=False, args=['--source'])
        self.assertEqual(result.returncode, 78)
        self.assertIn('git <clone>', log)
        self.assertNotIn('curl', log)

    def test_unsupported_asset_stops(self):
        result, log = self.run_installer(local=False, system='Linux', arch='aarch64')
        self.assertEqual(result.returncode, 1)
        self.assertIn('no prebuilt release', result.stderr)
        self.assertNotIn('sudo', log)


if __name__ == '__main__':
    unittest.main()
