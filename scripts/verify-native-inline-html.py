#!/usr/bin/env python3
"""Launch an isolated Grove HTML fixture with a synthetic Codex provider.

No credentials or main Grove config are used. Stop only the PID in fixture.json.
Send 'link', 'version', 'missing', or 'unfinished' to the fixture conversation.
"""
import argparse
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile
import time

FAKE = r'''import json, os, pathlib, sys, time
root = pathlib.Path(os.environ['GROVE_HTML_FIXTURE'])
project = root/'project'
thread, seq = 'inline-html-fixture', 0
def send(value): print(json.dumps(value), flush=True)
def document(title, version=1):
    return """<!doctype html><html><head><title>"""+title+"""</title><style>body{font:16px system-ui;margin:24px;background:#fff;color:#111}input,button{font:inherit;padding:8px;margin:6px}section{height:900px}nav{position:sticky;top:0;background:#fff}#save{color:green}</style></head><body>
<h1>"""+title+"""</h1><p>Version """+str(version)+""" · self-contained HTML+JS</p>
<nav><button onclick="document.body.dataset.tab='overview';document.querySelector('#tab').textContent='Overview'">Overview</button><button onclick="document.body.dataset.tab='tasks';document.querySelector('#tab').textContent='Tasks'">Tasks</button><b id="tab">Overview</b></nav>
<form onsubmit="event.preventDefault();document.querySelector('#save').textContent='Saved '+document.querySelector('#release').value"><label>Release <input id="release" value="October rollout"></label><button>Save changes</button><span id="save"></span></form>
<button id="count" onclick="this.textContent=Number(this.textContent)+1">0</button><a href="#bottom">Jump to bottom</a>
<p id="boundary">Checking blocked network…</p><script>fetch('http://127.0.0.1:19451/fixture-marker').then(()=>document.querySelector('#boundary').textContent='UNEXPECTED network access',()=>document.querySelector('#boundary').textContent='Network blocked');</script>
<section><h2>Scroll within this document</h2><p>Edit, select Tasks, increment, then scroll before Expand and Return.</p></section><h2 id="bottom">Bottom anchor</h2><input placeholder="e.g. Launch notes"></body></html>"""
for line in sys.stdin:
    request=json.loads(line); method=request.get('method'); ident=request.get('id')
    if method=='initialize': send({'id':ident,'result':{'userAgent':'offline-html-fixture'}})
    elif method=='config/read': send({'id':ident,'result':{'config':{'sandbox_workspace_write':{'writable_roots':[]}}}})
    elif method in ('thread/start','thread/resume'): send({'id':ident,'result':{'thread':{'id':thread,'turns':[]}}})
    elif method=='model/list': send({'id':ident,'result':{'data':[{'id':'fixture-model','model':'fixture-model','displayName':'Offline HTML fixture','supportedReasoningEfforts':[],'defaultReasoningEffort':'medium','isDefault':True}],'nextCursor':None}})
    elif method=='skills/list': send({'id':ident,'result':{'data':[]}})
    elif method=='turn/start':
        seq+=1; turn='html-turn-'+str(seq); prompt=request['params']['input'][0]['text'].lower()
        send({'id':ident,'result':{'turn':{'id':turn,'status':'inProgress'}}})
        send({'method':'turn/started','params':{'threadId':thread,'turn':{'id':turn}}})
        send({'method':'item/completed','params':{'threadId':thread,'turnId':turn,'item':{'id':'user-'+str(seq),'type':'userMessage','content':[{'type':'inputText','text':prompt}]}}})
        path=project/'launch.html';path.write_text(document('Linked launch checklist',seq))
        if 'version' in prompt:
            text='The linked file has changed. Use Check for updates on the earlier preview, then explicitly Load new version. [Current document](launch.html)'
        elif 'missing' in prompt: text='[Missing HTML document](missing.html)'
        elif 'link' in prompt: text='The last run produced [Launch checklist](launch.html).'
        else:
            text='Here are two independent live documents.\n\n```html\n'+document('First fenced checklist')+'\n```\n\n```html\n'+document('Second fenced checklist')+'\n```\n\n'+('\n\n'.join('Conversation paragraph '+str(i)+': local linked document appears after this prose.' for i in range(12)))+'\n\n[Generated launch checklist](launch.html)\n\n[Other link remains external](https://example.invalid/)'
            if 'unfinished' in prompt: text+='\n\n```html\n<!doctype html><html><body>Still receiving'
        item='answer-'+str(seq)
        send({'method':'item/started','params':{'threadId':thread,'turnId':turn,'item':{'type':'agentMessage','id':item,'text':''}}})
        for start in range(0,len(text),500):
            send({'method':'item/agentMessage/delta','params':{'threadId':thread,'turnId':turn,'itemId':item,'delta':text[start:start+500]}});time.sleep(.06)
        send({'method':'item/completed','params':{'threadId':thread,'turnId':turn,'item':{'type':'agentMessage','id':item,'text':text}}})
        time.sleep(.25)
        send({'method':'turn/completed','params':{'threadId':thread,'turn':{'id':turn,'status':'completed'}}})
    elif method=='turn/interrupt': send({'id':ident,'result':{}})
    elif ident is not None: send({'id':ident,'result':{}})
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True, type=Path)
    parser.add_argument('--scenario', default='link')
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve()
    root = Path(tempfile.mkdtemp(prefix='grove-inline-html-', dir='/tmp')).resolve()
    root.chmod(0o700)
    config, project, fakebin = (root/name for name in ('config', 'project', 'bin'))
    for path in (config, project, fakebin): path.mkdir()
    subprocess.run(['git', 'init', str(project)], check=True, capture_output=True)
    subprocess.run(['git', '-C', str(project), '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '--allow-empty', '-m', 'Disposable fixture'], check=True, capture_output=True)
    fake = fakebin/'codex'
    fake.write_text('#!'+sys.executable+'\n'+FAKE)
    fake.chmod(0o755)
    shell = root/'shell'
    shell.write_text('#!/bin/sh\ncase "$1" in -ilc|-lic|-lc|-ic) shift; exec /bin/sh -c "$@";; -l) shift;; esac\nexec /bin/sh "$@"\n')
    shell.chmod(0o755)
    env = dict(os.environ, SHELL=str(shell), GROVE_CONFIG_DIR=str(config), GROVE_HTML_FIXTURE=str(root), PATH=str(fakebin)+os.pathsep+os.environ['PATH'])
    for name in ('GROVE_TEST_CONFIG_DIR', 'GROVE_FORCE_LOGIN_PATH', 'GROVE_TASK_ID'): env.pop(name, None)
    (config/'projects.json').write_text(json.dumps({'projects': [], 'tmux_enabled': False, 'native_agents_enabled': True, 'onboarded': True, 'telemetry_enabled': False}))
    app_name = 'Grove HTML '+root.name.rsplit('-', 1)[-1]
    contents = root/(app_name+'.app')/'Contents'
    launch = contents/'MacOS'/'grove'
    launch.parent.mkdir(parents=True)
    shutil.copy2(binary, launch)
    bundle = 'dev.grove.inlinehtml.'+root.name
    with (contents/'Info.plist').open('wb') as file:
        plistlib.dump({'CFBundleName': app_name, 'CFBundleIdentifier': bundle, 'CFBundleExecutable': 'grove', 'CFBundlePackageType': 'APPL', 'CFBundleVersion': '1', 'NSHighResolutionCapable': True}, file)
    log = (root/'desktop.log').open('w')
    owner = subprocess.Popen([str(launch)], env=env, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
    def cli(*words):
        result = subprocess.run([str(binary), *map(str, words)], env=env, capture_output=True, text=True, timeout=15)
        assert result.returncode == 0, (words, result.stdout, result.stderr)
        data = json.loads(result.stdout)
        assert data['ok'], data
        return data['data']
    for _ in range(200):
        assert owner.poll() is None, (root/'desktop.log').read_text()
        try: cli('sessions', 'list'); break
        except AssertionError: time.sleep(.1)
    cli('projects', 'add', '--name', 'Inline HTML fixture', '--path', project)
    session = cli('sessions', 'start', '--project', project, '--worktree', project, '--agent', 'codex', '--backend', 'native')
    session_id = session['id']
    for _ in range(200):
        row = next(row for row in cli('sessions', 'list') if row['id'] == session_id)
        if row.get('native_status', '').lower() == 'idle': break
        time.sleep(.1)
    prompt = root/'prompt.txt'
    prompt.write_text(args.scenario)
    cli('sessions', 'input', session_id, '--input-file', prompt)
    evidence = {'root': str(root), 'bundle_id': bundle, 'app': str(contents.parent), 'pid': owner.pid, 'session_id': session_id, 'config': str(config), 'binary': str(binary), 'project': str(project), 'scenario': args.scenario, 'prompt_file': str(prompt), 'input_command': [str(binary), 'sessions', 'input', session_id, '--input-file', str(prompt)]}
    (root/'fixture.json').write_text(json.dumps(evidence, indent=2)+'\n')
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(evidence, indent=2)+'\n')
    print(json.dumps(evidence, indent=2), flush=True)

if __name__ == '__main__': main()
