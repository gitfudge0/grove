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
                      system='Darwin', arch='arm64', args=(), download_fails=False, build_target='',
                      build_succeeds=False, install_fails=False, fallback=False,
                      clean_fails=False, missing_webkit=False, dependency_install_fails=False):
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
                'cargo': ('if [ "$2" = --version ]; then echo "cargo-bundle v0.12.0"; exit 0; fi\n'
                          'if [ "$1" = clean ]; then exit ' + ('79' if clean_fails else '0') + '; fi\n' +
                          ('exit 77\n' if not build_succeeds else
                           'profile=local; target=""\n'
                           'while [ "$#" -gt 0 ]; do case "$1" in '
                           '--profile) shift; profile="$1";; --target) shift; target="$1";; esac; shift; done\n'
                           'dir="$CARGO_TARGET_DIR${target:+/$target}/$profile"\n'
                           '/bin/mkdir -p "$dir/bundle/osx/Grove.app/Contents/MacOS" "$dir/bundle/deb"\n'
                           'touch "$dir/grove" "$dir/bundle/osx/Grove.app/Contents/MacOS/grove"\n' +
                           ('' if fallback else 'touch "$dir/bundle/deb/grove.deb"\n'))),
                'curl': ('exit 22\n' if download_fails else
                         'case "$*" in *url_effective*) echo https://github.com/gitfudge0/grove/releases/tag/v1.2.3;; esac\n'),
                'dpkg': 'exit 0\n',
                'sudo': ('case "$1" in pacman) ' +
                         ('exit 75' if dependency_install_fails else 'touch "$TEST_WEBKIT"; exit 0') +
                         ';; esac\nexit ' + ('76' if install_fails else '0') + '\n'),
                'pkg-config': ('[ -e "$TEST_WEBKIT" ]\n' if missing_webkit else 'exit 0\n'),
                'pacman': 'exit 0\n',
                'git': 'exit 78\n',
                'cp': ('exit 76\n' if install_fails else 'case "$3" in */osx/) mkdir -p "$3/Grove.app";; esac\n'),
                'rm': 'exit 0\n',
                'file': 'echo "Mach-O 64-bit executable arm64"\n',
                'install': 'exit 0\n',
                'update-desktop-database': 'exit 0\n',
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
                       TEST_LOG=str(log), TEST_WEBKIT=str(root / 'webkit'), HOME=str(root), CARGO_TARGET_DIR='artifacts',
                       CARGO_BUILD_TARGET=build_target)
            result = subprocess.run(['/bin/bash', str(script), *args], env=env,
                                    text=True, capture_output=True)
            return result, log.read_text() if log.exists() else ''

    def test_success_cleans_only_selected_profile_after_install(self):
        cases = [
            ({}, 'local', '', 'cp <-R>'),
            ({'system': 'Linux', 'arch': 'x86_64'}, 'local', '', 'sudo <dpkg> <-i>'),
            ({'args': ['--release']}, 'release', '', 'cp <-R>'),
            ({'system': 'Linux', 'arch': 'x86_64', 'build_target': 'x86_64-unknown-linux-gnu'},
             'local', 'x86_64-unknown-linux-gnu', 'sudo <dpkg> <-i>'),
            ({'host': 'x86_64-apple-darwin'}, 'local', 'aarch64-apple-darwin', 'codesign '),
            ({'system': 'Linux', 'arch': 'x86_64', 'fallback': True}, 'local', '', 'install <-Dm644>'),
        ]
        for options, profile, target, installation in cases:
            with self.subTest(options=options):
                result, log = self.run_installer(build_succeeds=True, **options)
                self.assertEqual(result.returncode, 0, result.stderr)
                clean = f'cargo <clean> <--profile> <{profile}>'
                if target:
                    clean += f' <--target> <{target}>'
                self.assertEqual([line for line in log.splitlines() if line.startswith('cargo <clean>')], [clean])
                self.assertLess(log.index(installation), log.index(clean))

    def test_install_failure_retains_build_artifacts(self):
        for system, arch in [('Darwin', 'arm64'), ('Linux', 'x86_64')]:
            with self.subTest(system=system):
                result, log = self.run_installer(system=system, arch=arch,
                                                 build_succeeds=True, install_fails=True)
                self.assertEqual(result.returncode, 76, result.stderr)
                self.assertNotIn('cargo <clean>', log)

    def test_build_failure_retains_build_artifacts(self):
        result, log = self.run_installer()
        self.assertEqual(result.returncode, 77)
        self.assertNotIn('cargo <clean>', log)

    def test_cleanup_failure_is_reported(self):
        result, log = self.run_installer(build_succeeds=True, clean_fails=True)
        self.assertEqual(result.returncode, 79)
        self.assertIn('codesign ', log)
        self.assertIn('cargo <clean> <--profile> <local>', log)

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

    def test_linux_installs_missing_webkit_before_build(self):
        result, log = self.run_installer(system='Linux', arch='x86_64', missing_webkit=True)
        self.assertEqual(result.returncode, 77, result.stderr)
        self.assertLess(log.index('sudo <pacman> <-S> <--needed> <webkit2gtk-4.1>'),
                        log.index('cargo <bundle>'))

    def test_dependency_install_failure_stops_build(self):
        result, log = self.run_installer(system='Linux', arch='x86_64', missing_webkit=True,
                                         dependency_install_fails=True)
        self.assertEqual(result.returncode, 75)
        self.assertNotIn('cargo <bundle>', log)

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
