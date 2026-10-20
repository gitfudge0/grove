#!/usr/bin/env python3
"""Launch an isolated native subagent fixture; inspect/answer in its own app.

No real provider credentials, user's app, or project are touched. The fixture
creates two permanently in-progress sessions, each with four child agents.
Use the work disclosure to inspect simple rows; two work, one fails, one completes.
Stop only the PID printed in fixture.json when inspection is complete.
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
root = pathlib.Path(os.environ['GROVE_SUBAGENT_FIXTURE'])
claude = pathlib.Path(sys.argv[0]).name == 'claude'
if claude and '--input-format' not in sys.argv:
    print('[]', flush=True)
    raise SystemExit()
def send(value): print(json.dumps(value), flush=True)
thread, turn = 'subagent-fixture', 'turn-fixture'
names = ['Explorer', 'Implementer', 'Reviewer', 'Researcher']
activities = ['Finding authentication paths', 'Updating session state', 'Checking error handling', 'Compared provider protocols']
ids = ['child-'+str(i) for i in range(4)]
def notify(method, **params): send({'method':method,'params':dict(threadId=thread, **params)})
def child_fixture():
    if claude:
        send({'type':'system','subtype':'init','session_id':thread,'model':'fixture-model','permissionMode':'default'})
        send({'type':'assistant','session_id':thread,'message':{'id':'spawn','content':[{'type':'tool_use','id':ident,'name':'Agent','input':{'name':name,'description':activity}} for ident,name,activity in zip(ids,names,activities)]}})
        for i in (0,1):
            send({'type':'assistant','parent_tool_use_id':ids[i],'session_id':'nested-'+ids[i],'message':{'id':'activity-'+ids[i],'content':[{'type':'text','text':activities[i]}]}})
        for i in (2,3):
            send({'type':'user','session_id':thread,'message':{'content':[{'type':'tool_result','tool_use_id':ids[i],'is_error':i==2,'content':activities[i]}]}})
    else:
        notify('turn/started', turn={'id':turn,'status':'inProgress'})
        for ident,activity in zip(ids,activities):
            item={'type':'collabAgentToolCall','id':'spawn-'+ident,'tool':'spawnAgent','senderThreadId':thread,'receiverThreadIds':[ident],'status':'completed','agentsStates':{ident:{'status':'running','message':activity}},'prompt':activity,'model':None,'reasoningEffort':None}
            notify('item/started', turnId=turn,item=dict(item,status='inProgress'))
            notify('item/completed',turnId=turn,item=item)
for line in sys.stdin:
    request=json.loads(line)
    (root/'provider-requests.jsonl').open('a').write(json.dumps(request)+'\n')
    if claude:
        if request.get('type')=='control_request':
            kind=request['request']['subtype']
            data={'models':[{'value':'fixture-model','displayName':'Fixture model'}],'commands':[]} if kind in ('initialize','list_models') else {}
            send({'type':'control_response','response':{'subtype':'success','request_id':request['request_id'],'response':data}})
        elif request.get('type')=='user': child_fixture()
    else:
        method=request.get('method'); ident=request.get('id')
        if method=='initialize': result={'userAgent':'fixture'}
        elif method=='config/read': result={'config':{'sandbox_workspace_write':{'writable_roots':[]}}}
        elif method in ('thread/start','thread/resume'): result={'thread':{'id':thread,'turns':[]}}
        elif method=='model/list': result={'data':[{'id':'fixture-model','model':'fixture-model','displayName':'Fixture model','supportedReasoningEfforts':[],'defaultReasoningEffort':'medium','isDefault':True}],'nextCursor':None}
        elif method=='skills/list': result={'data':[]}
        elif method=='thread/read':
            child=request['params']['threadId']; i=ids.index(child)
            result={'thread':{'id':child,'agentNickname':names[i],'turns':[{'id':'child-turn','status':'inProgress' if i<2 else 'failed' if i==2 else 'completed','items':[{'type':'agentMessage','id':'child-message','text':activities[i]}]}]}}
        elif method=='turn/start':
            send({'id':ident,'result':{'turn':{'id':turn,'status':'inProgress'}}})
            child_fixture()
            continue
        else: result={}
        if ident is not None: send({'id':ident,'result':result})
'''

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True, type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve()
    root = Path(tempfile.mkdtemp(prefix='grove-native-subagents-', dir='/tmp')).resolve()
    root.chmod(0o700)
    config, project, fakebin = (root/name for name in ('config','project','bin'))
    for path in (config, project, fakebin): path.mkdir()
    subprocess.run(['git','init',str(project)],check=True,capture_output=True)
    subprocess.run(['git','-C',str(project),'-c','user.name=Fixture','-c','user.email=fixture@example.invalid','commit','--allow-empty','-m','Disposable fixture'],check=True,capture_output=True)
    for agent in ('claude','codex'):
        fake = fakebin/agent
        fake.write_text('#!'+sys.executable+'\n'+FAKE)
        fake.chmod(0o755)
    shell = root/'shell'
    shell.write_text('#!/bin/sh\ncase "$1" in -ilc|-lic|-lc|-ic) shift; exec /bin/sh -c "$@";; -l) shift;; esac\nexec /bin/sh "$@"\n')
    shell.chmod(0o755)
    env = dict(os.environ, SHELL=str(shell), GROVE_CONFIG_DIR=str(config), GROVE_SUBAGENT_FIXTURE=str(root), PATH=str(fakebin)+os.pathsep+os.environ['PATH'])
    env.pop('GROVE_TEST_CONFIG_DIR',None)
    env.pop('GROVE_FORCE_LOGIN_PATH',None)
    (config/'projects.json').write_text(json.dumps({'projects':[],'tmux_enabled':False,'native_agents_enabled':True,'onboarded':True,'dangerously_skip_permissions_enabled':False,'telemetry_enabled':False}))
    app_name = 'Grove Subagents ' + root.name.rsplit('-', 1)[-1]
    contents = root/(app_name + '.app')/'Contents'
    launch = contents/'MacOS'/'grove'
    launch.parent.mkdir(parents=True)
    shutil.copy2(binary,launch)
    bundle = 'dev.grove.subagents.'+root.name
    with (contents/'Info.plist').open('wb') as file:
        plistlib.dump({'CFBundleName':app_name,'CFBundleIdentifier':bundle,'CFBundleExecutable':'grove','CFBundlePackageType':'APPL','CFBundleVersion':'1','NSHighResolutionCapable':True},file)
    log = (root/'desktop.log').open('w')
    owner = subprocess.Popen([str(launch)],env=env,stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
    def cli(*words):
        result = subprocess.run([str(binary),*map(str,words)],env=env,capture_output=True,text=True,timeout=15)
        assert result.returncode == 0,(words,result.stdout,result.stderr)
        data=json.loads(result.stdout)
        assert data['ok'],data
        return data['data']
    for attempt in range(200):
        assert owner.poll() is None,(root/'desktop.log').read_text()
        try:
            cli('sessions','list')
            break
        except AssertionError:
            time.sleep(.1)
    cli('projects','add','--name','Subagent progress','--path',project)
    sessions = {}
    prompt=root/'prompt.txt'
    prompt.write_text('Inspect four delegated tasks')
    for agent in ('claude', 'codex'):
        session=cli('sessions','start','--project',project,'--worktree',project,'--agent',agent,'--backend','native')
        session_id=session['id']
        for attempt in range(200):
            row=next(row for row in cli('sessions','list') if row['id']==session_id)
            if row.get('native_status','').lower()=='idle': break
            time.sleep(.1)
        else: raise RuntimeError(f'{agent} never became idle')
        cli('sessions','input',session_id,'--input-file',prompt)
        sessions[agent]=session_id
    evidence={'root':str(root),'bundle_id':bundle,'app':str(contents.parent),'pid':owner.pid,'session_ids':sessions,'config':str(config),'binary':str(binary),'expected_summary':'4 subagents · 2 working · 1 failed','inspection':['Open each session and expand its work header','Check name/activity/status rows and red failure','Collapse header; summary remains','Resize narrow; activity truncates before status','Tab to work header then Space/Enter to toggle']}
    (root/'fixture.json').write_text(json.dumps(evidence,indent=2))
    print(json.dumps(evidence,indent=2),flush=True)

if __name__=='__main__': main()
