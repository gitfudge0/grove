#!/usr/bin/env python3
"""Check real Cargo metadata after moving a checkout with its build cache."""
from pathlib import Path
import os
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class IconBuildPathsTests(unittest.TestCase):
    def test_moved_checkout_refreshes_icon_metadata(self):
        with tempfile.TemporaryDirectory() as temp:
            original = Path(temp) / 'original'
            for name in ('assets', 'ui'):
                crate = original / name
                (crate / 'src').mkdir(parents=True)
                shutil.copy2(ROOT / f'vendor/gpui-component/{name}/build.rs', crate / 'build.rs')
            (original / 'Cargo.toml').write_text('[workspace]\nmembers = ["assets", "ui"]\nresolver = "2"\n')
            (original / 'assets/Cargo.toml').write_text(
                '[package]\nname = "icon-assets-fixture"\nversion = "0.1.0"\n'
                'edition = "2021"\nlinks = "gpui-component-default-icons"\n')
            (original / 'assets/src/lib.rs').touch()
            (original / 'assets/assets/icons').mkdir(parents=True)
            (original / 'assets/assets/icons/test.svg').write_text('<svg/>')
            (original / 'ui/Cargo.toml').write_text(
                '[package]\nname = "icon-ui-fixture"\nversion = "0.1.0"\nedition = "2021"\n'
                '[dependencies]\nicon-assets-fixture = { path = "../assets" }\n')
            (original / 'ui/src/main.rs').write_text(
                'fn main() { println!("{}", env!("GPUI_COMPONENT_DEFAULT_ICONS_DIR")); }\n')
            env = dict(os.environ)
            env.pop('CARGO_TARGET_DIR', None)
            env.pop('CARGO_BUILD_TARGET', None)

            def check(checkout):
                result = subprocess.run(
                    ['cargo', 'run', '--offline', '--quiet', '-p', 'icon-ui-fixture'],
                    cwd=checkout, env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout.strip(), str(checkout / 'assets/assets/icons'))

            check(original)
            moved = Path(temp) / 'moved'
            # Preserve source mtimes and cached Cargo fingerprints, as happens
            # with a moved checkout or a restored build cache.
            shutil.copytree(original, moved)
            shutil.rmtree(original)
            check(moved)


if __name__ == '__main__':
    unittest.main()
