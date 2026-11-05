#!/usr/bin/env python3
"""Conservative CI gating must never skip unknown or unresolved inputs."""
import importlib.util
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True

spec = importlib.util.spec_from_file_location('ci_changes', Path(__file__).with_name('ci-changes.py'))
changes = importlib.util.module_from_spec(spec)
spec.loader.exec_module(changes)


class ChangeTests(unittest.TestCase):
    def test_documentation_only(self):
        self.assertFalse(changes.needs_build(['README.md', 'AGENTS.md', 'CLAUDE.md']))

    def test_unknown_and_native_inputs(self):
        for path in ['src/main.rs', 'crates/grove-core/src/lib.rs', 'Cargo.lock',
                     'rust-toolchain.toml', '.cargo/config.toml', 'build.rs',
                     '.github/workflows/ci.yml', 'scripts/ci-changes.py',
                     'product/design.md', 'product/features.md', 'assets/fonts/font.ttf', 'assets/release-highlights.md', 'vendor/ui.rs', 'new-file', 'docs.rs']:
            with self.subTest(path=path):
                self.assertTrue(changes.needs_build(['README.md', path]))
        self.assertTrue(changes.needs_build([]))

    def test_main_and_missing_revisions_fail_open(self):
        self.assertTrue(changes.detect_build({'GITHUB_EVENT_NAME': 'push'}))
        self.assertTrue(changes.detect_build({'GITHUB_EVENT_NAME': 'pull_request'}))

    def test_failed_diff_fails_open(self):
        with patch.object(changes.subprocess, 'run', side_effect=subprocess.CalledProcessError(1, 'git')):
            self.assertTrue(changes.detect_build(dict(GITHUB_EVENT_NAME='pull_request', BASE_SHA='base', HEAD_SHA='head')))

    def test_deleted_native_file_runs_checks(self):
        result = subprocess.CompletedProcess([], 0, stdout=b'README.md\0src/deleted.rs\0')
        with patch.object(changes.subprocess, 'run', return_value=result) as run:
            self.assertTrue(changes.detect_build(dict(GITHUB_EVENT_NAME='pull_request', BASE_SHA='base', HEAD_SHA='head')))
            self.assertIn('--no-renames', run.call_args.args[0])


if __name__ == '__main__':
    unittest.main()
