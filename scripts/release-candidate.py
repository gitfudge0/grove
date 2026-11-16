#!/usr/bin/env python3
"""Bind release installers to an exact source revision; never compile or publish."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tomllib

PLATFORMS = {
    'macos-arm64': 'macos-arm64.dmg',
    'macos-x86_64': 'macos-x86_64.dmg',
    'linux-amd64': 'linux-amd64.deb',
    'windows-x86_64': 'windows-x86_64-alpha.msi',
}


def command(*args):
    return subprocess.check_output(args, text=True).strip()


def version():
    with open('Cargo.toml', 'rb') as file:
        return tomllib.load(file)['workspace']['package']['version']


def check_tag(tag, expected_version):
    if not re.fullmatch(r'v\d+\.\d+\.\d+', tag) or tag != 'v' + expected_version:
        raise ValueError('Release tag must match the workspace version')


def digest(path):
    with path.open('rb') as file:
        return hashlib.file_digest(file, 'sha256').hexdigest()


def record(directory, platform, sha, candidate_version):
    name = f'Grove-{candidate_version}-{PLATFORMS[platform]}'
    files = sorted(p.name for p in directory.iterdir())
    if files != [name] or not (directory / name).is_file():
        raise ValueError(f'Expected exactly one installer: {name}; got {files}')
    manifest = dict(schema_version=1, platform=platform, sha=sha,
                    version=candidate_version, file=name, sha256=digest(directory / name))
    (directory / 'candidate.json').write_text(json.dumps(manifest, indent=2) + '\n')


def validate_run(run, repository, sha):
    if (run.get('path') != '.github/workflows/release.yml'
            or run.get('head_repository', {}).get('full_name') != repository
            or run.get('head_branch') != 'main'
            or run.get('event') not in ('push', 'workflow_dispatch')
            or run.get('status') != 'completed' or run.get('conclusion') != 'success'
            or run.get('head_sha') != sha):
        raise ValueError('Candidate must be a successful main preparation run for the tagged commit')


def verify(directory, sha, candidate_version):
    """Reject incomplete, mixed-revision, renamed or modified candidate artifacts."""
    expected_directories = {'candidate-' + p for p in PLATFORMS}
    if {p.name for p in directory.iterdir()} != expected_directories:
        raise ValueError('Expected exactly four platform candidate artifacts')
    installers = []
    for platform, suffix in PLATFORMS.items():
        folder = directory / ('candidate-' + platform)
        if folder.is_symlink() or not folder.is_dir():
            raise ValueError(f'Invalid candidate folder: {platform}')
        manifest = json.loads((folder / 'candidate.json').read_text())
        name = f'Grove-{candidate_version}-{suffix}'
        if (manifest.get('schema_version') != 1 or manifest.get('platform') != platform
                or manifest.get('sha') != sha or manifest.get('version') != candidate_version
                or manifest.get('file') != name):
            raise ValueError(f'Candidate identity mismatch: {platform}')
        if {p.name for p in folder.iterdir()} != {name, 'candidate.json'}:
            raise ValueError(f'Unexpected candidate contents: {platform}')
        installer = folder / name
        if installer.is_symlink() or not installer.is_file() or digest(installer) != manifest.get('sha256'):
            raise ValueError(f'Installer checksum mismatch: {platform}')
        installers.append(installer)
    return installers


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='action', required=True)
    tag_parser = sub.add_parser('check-tag')
    tag_parser.add_argument('tag')
    record_parser = sub.add_parser('record')
    record_parser.add_argument('directory', type=Path)
    record_parser.add_argument('platform', choices=PLATFORMS)
    verify_parser = sub.add_parser('download')
    verify_parser.add_argument('run_id')
    verify_parser.add_argument('tag')
    verify_parser.add_argument('directory', type=Path)
    args = parser.parse_args()
    if args.action == 'check-tag':
        check_tag(args.tag, version())
    elif args.action == 'record':
        record(args.directory, args.platform, command('git', 'rev-parse', 'HEAD'), version())
    else:
        check_tag(args.tag, version())
        if not args.run_id.isdigit():
            raise ValueError('Expected a numeric candidate run ID')
        repository = os.environ['GITHUB_REPOSITORY']
        sha = command('git', 'rev-parse', 'HEAD')
        run = json.loads(command('gh', 'api', f'repos/{repository}/actions/runs/{args.run_id}'))
        validate_run(run, repository, sha)
        # Start with an empty folder: leftovers must never substitute for expired artifacts.
        args.directory.mkdir(parents=True, exist_ok=False)
        command('gh', 'run', 'download', args.run_id, '--repo', repository,
                '--pattern', 'candidate-*', '--dir', str(args.directory))
        for installer in verify(args.directory, sha, version()):
            print(installer)


if __name__ == '__main__':
    main()
