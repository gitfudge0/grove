#!/usr/bin/env python3
"""Exercise artifact rejection with a real Git index in an isolated repository."""
from pathlib import Path
import subprocess
import re
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent
CHECK = ROOT / 'scripts/check-repository-artifacts.py'


class ArtifactTests(unittest.TestCase):
    def test_product_document_links(self):
        guides = list((ROOT / 'product').glob('*.md'))
        guides += [ROOT / name for name in ['README.md', 'AGENTS.md', 'CLAUDE.md']]
        for directory in ['.agents/skills', '.claude/skills']:
            guides += list((ROOT / directory).rglob('SKILL.md'))
        for guide in guides:
            for link in re.findall(r'\]\(([^)]+)\)', guide.read_text()):
                if '://' in link or link.startswith(('mailto:', '#')):
                    continue
                target = link.split('#', 1)[0].strip('<>')
                with self.subTest(guide=guide.relative_to(ROOT), link=link):
                    self.assertTrue((guide.parent / target).exists(), 'missing local link target')

    def test_document_ownership(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            subprocess.run(['git', 'init'], cwd=root, check=True, capture_output=True)

            def check():
                return subprocess.run(['python3', str(CHECK)], cwd=root, capture_output=True)

            def write(path):
                p = root / path
                p.parent.mkdir(parents=True, exist_ok=True)
                p.write_text('fixture')

            for path in ['README.md', 'AGENTS.md', 'CLAUDE.md', 'RELEASE_NOTES.md',
                         'product/README.md', 'product/design.md', 'product/verification.md',
                         'vendor/library/README.md', 'assets/fonts/README.md',
                         'crates/core/tests/fixtures/README.md', 'assets/font/LICENSE.md',
                         '.agents/skills/grove-ui/SKILL.md',
                         '.claude/skills/gpui-development/SKILL.md']:
                write(path)
            subprocess.run(['git', 'add', '.'], cwd=root, check=True, capture_output=True)
            self.assertEqual(check().returncode, 0)
            for path in ['NOTES.md', 'src/product-notes.md', '.grove/qa/review.md',
                         '.agents/skills/grove-ui/old-design.md',
                         '.claude/skills/gpui-development/patterns.md']:
                with self.subTest(path=path):
                    write(path)
                    self.assertEqual(check().returncode, 1)
                    subprocess.run(['git', 'add', path], cwd=root, check=True, capture_output=True)
                    result = check()
                    self.assertEqual(result.returncode, 1)
                    self.assertIn(path.encode(), result.stderr)
                    (root / path).unlink()
                    self.assertEqual(check().returncode, 0)
                    subprocess.run(['git', 'rm', '--cached', path], cwd=root,
                                   check=True, capture_output=True)

    def test_real_index_and_working_tree(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)

            def git(*args):
                return subprocess.run(['git', *args], cwd=root, check=True, capture_output=True)

            def check():
                return subprocess.run(['python3', str(CHECK)], cwd=root, capture_output=True)

            def write(path):
                p = root / path
                p.parent.mkdir(parents=True, exist_ok=True)
                p.write_text('fixture')

            git('init')
            (root / '.gitignore').write_text((ROOT / '.gitignore').read_text())
            for path in ['ui-references/fixtures/pilot.json', 'wix/main.wxs',
                         'tests/fixtures/native-plans/example.html', 'assets/image.png',
                         '.grove/change-categories.json']:
                write(path)
                git('add', path)
            self.assertEqual(check().returncode, 0)
            for path in ['.grove/plans/example.json', '.grove/qa/oversized-live-path.json',
                         '.grove/qa/oversized_artifact_smoke.rs',
                         'todo.md', 'assets/showcase/grove-launch.jpg',
                         'assets/showcase/grove-launch.mp4', 'docs/guide.md', 'mockups/mock.html', 'output/chart.html',
                         'plans/example.json', '.grove-artifacts/plan.json',
                         '.playwright-cli/page.yml', '.fudge/mock.html',
                         '.grove/mock/activity.html', '.grove/mocks/task.png',
                         '.grove/plan-samples/sample.html',
                         '.grove/plan-visual-sample.grove-plan.json', 'DESIGN.html',
                         'sidebar-variations.html', 'transcript-performance-new.png']:
                with self.subTest(path=path):
                    write(path)
                    self.assertEqual(check().returncode, 0, 'untracked ignored scratch is allowed')
                    git('add', '-f', path)
                    result = check()
                    self.assertEqual(result.returncode, 1)
                    self.assertIn(path.encode(), result.stderr)
                    (root / path).unlink()
                    self.assertEqual(check().returncode, 0, 'unstaged deletion is allowed')
                    git('rm', '--cached', path)


if __name__ == '__main__':
    unittest.main()
