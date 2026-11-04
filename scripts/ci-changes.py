#!/usr/bin/env python3
"""Skip native jobs only for known documentation-only PRs; fail open otherwise."""
import os
from pathlib import Path
import subprocess


def needs_build(paths):
    # Unknown files, scripts, assets and workflow changes always run native jobs.
    # Deletions/renames are included by git diff (both sides of a rename).
    return not paths or any(
        not (path in {'README.md', 'AGENTS.md', 'DESIGN.md', 'CONTRIBUTING.md'}
             or path.startswith(('docs/', 'plans/')))
        for path in paths
    )


def detect_build(env):
    if env.get('GITHUB_EVENT_NAME') != 'pull_request':
        return True
    base, head = env.get('BASE_SHA', ''), env.get('HEAD_SHA', '')
    if not base or not head:
        return True
    try:
        # Checkout contains a synthetic merge, so explicitly fetch both PR ends.
        for revision in (base, head):
            subprocess.run(['git', 'fetch', '--no-tags', '--depth=1', 'origin', revision],
                           check=True, capture_output=True, timeout=60)
        result = subprocess.run(['git', 'diff', '--no-renames', '--name-only', '-z', base, head],
                                check=True, capture_output=True, timeout=30)
        paths = result.stdout.decode('utf-8').rstrip('\0').split('\0')
        return needs_build(paths)
    except (subprocess.SubprocessError, UnicodeError, OSError):
        return True


if __name__ == '__main__':
    build = str(detect_build(os.environ)).lower()
    with Path(os.environ['GITHUB_OUTPUT']).open('a') as output:
        output.write(f'build={build}\n')
    print(f'Native build required: {build}')
