#!/usr/bin/env python3
"""Launch a disposable two-turn native Codex fixture for sent-message editing.

Inspect the printed app using native UI controls. Cancel edit first (no changes),
then edit the first message inline and press Send. Confirm Edit and restore:
the file and provider history must reflect the edited prompt, later replies must
be removed, and an unrelated composer draft must remain unchanged.
Use --fail-revert once to exercise provider rejection without restoring files.
Only stop the PID printed in fixture.json; no user project or credentials are used.
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

FAKE = r'''import json, os, pathlib, sys
root = pathlib.Path(os.environ['GROVE_EDIT_FIXTURE'])
state_path = root/'provider-state.json'
def state():
    return json.loads(state_path.read_text()) if state_path.exists() else {'id':'edit-fixture','turns':[], 'sequence':0}
def save(s): state_path.write_text(json.dumps(s,indent=2))
def send(value): print(json.dumps(value),flush=True)
def event(method,params): send({'method':method,'params':params})
for line in sys.stdin:
    request=json.loads(line)
    (root/'requests.jsonl').open('a').write(json.dumps(request)+'\n')
    method=request.get('method'); ident=request.get('id'); p=request.get('params',{})
    s=state(); thread=s['id']
    def reply(value): send({'id':ident,'result':value})
    if method=='initialize': reply({'userAgent':'edit-fixture'})
    elif method=='config/read': reply({'config':{'sandbox_workspace_write':{'writable_roots':[]}}})
    elif method in ('thread/start','thread/resume','thread/read'):
        reply({'thread':{'id':thread,'historyMode':'paginated','turns':s['turns']}})
    elif method=='thread/turns/list':
        turns=list(reversed(s['turns'])) if p.get('sortDirection')=='desc' else s['turns']
        offset=int(p.get('cursor') or 0); limit=p.get('limit',100)
        page=turns[offset:offset+limit]
        reply({'data':page,'nextCursor':str(offset+limit) if offset+limit<len(turns) else None})
    elif method=='model/list': reply({'data':[{'id':'fixture-model','model':'fixture-model','displayName':'Fixture model','supportedReasoningEfforts':[],'defaultReasoningEffort':'medium','isDefault':True}],'nextCursor':None})
    elif method=='skills/list': reply({'data':[]})
    elif method=='thread/revert':
        fail=root/'fail-next-revert'
        if fail.exists():
            fail.unlink()
            send({'id':ident,'error':{'code':-32000,'message':'Fixture rejected revert once; retry to continue.'}})
            continue
        assert p['threadId']==thread,p
        index=next(i for i,t in enumerate(s['turns']) if t['id']==p['beforeTurnId'])
        s['turns']=s['turns'][:index];save(s)
        reply({'thread':{'id':thread,'historyMode':'paginated','turns':s['turns']}})
    elif method=='turn/start':
        s['sequence']+=1; turn='turn-'+str(s['sequence'])
        prompt='\n'.join(x.get('text','') for x in p['input'] if x.get('type')=='text')
        user={'id':'user-'+turn,'type':'userMessage','content':[{'type':'text','text':prompt}]}
        answer={'id':'answer-'+turn,'type':'agentMessage','phase':'final_answer','text':'Updated note.txt for: '+prompt}
        # The start response lets Grove bind its pre-turn filesystem checkpoint.
        reply({'turn':{'id':turn,'status':'inProgress'}})
        event('turn/started',{'threadId':thread,'turn':{'id':turn,'status':'inProgress'}})
        event('item/completed',{'threadId':thread,'turnId':turn,'item':user})
        project=pathlib.Path(p.get('cwd') or os.environ['GROVE_EDIT_PROJECT'])
        (project/'note.txt').write_text('After '+turn+': '+prompt+'\n')
        (project/'turn-created.txt').write_text('Created by '+turn+'\n')
        s['turns'].append({'id':turn,'status':'completed','items':[user,answer]}); save(s)
        event('item/completed',{'threadId':thread,'turnId':turn,'item':answer})
        event('turn/completed',{'threadId':thread,'turn':{'id':turn,'status':'completed'}})
    elif ident is not None: reply({})
'''


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',required=True,type=Path)
    parser.add_argument('--fail-revert',action='store_true',help='Reject the first provider revert request')
    args=parser.parse_args()
    binary=args.binary.resolve(strict=True)
    root=Path(tempfile.mkdtemp(prefix='grove-native-edit-',dir='/tmp')).resolve()
    root.chmod(0o700)
    config,project,fakebin=(root/name for name in ('config','project','bin'))
    for path in (config,project,fakebin): path.mkdir()
    (project/'note.txt').write_text('Before either turn\n')
    subprocess.run(['git','init',str(project)],check=True,capture_output=True)
    subprocess.run(['git','-C',str(project),'add','note.txt'],check=True,capture_output=True)
    subprocess.run(['git','-C',str(project),'-c','user.name=Fixture','-c','user.email=fixture@example.invalid','commit','-m','Disposable fixture'],check=True,capture_output=True)
    fake=fakebin/'codex';fake.write_text('#!'+sys.executable+'\n'+FAKE);fake.chmod(0o755)
    if args.fail_revert: (root/'fail-next-revert').touch()
    shell=root/'shell'
    shell.write_text('#!/bin/sh\ncase "$1" in -ilc|-lic|-lc|-ic) shift; exec /bin/sh -c "$@";; -l) shift;; esac\nexec /bin/sh "$@"\n')
    shell.chmod(0o755)
    env=dict(os.environ,SHELL=str(shell),GROVE_CONFIG_DIR=str(config),GROVE_EDIT_FIXTURE=str(root),GROVE_EDIT_PROJECT=str(project),PATH=str(fakebin)+os.pathsep+os.environ['PATH'])
    for key in ('GROVE_TEST_CONFIG_DIR','GROVE_FORCE_LOGIN_PATH'): env.pop(key,None)
    (config/'projects.json').write_text(json.dumps({'projects':[],'tmux_enabled':False,'native_agents_enabled':True,'onboarded':True,'dangerously_skip_permissions_enabled':False,'telemetry_enabled':False}))
    app_name='Grove Edit '+root.name.rsplit('-',1)[-1]
    contents=root/(app_name+'.app')/'Contents'
    launch=contents/'MacOS'/'grove';launch.parent.mkdir(parents=True);shutil.copy2(binary,launch)
    bundle='dev.grove.edit.'+root.name
    with (contents/'Info.plist').open('wb') as file:
        plistlib.dump({'CFBundleName':app_name,'CFBundleIdentifier':bundle,'CFBundleExecutable':'grove','CFBundlePackageType':'APPL','CFBundleVersion':'1','NSHighResolutionCapable':True},file)
    log=(root/'desktop.log').open('w')
    owner=subprocess.Popen([str(launch)],env=env,stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
    evidence={'root':str(root),'bundle_id':bundle,'app':str(contents.parent),'pid':owner.pid,'config':str(config),'project':str(project),'note':str(project/'note.txt'),'created_file':str(project/'turn-created.txt'),'provider_state':str(root/'provider-state.json'),'request_log':str(root/'requests.jsonl'),'binary':str(binary),'fail_revert':args.fail_revert}
    (root/'fixture.json').write_text(json.dumps(evidence,indent=2))
    def cli(*words):
        result=subprocess.run([str(binary),*map(str,words)],env=env,capture_output=True,text=True,timeout=15)
        assert result.returncode==0,(words,result.stdout,result.stderr)
        data=json.loads(result.stdout);assert data['ok'],data
        return data['data']
    def wait_until(predicate,description):
        deadline=time.monotonic()+30
        while time.monotonic()<deadline:
            assert owner.poll() is None,(root/'desktop.log').read_text()
            try:
                if predicate(): return
            except (AssertionError,FileNotFoundError,json.JSONDecodeError): pass
            time.sleep(.1)
        raise RuntimeError('Timed out: '+description+'; fixture: '+str(root))
    wait_until(lambda: cli('sessions','list') is not None,'desktop IPC')
    cli('projects','add','--name','Sent message edit','--path',project)
    session=cli('sessions','start','--project',project,'--worktree',project,'--agent','codex','--backend','native')
    session_id=session['id'];evidence['session_id']=session_id
    def idle():
        return next(row for row in cli('sessions','list') if row['id']==session_id).get('native_status','').lower() in ('idle','done')
    wait_until(idle,'initial native idle')
    for index,prompt in enumerate(('First request: make the first change','Second request: make the second change'),1):
        path=root/('prompt-'+str(index)+'.txt');path.write_text(prompt)
        cli('sessions','input',session_id,'--input-file',path)
        wait_until(lambda: len(json.loads((root/'provider-state.json').read_text())['turns'])==index and idle(),'turn '+str(index))
    evidence['selectors']=['codex-edit-message-0','codex-inline-editor','codex-send-message-edit','codex-cancel-message-edit','codex-edit-confirmation','codex-confirm-edit','codex-cancel-edit']
    evidence['expected_after_edit']={'provider_turns':1,'composer':'unchanged unrelated draft','original_message_editor':'inline in transcript','confirmation':'inline in transcript','later_replies_removed':True}
    (root/'fixture.json').write_text(json.dumps(evidence,indent=2))
    print(json.dumps(evidence,indent=2),flush=True)

if __name__=='__main__': main()
