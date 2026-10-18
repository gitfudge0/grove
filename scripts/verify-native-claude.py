#!/usr/bin/env python3
"""Opt-in real Claude Code acceptance through an isolated installed Grove owner.

Uses the signed-in local CLI, sends harmless prompts, and leaves private evidence
under /tmp. Never operates the user's running Grove or modifies Claude Code auth/config.
No native window inspection is performed; this verifies runtime/protocol contracts.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import shutil
import signal
import subprocess
import sys
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True, type=Path)
    parser.add_argument('--real-claude', action='store_true', required=True,
                        help='Explicitly permit using existing Claude Code authentication for harmless prompts')
    parser.add_argument('--inspect-ui', action='store_true',
                        help='Pause on stdin before cleanup for isolated native UI inspection')
    args = parser.parse_args()
    binary = args.binary.expanduser().resolve()
    root = Path(tempfile.mkdtemp(prefix='grove-native-claude-', dir='/tmp')).resolve()
    os.chmod(root, 0o700)
    print('EVIDENCE=' + str(root), flush=True)
    config = root / 'config'
    config.mkdir()
    project = root / 'project'
    project.mkdir()
    subprocess.run(['/usr/bin/git', 'init', str(project)], check=True, capture_output=True)
    subprocess.run(['/usr/bin/git', '-C', str(project), '-c', 'user.name=Native acceptance',
                    '-c', 'user.email=acceptance@example.invalid', 'commit', '--allow-empty',
                    '-m', 'Disposable baseline'], check=True, capture_output=True)
    # Deliberately mimic Finder's sparse PATH. Real login-shell capture and the same
    # resolver used by UI availability must recover the configured user install.
    env = dict(os.environ, GROVE_CONFIG_DIR=str(config),
               PATH='/usr/bin:/bin:/usr/sbin:/sbin', GROVE_FORCE_LOGIN_PATH='1')
    env.pop('GROVE_TEST_CONFIG_DIR', None)
    (config / 'projects.json').write_text(json.dumps({
        'projects': [], 'tmux_enabled': False, 'native_agents_enabled': True, 'onboarded': True,
        'dangerously_skip_permissions_enabled': False, 'telemetry_enabled': False}))
    evidence = []
    owner = None
    session_id = None
    descendants = {}

    def record_children():
        if owner is None:
            return
        rows = subprocess.check_output(['/bin/ps', '-axo', 'pid=,ppid=,command='], text=True)
        table = {}
        for line in rows.splitlines():
            parts = line.strip().split(None, 2)
            if len(parts) == 3:
                table[int(parts[0])] = (int(parts[1]), parts[2])
        parents = {owner.pid}
        for _ in range(20):
            children = {pid for pid, (ppid, _) in table.items() if ppid in parents}
            if children <= parents:
                break
            parents |= children
        descendants.update({pid: table[pid][1] for pid in parents - {owner.pid}})

    def cli(*command, timeout=30):
        result = subprocess.run([str(binary), *map(str, command)], env=env,
                                text=True, capture_output=True, timeout=timeout)
        row = {'argv': list(map(str, command)), 'exit': result.returncode,
               'stdout': result.stdout, 'stderr': result.stderr}
        evidence.append(row)
        (root / 'commands.json').write_text(json.dumps(evidence, indent=2))
        record_children()
        data = json.loads(result.stdout)
        assert result.returncode == 0 and data['ok'], row
        return data['data']

    def poll(fn, predicate, timeout=120):
        deadline = time.monotonic() + timeout
        last = None
        while time.monotonic() < deadline:
            last = fn()
            if predicate(last):
                return last
            time.sleep(.2)
        raise AssertionError(('Timed out', last))

    def start_owner():
        nonlocal owner
        launch = binary
        if sys.platform == 'darwin':
            contents = root / 'Grove Native Acceptance.app' / 'Contents'
            launch = contents / 'MacOS' / 'grove'
            if not launch.exists():
                launch.parent.mkdir(parents=True)
                shutil.copy2(binary, launch)
                with (contents / 'Info.plist').open('wb') as file:
                    plistlib.dump({'CFBundleName': 'Grove Native Acceptance',
                                   'CFBundleIdentifier': 'dev.grove.native-acceptance.' + root.name,
                                   'CFBundleExecutable': 'grove', 'CFBundlePackageType': 'APPL',
                                   'CFBundleVersion': '1', 'NSHighResolutionCapable': True}, file)
        log = (root / 'desktop.log').open('a')
        owner = subprocess.Popen([str(launch)], env=env, stdout=log,
                                 stderr=subprocess.STDOUT, start_new_session=True)
        for _ in range(200):
            assert owner.poll() is None, (root / 'desktop.log').read_text()
            probe = subprocess.run([str(binary), 'sessions', 'list'], env=env,
                                   text=True, capture_output=True, timeout=5)
            if probe.returncode == 0:
                return
            time.sleep(.1)
        raise AssertionError('Isolated owner did not become ready')

    def clean_children():
        # Only known descendants with unchanged PID/command identity are ours.
        remaining = set()
        for _ in range(30):
            living = {}
            rows = subprocess.check_output(['/bin/ps', '-axo', 'pid=,command='], text=True)
            for line in rows.splitlines():
                parts = line.strip().split(None, 1)
                if len(parts) == 2:
                    living[int(parts[0])] = parts[1]
            remaining = {pid for pid, command in descendants.items() if living.get(pid) == command}
            if not remaining:
                break
            for pid in remaining:
                try:
                    os.kill(pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
            time.sleep(.1)
        return remaining

    def stop_owner():
        record_children()
        if owner and owner.poll() is None:
            owner.terminate()
            try:
                owner.wait(timeout=15)
            except subprocess.TimeoutExpired:
                owner.kill()
                owner.wait(timeout=5)
        assert not clean_children(), 'Owned Claude Code descendants survived owner shutdown'

    def status():
        rows = cli('sessions', 'list')
        row = next(row for row in rows if row['id'] == session_id)
        assert not row.get('spawn_error'), row
        assert row['status'] not in ('failed', 'exited'), row
        return row

    def logs():
        return cli('sessions', 'logs', session_id, '--lines', '500')

    def text(value):
        return json.dumps(value)

    try:
        start_owner()
        (root / 'owner.json').write_text(json.dumps({'binary': str(binary),
            'sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
            'initial_owner_pid': owner.pid, 'config': str(config),
            'inherited_path': env['PATH'], 'real_claude': True}, indent=2))
        cli('projects', 'add', '--name', 'Native acceptance', '--path', project)
        session = cli('sessions', 'start', '--project', project, '--worktree', project,
                      '--agent', 'claude', '--backend', 'native')
        session_id = session['id']
        assert session['backend'] == 'native', session
        poll(status, lambda row: row['native_status'].lower() == 'idle', timeout=60)
        recovery = list((config / 'codex-sessions').glob('*.json'))
        poll(lambda: list((config / 'codex-sessions').glob('*.json')), bool, timeout=10)
        recovery = list((config / 'codex-sessions').glob('*.json'))
        assert len(recovery) == 1
        prompt = root / 'prompt.txt'
        prompt.write_text('Remember MARIGOLD for this conversation. Run exactly `pwd` once, then reply in Markdown with a heading, a bullet, and a fenced code block containing MARIGOLD. Do not modify files or access other directories.')
        cli('sessions', 'input', session_id, '--input-file', prompt)
        poll(logs, lambda value: str(project) in text(value), timeout=120)
        poll(status, lambda row: row['native_status'].lower() == 'done', timeout=120)
        output = text(logs())
        assert 'MARIGOLD' in output and '```' in output, output
        saved = poll(lambda: json.loads(recovery[0].read_text()), lambda value: any(item['type'] == 'agentMessage' and '```' in item.get('text', '') for item in value['items']), timeout=10)
        assert saved['provider'] == 'claude', saved
        assistant_blocks = [(item['id'].rsplit('-', 1)[0], item.get('text', ''))
                            for item in saved['items'] if item['type'] == 'agentMessage']
        assert len(assistant_blocks) == len(set(assistant_blocks)), ('Duplicate assistant blocks', saved['items'])
        assert saved['thread_id'] and any(item['type'] == 'agentMessage' for item in saved['items']), saved
        assert any(item['type'] == 'commandExecution' for item in saved['items']), saved
        original_thread = saved['thread_id']
        stop_owner()
        start_owner()
        poll(status, lambda row: row['native_status'].lower() == 'idle', timeout=60)
        assert json.loads(recovery[0].read_text())['thread_id'] == original_thread
        prompt.write_text('What word did I ask you to remember? Reply only that word. Do not use tools or modify files.')
        cli('sessions', 'input', session_id, '--input-file', prompt)
        poll(status, lambda row: row['native_status'].lower() == 'done', timeout=120)
        final_saved = poll(lambda: json.loads(recovery[0].read_text()), lambda value: len([item for item in value['items'] if item['type'] == 'agentMessage']) > len([item for item in saved['items'] if item['type'] == 'agentMessage']), timeout=10)
        replies = [item.get('text', '') for item in final_saved['items'] if item['type'] == 'agentMessage']
        assert replies and 'MARIGOLD' in replies[-1], replies
        if args.inspect_ui:
            print('INSPECT_UI=' + json.dumps({'app': str(root / 'Grove Native Acceptance.app'),
                  'config': str(config), 'session_id': session_id}), flush=True)
            input('Press Enter to stop the acceptance session and clean up: ')
        cli('sessions', 'stop', session_id)
        poll(lambda: cli('sessions', 'list'), lambda rows: not rows, timeout=10)
        poll(lambda: list((config / 'codex-sessions').glob('*.json')), lambda files: not files, timeout=10)
        summary = {'status': 'passed', 'commands_checked': len(evidence),
                   'checks': ['sparse-path native create', 'actual prompt and tool output',
                              'Markdown output', 'live conversation updates',
                              'owner restart and same-thread resume', 'follow-up recall',
                              'session stop and private recovery cleanup'],
                   'visual_interaction': 'requires separate native UI verification'}
        (root / 'summary.json').write_text(json.dumps(summary, indent=2))
        print(json.dumps(summary), flush=True)
    finally:
        stop_owner()
        remaining = clean_children()
        (root / 'cleanup.json').write_text(json.dumps({'owner_exit': owner.returncode if owner else None,
                                                      'surviving_owned_children': sorted(remaining)}))
        assert not remaining, remaining
        print('CLEANUP_OWNER_EXIT=' + str(owner.returncode if owner else None), flush=True)


if __name__ == '__main__':
    main()
