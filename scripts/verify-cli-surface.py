#!/usr/bin/env python3
"""Disposable CLI acceptance; stand-in agents, one GUI owner, raw JSON evidence.

Run after installation: python3 scripts/verify-cli-surface.py --binary ~/.cargo/bin/grove
No real coding agents or foreign configs/sessions are operated on. Evidence remains
under /tmp; the explicitly started owner and all created worktrees are cleaned up.
"""
import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import plistlib
import signal
import shlex
import shutil
import subprocess
import sys
import tempfile
import time


def windows(pid):
    """Count native layer-zero windows without Accessibility permissions."""
    if sys.platform != 'darwin':
        return None
    cg = ctypes.CDLL('/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics')
    cf = ctypes.CDLL('/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation')
    cg.CGWindowListCopyWindowInfo.argtypes = [ctypes.c_uint32, ctypes.c_uint32]
    cg.CGWindowListCopyWindowInfo.restype = ctypes.c_void_p
    cf.CFArrayGetCount.argtypes = [ctypes.c_void_p]
    cf.CFArrayGetCount.restype = ctypes.c_long
    cf.CFArrayGetValueAtIndex.argtypes = [ctypes.c_void_p, ctypes.c_long]
    cf.CFArrayGetValueAtIndex.restype = ctypes.c_void_p
    cf.CFDictionaryGetValue.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
    cf.CFDictionaryGetValue.restype = ctypes.c_void_p
    cf.CFStringCreateWithCString.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_uint32]
    cf.CFStringCreateWithCString.restype = ctypes.c_void_p
    cf.CFNumberGetValue.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_void_p]
    cf.CFRelease.argtypes = [ctypes.c_void_p]
    keys = [cf.CFStringCreateWithCString(None, key, 0x08000100)
            for key in (b'kCGWindowOwnerPID', b'kCGWindowLayer')]
    rows = cg.CGWindowListCopyWindowInfo(0, 0)
    try:
        count = 0
        for i in range(cf.CFArrayGetCount(rows)):
            row = cf.CFArrayGetValueAtIndex(rows, i)
            values = []
            for key in keys:
                number = cf.CFDictionaryGetValue(row, key)
                value = ctypes.c_int()
                if number:
                    cf.CFNumberGetValue(number, 9, ctypes.byref(value))
                values.append(value.value)
            count += values == [pid, 0]
        return count
    finally:
        cf.CFRelease(rows)
        for key in keys:
            cf.CFRelease(key)


def processes():
    rows = subprocess.check_output(['ps', '-axo', 'pid=,comm='], text=True)
    return {int(row.split(None, 1)[0]) for row in rows.splitlines()
            if row.strip() and Path(row.split(None, 1)[1]).name == 'grove'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True, type=Path)
    args = parser.parse_args()
    binary = args.binary.expanduser().absolute()
    root = Path(tempfile.mkdtemp(prefix='grove-cli-surface-', dir='/tmp')).resolve()
    print('EVIDENCE=' + str(root), flush=True)
    config = root / 'config'
    config.mkdir()
    fakebin = root / 'stand-in-agents'
    fakebin.mkdir()
    # All supported coding agents use deterministic executable stand-ins. They
    # record the prompt/context and stay alive; task completion is explicit CLI.
    for agent in ('codex', 'claude', 'opencode'):
        file = fakebin / agent
        file.write_text('#!/bin/sh\n' +
                        'printf "%s\\n" "$@" > "$GROVE_ACCEPTANCE_ROOT/' + agent + '-args.txt"\n' +
                        'printf "%s" "$GROVE_TASK_ID" > "$GROVE_ACCEPTANCE_ROOT/' + agent + '-task.txt"\n' +
                        'exec /bin/sleep 300\n')
        file.chmod(0o755)
    shell_wrapper = root / 'fixture-shell'
    shell_wrapper.write_text('#!/bin/sh\n'
        'case "$1" in\n'
        '  -ilc|-lic|-lc|-ic) shift; exec /bin/sh -c "$@";;\n'
        '  -l) shift;;\n'
        'esac\n'
        'exec /bin/sh "$@"\n')
    shell_wrapper.chmod(0o755)
    env = dict(os.environ, SHELL=str(shell_wrapper), GROVE_CONFIG_DIR=str(config),
               GROVE_ACCEPTANCE_ROOT=str(root),
               PATH=str(fakebin) + os.pathsep + str(binary.parent) + os.pathsep + os.environ['PATH'])
    (config / 'projects.json').write_text(json.dumps({
        'projects': [], 'tmux_enabled': False, 'onboarded': True,
        'dangerously_skip_permissions_enabled': False, 'telemetry_enabled': False}))
    evidence = []
    owner = None
    baseline = processes()
    owner_windows = None
    owned_projects = []
    tracked_worktrees = []
    owned_children = {}

    def check_surface():
        if owner:
            rows = subprocess.check_output(['ps', '-axo', 'pid=,ppid=,command='], text=True)
            table = {}
            for line in rows.splitlines():
                pieces = line.strip().split(None, 2)
                if len(pieces) == 3:
                    table[int(pieces[0])] = (int(pieces[1]), pieces[2])
            parents = {owner.pid}
            for _ in range(10):
                children = {pid for pid, (ppid, _) in table.items() if ppid in parents}
                if children <= parents:
                    break
                parents |= children
            owned_children.update({pid: table[pid][1] for pid in parents - {owner.pid}})
        expected = baseline | ({owner.pid} if owner and owner.poll() is None else set())
        actual = processes()
        # Concurrent short CLI clients may still be exiting; extra GUI owners persist.
        for _ in range(10):
            if actual == expected:
                break
            time.sleep(.1)
            actual = processes()
        assert actual == expected, ('unexpected Grove process', expected, actual)
        count = windows(owner.pid) if owner and owner.poll() is None else None
        if owner_windows is not None:
            assert count == owner_windows, ('extra Grove window', owner_windows, count)
        return {'grove_pids': sorted(actual), 'owner_windows': count}

    def cli(*command, ok=True):
        result = subprocess.run([str(binary), *map(str, command)], env=env,
                                text=True, capture_output=True, timeout=30)
        row = {'argv': list(map(str, command)), 'exit': result.returncode,
               'stdout': result.stdout, 'stderr': result.stderr}
        evidence.append(row)
        (root / 'commands.json').write_text(json.dumps(evidence, indent=2))
        data = json.loads(result.stdout)
        assert data['version'] == 1, data
        assert data['ok'] == ok and (result.returncode == 0) == ok, row
        row.update(check_surface())
        (root / 'commands.json').write_text(json.dumps(evidence, indent=2))
        return data['data'] if ok else data

    def poll(command, predicate, timeout=15):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            data = cli(*command)
            if predicate(data):
                return data
            time.sleep(.12)
        raise AssertionError(('timed out', command, data))

    def project(name):
        path = root / name
        path.mkdir()
        cli('projects', 'add', '--name', name, '--path', path, '--workspace', workspace)
        owned_projects.append(path)
        cli('projects', 'init-git', '--project', path)
        subprocess.run(['git', '-C', str(path), '-c', 'user.name=CLI acceptance',
                        '-c', 'user.email=acceptance@example.invalid', 'commit',
                        '--allow-empty', '-m', 'Disposable baseline'], check=True,
                       capture_output=True)
        return path

    def worktree(path, name):
        data = cli('worktrees', 'create', '--project', path, '--name', name,
                   '--base', 'HEAD', '--branch', name)
        tracked_worktrees.append((path, data['path']))
        return data['path']

    def remove_wt(path, wt, skip=False):
        cli('worktrees', 'remove', '--project', path, '--worktree', wt, '--confirm', wt)
        if skip:
            poll(('worktrees', 'removal-status', '--path', wt),
                 lambda row: row['stage'] == 'RunningScript')
            cli('worktrees', 'skip-teardown', '--path', wt, '--confirm', wt)
        result = poll(('worktrees', 'removal-status', '--path', wt), lambda row: row['finished'])
        assert result['error'] is None, result
        assert not Path(wt).exists(), wt
        tracked_worktrees.remove((path, wt))

    try:
        cli('--help', '--json')
        cli('--version')
        cli('projects', 'list', ok=False)  # No auto-launch when unavailable.
        cli('not-a-command', ok=False)
        launch = binary
        if sys.platform == 'darwin':
            bundle = root / 'Grove CLI Acceptance.app' / 'Contents'
            launch = bundle / 'MacOS' / 'grove'
            launch.parent.mkdir(parents=True)
            shutil.copy2(binary, launch)
            with (bundle / 'Info.plist').open('wb') as file:
                plistlib.dump({'CFBundleName': 'Grove CLI Acceptance',
                               'CFBundleIdentifier': 'dev.grove.acceptance.' + root.name,
                               'CFBundleExecutable': 'grove', 'CFBundlePackageType': 'APPL',
                               'CFBundleVersion': '1', 'NSHighResolutionCapable': True}, file)
        log = (root / 'desktop.log').open('w')
        owner = subprocess.Popen([str(launch)], env=env, stdout=log,
                                 stderr=subprocess.STDOUT, start_new_session=True)
        for _ in range(100):
            assert owner.poll() is None, (root / 'desktop.log').read_text()
            probe = subprocess.run([str(binary), 'projects', 'list'], env=env,
                                   text=True, capture_output=True, timeout=5)
            (root / 'readiness.json').write_text(json.dumps({'exit': probe.returncode, 'stdout': probe.stdout, 'windows': windows(owner.pid)}))
            if probe.returncode == 0:
                break
            time.sleep(.1)
        else:
            raise AssertionError('Owner did not become ready with one window')
        time.sleep(.7)  # Let native window startup surfaces settle before baseline.
        owner_windows = windows(owner.pid)
        if sys.platform == 'darwin':
            assert owner_windows >= 1, owner_windows
        check_surface()
        (root / 'owner.json').write_text(json.dumps({'pid': owner.pid,
            'config': str(config), 'binary': str(binary),
            'sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
            'windows': owner_windows, 'stand_in_agents': True}, indent=2))
        cli('projects', 'list', '--json')
        cli('workspaces', 'list')
        workspace = cli('workspaces', 'create', '--name', 'Disposable')['id']
        cli('workspaces', 'rename', workspace, '--name', 'Verified')
        cli('workspaces', 'select', workspace)
        path = project('surface-' + root.name)
        cli('projects', 'edit', '--project', path, '--name', 'Renamed')
        cli('projects', 'move', '--project', path, '--workspace', '1')
        cli('projects', 'move', '--project', path, '--workspace', workspace)
        cli('projects', 'archive', '--project', path)
        cli('projects', 'restore', '--project', path)
        for agent in ('codex', 'claude', 'opencode'):
            cli('skills', 'status', '--agent', agent)
            cli('skills', 'status', '--agent', agent, '--project', path)
            cli('skills', 'install', '--agent', agent, '--project', path)
            cli('skills', 'install', '--agent', agent, '--project', path, '--overwrite', 'false')
            cli('skills', 'install', '--agent', agent, '--project', path, '--overwrite', 'true')
        setup = root / 'setup.sh'; setup.write_text('printf setup > setup-proof\n')
        run = root / 'run.sh'; run.write_text('printf run > run-proof\nsleep 300\n')
        teardown = root / 'teardown.sh'; teardown.write_text('printf teardown > ' + shlex.quote(str(root / 'teardown-proof')) + '\n')
        empty = root / 'empty.sh'; empty.write_text('')
        cli('projects', 'edit', '--project', path, '--setup-file', setup,
            '--run-file', run, '--teardown-file', teardown)
        wt = worktree(path, 'surface-worker')
        assert any(row['scripts']['setup'] == setup.read_text()
                   for row in cli('projects', 'list') if row['path'] == str(path))
        cli('worktrees', 'list', '--project', path)
        anchor = cli('sessions', 'start', '--project', path, '--worktree', wt,
                     '--agent', 'terminal', '--backend', 'native', '--root', path)
        shell = cli('sessions', 'shell', '--project', path, '--worktree', wt)
        home = cli('sessions', 'shell')
        running = cli('sessions', 'run', '--project', path, '--worktree', wt)
        deadline = time.monotonic() + 10
        while not (Path(wt) / 'run-proof').exists() and time.monotonic() < deadline:
            time.sleep(.1)
        assert (Path(wt) / 'run-proof').read_text() == 'run'
        inp = root / 'input.txt'
        proof = root / 'input-proof'
        inp.write_text('printf literal > ' + shlex.quote(str(proof)) + '\r')
        cli('sessions', 'input', shell['id'], '--input-file', inp)
        deadline = time.monotonic() + 10
        while not proof.exists() and time.monotonic() < deadline:
            time.sleep(.1)
        assert proof.read_text() == 'literal'
        agent_paths = root / 'agent-path-proof'
        inp.write_text('command -v codex claude opencode > ' + shlex.quote(str(agent_paths)) + '\r')
        cli('sessions', 'input', shell['id'], '--input-file', inp)
        deadline = time.monotonic() + 10
        while not agent_paths.exists() and time.monotonic() < deadline:
            time.sleep(.1)
        assert agent_paths.read_text().splitlines() == [str(fakebin / agent)
            for agent in ('codex', 'claude', 'opencode')], agent_paths.read_text()
        cli('sessions', 'list')
        for session in (anchor, shell, home, running):
            cli('sessions', 'show', session['id'])
            cli('sessions', 'logs', session['id'], '--lines', '50')
            cli('sessions', 'focus', session['id'])
        prompt = root / 'prompt.txt'; prompt.write_text('DETERMINISTIC_STAND_IN_PROMPT')
        result = root / 'result.json'
        result.write_text(json.dumps({'status': 'completed', 'summary': 'Stand-in acceptance',
                                     'changed_files': [], 'checks': ['Recorded prompt'], 'unresolved': []}))
        parent = None
        task_sessions = []
        for agent in ('codex', 'claude', 'opencode'):
            command = ['sessions', 'start', '--project', path, '--worktree', wt,
                       '--agent', agent, '--backend', 'native', '--root', path,
                       '--prompt-file', prompt, '--task-title', agent + ' stand-in',
                       '--task-file', prompt, '--request-id', 'surface-' + agent]
            if parent:
                command += ['--parent-task', parent]
            session = cli(*command)
            retry = cli(*command)
            assert session == retry, (session, retry)
            task = session['task_id']
            parent = parent or task
            deadline = time.monotonic() + 10
            while not (root / (agent + '-args.txt')).exists() and time.monotonic() < deadline:
                time.sleep(.1)
            recorded = (root / (agent + '-args.txt')).read_text()
            assert 'DETERMINISTIC_STAND_IN_PROMPT' in recorded, recorded
            assert 'dangerously' not in recorded, recorded
            assert (root / (agent + '-task.txt')).read_text() == task
            cli('tasks', 'list')
            cli('tasks', 'show', task)
            cli('tasks', 'wait', task, '--timeout', '0', ok=False)
            task_sessions.append((task, session['id']))
        for task, session_id in reversed(task_sessions):
            cli('tasks', 'complete', task, '--result-file', result)
            completed = cli('tasks', 'wait', task, '--timeout', '1')
            assert completed['status'] == 'completed', completed
            cli('sessions', 'stop', session_id)
        # Exercise tmux requested backend; returned backend exposes any fallback.
        session = cli('sessions', 'start', '--project', path, '--worktree', wt,
                      '--agent', 'terminal', '--backend', 'tmux')
        (root / 'tmux-backend.json').write_text(json.dumps({'requested': 'tmux', 'actual': session['backend'], 'tmux_installed': shutil.which('tmux') is not None}, indent=2))
        if shutil.which('tmux'):
            assert session['backend'] == 'tmux', session
        cli('sessions', 'stop', session['id'])
        for session in (anchor, shell, home, running):
            cli('sessions', 'stop', session['id'])
        remove_wt(path, wt)
        assert (root / 'teardown-proof').read_text() == 'teardown'
        teardown.write_text('sleep 300\n')
        cli('projects', 'edit', '--project', path, '--teardown-file', teardown)
        wt = worktree(path, 'skip-worker')
        remove_wt(path, wt, skip=True)
        cli('projects', 'edit', '--project', path, '--setup-file', empty,
            '--run-file', empty, '--teardown-file', empty)
        cli('projects', 'archive', '--project', path)
        cli('projects', 'delete', '--project', path, '--confirm', 'Renamed')
        owned_projects.remove(path)
        for remove in ('false', 'true'):
            name = 'remove-' + remove + '-' + root.name
            project_path = project(name)
            wt = worktree(project_path, 'remove-' + remove)
            cli('projects', 'remove', '--project', project_path, '--confirm', name,
                '--remove-worktrees', remove)
            status = poll(('projects', 'removal-status', '--path', project_path),
                          lambda row: row['finished'])
            assert not status['errors'] and status['unregistered'], status
            assert Path(wt).exists() == (remove == 'false')
            if remove == 'false':
                subprocess.run(['git', '-C', str(project_path), 'worktree', 'remove', wt],
                               check=True, capture_output=True)
            tracked_worktrees.remove((project_path, wt))
            owned_projects.remove(project_path)
        cli('workspaces', 'delete', workspace, '--confirm', 'Verified')
        cli('projects', 'list'); cli('sessions', 'list'); cli('tasks', 'list')
        summary = {'status': 'passed', 'commands_checked': len(evidence),
                   'single_owner_pid': owner.pid, 'owner_window_count': owner_windows,
                   'user_scope_skill_install': 'not run: preserves foreign user files',
                   'agent_launches': 'deterministic stand-ins; no external coding agents'}
        (root / 'summary.json').write_text(json.dumps(summary, indent=2))
        print(json.dumps(summary), flush=True)
    finally:
        if owner and owner.poll() is None:
            owner.terminate()
            try:
                owner.wait(timeout=10)
            except subprocess.TimeoutExpired:
                owner.kill(); owner.wait(timeout=10)
        for project_path, wt in tracked_worktrees:
            subprocess.run(['git', '-C', str(project_path), 'worktree', 'remove', '--force', wt],
                           capture_output=True)
        # Verify all fixture worktrees were removed; /tmp evidence stays.
        for project_path in root.iterdir():
            if project_path.is_dir() and (project_path / '.git').exists():
                listing = subprocess.run(['git', '-C', str(project_path), 'worktree', 'list', '--porcelain'],
                                         text=True, capture_output=True).stdout
                assert listing.count('worktree ') == 1, listing
        # Native sessions should close their PTYs; clean only identified owned
        # descendants that survived an aborted run, matching their command too.
        for _ in range(20):
            rows = subprocess.check_output(['ps', '-axo', 'pid=,command='], text=True)
            living = {int(row.strip().split(None, 1)[0]): row.strip().split(None, 1)[1]
                      for row in rows.splitlines() if len(row.strip().split(None, 1)) == 2}
            survivors = {pid for pid, command in owned_children.items()
                         if living.get(pid) == command}
            if not survivors:
                break
            for pid in survivors:
                try:
                    os.kill(pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
            time.sleep(.1)
        assert not survivors, ('owned child processes survived cleanup', survivors)
        assert processes() == baseline, ('Grove process remains after cleanup', processes(), baseline)
        (root / 'cleanup.json').write_text(json.dumps({'owner_exit': owner.returncode if owner else None,
            'owned_children_checked': len(owned_children), 'surviving_children': [],
            'grove_pids': sorted(processes())}, indent=2))
        print('CLEANUP_OWNER_EXIT=' + str(owner.returncode if owner else None), flush=True)


if __name__ == '__main__':
    main()
