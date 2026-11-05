#!/usr/bin/env python3
"""Reject tracked scratch output, including force-added ignored files."""
from pathlib import Path
import os
import subprocess
import sys


def misplaced_document(path):
    """Keep product Markdown centralized while preserving owner-local records."""
    if not path.endswith('.md'):
        return False
    name = Path(path).name
    return not (
        path in {'README.md', 'AGENTS.md', 'CLAUDE.md', 'RELEASE_NOTES.md',
                 'assets/fonts/README.md'}
        or path.startswith(('product/', 'vendor/'))
        or (path.startswith(('.agents/skills/', '.claude/skills/')) and name == 'SKILL.md')
        or ('/tests/fixtures/' in '/' + path and name == 'README.md')
        or name.startswith(('LICENSE', 'NOTICE', 'COPYING'))
    )


def main():
    root = Path(subprocess.check_output(
        ['git', 'rev-parse', '--show-toplevel'], text=True).strip())
    paths = subprocess.check_output(
        ['git', 'ls-files', '--cached', '--others', '--exclude-standard', '-z'], cwd=root)
    # Ignore unstaged deletions so the check also works during local cleanup.
    paths = sorted({p for p in paths.split(b'\0') if p and
                    os.path.lexists(root / os.fsdecode(p))})
    if not paths:
        return 0
    misplaced = [os.fsdecode(path) for path in paths if misplaced_document(os.fsdecode(path))]
    # --no-index is essential: normally Git does not check tracked files.
    result = subprocess.run(
        ['git', 'check-ignore', '--no-index', '--stdin', '-z'], cwd=root,
        input=b'\0'.join(paths) + b'\0', capture_output=True)
    if result.returncode not in (0, 1):
        sys.stderr.buffer.write(result.stderr)
        return result.returncode
    forbidden = result.stdout.split(b'\0')
    if any(forbidden) or misplaced:
        if any(forbidden):
            print('Ignored artifacts must not be tracked. Remove these paths from Git:', file=sys.stderr)
            for path in forbidden:
                if path:
                    print('  ' + path.decode('utf-8', errors='replace'), file=sys.stderr)
        if misplaced:
            print('Product guidance belongs under product/. Misplaced Markdown:', file=sys.stderr)
            for path in misplaced:
                print('  ' + path, file=sys.stderr)
        return 1
    print('Repository artifact policy passed.')
    return 0


if __name__ == '__main__':
    sys.exit(main())
