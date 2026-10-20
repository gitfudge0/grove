#!/usr/bin/env python3
"""Launch an isolated native Claude transcript fixture, with no network or auth.

Produces two turns with file diffs, search and completed tasks. The printed app
is left open for native inspection. Edit the first sent message to verify fork
identity and workspace checkpoints. Only stop the PID in fixture.json.
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

FAKE = r'''import json, os, pathlib, sys, uuid
root = pathlib.Path(os.environ['GROVE_EDIT_FIXTURE'])
state_path = root/'provider-state.json'
def load(): return json.loads(state_path.read_text()) if state_path.exists() else {'id':'claude-parity-fixture','turns':[], 'sequence':0}
def save(s): state_path.write_text(json.dumps(s,indent=2))
s=load()
if '--session-id' in sys.argv:
    s['id']=sys.argv[sys.argv.index('--session-id')+1]
    if '--resume-drops-turn' in sys.argv:
        drop=sys.argv[sys.argv.index('--resume-drops-turn')+1]
        index=next((i for i,t in enumerate(s['turns']) if t['id']==drop),len(s['turns']))
        s['turns']=s['turns'][:index]
    else: s['turns']=[]
    save(s)
(root/'launches.jsonl').open('a').write(json.dumps(sys.argv)+'\n')
def send(value):
    value.setdefault('session_id',s['id'])
    print(json.dumps(value),flush=True)
def assistant(content): send({'type':'assistant','uuid':str(uuid.uuid4()),'message':{'id':str(uuid.uuid4()),'content':content}})
def use(name,args,result,text):
    ident=str(uuid.uuid4())
    assistant([{'type':'tool_use','id':ident,'name':name,'input':args}])
    send({'type':'user','uuid':str(uuid.uuid4()),'message':{'content':[{'type':'tool_result','tool_use_id':ident,'content':text}]},'tool_use_result':result})
for line in sys.stdin:
    request=json.loads(line)
    (root/'requests.jsonl').open('a').write(json.dumps(request)+'\n')
    if request['type']=='control_request':
        subtype=request['request']['subtype']
        data={'session_state':'idle','current_permission_mode':'default','models':[{'value':'default','resolvedModel':'fixture-model','displayName':'Fixture model'}],'commands':[]} if subtype=='initialize' else {}
        send({'type':'control_response','response':{'subtype':'success','request_id':request['request_id'],'response':data}})
        if subtype=='initialize': send({'type':'system','subtype':'init','model':'fixture-model','permissionMode':'default','tools':['Write','Edit','WebSearch','TaskCreate','TaskUpdate']})
    elif request['type']=='user':
        s=load();s['sequence']+=1
        turn=request.get('uuid') or str(uuid.uuid4());prompt=request['message']['content']
        if not isinstance(prompt,str): prompt='\n'.join(v.get('text','') for v in prompt)
        send({'type':'system','subtype':'session_state_changed','state':'running'})
        send({'type':'user','uuid':turn,'message':{'content':prompt}})
        project=pathlib.Path(os.environ['GROVE_EDIT_PROJECT']);path=project/'note.txt'
        old=path.read_text();new='After '+turn+': '+prompt+'\n';path.write_text(new)
        use('Edit',{'file_path':str(path),'old_string':old,'new_string':new},{'filePath':str(path),'originalFile':old,'structuredPatch':[{'oldStart':1,'oldLines':len(old.splitlines()),'newStart':1,'newLines':1,'lines':['-'+x for x in old.splitlines()]+['+'+x for x in new.splitlines()]}]},'Updated note.txt')
        use('WebSearch',{'query':'Grove parity fixture documentation'},{'query':'Grove parity fixture documentation','results':[{'title':'Fixture source','url':'https://example.invalid/parity'}]},'Fixture source: https://example.invalid/parity')
        use('TaskCreate',{'subject':'Verify transcript parity'},{'task':{'id':str(s['sequence']),'subject':'Verify transcript parity'}},'Task created')
        use('TaskUpdate',{'taskId':str(s['sequence']),'status':'completed'},{'success':True,'taskId':str(s['sequence']),'statusChange':{'from':'pending','to':'completed'}},'Task completed')
        assistant([{'type':'text','text':'Updated note.txt and verified **file changes**, web search, and the completed checklist for: '+prompt}])
        s['turns'].append({'id':turn,'prompt':prompt});save(s)
        send({'type':'result','subtype':'success','is_error':False,'duration_ms':1,'result':'Done'})
        send({'type':'system','subtype':'session_state_changed','state':'idle'})
'''


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',required=True,type=Path)
    args=parser.parse_args()
    binary=args.binary.resolve(strict=True)
    root=Path(tempfile.mkdtemp(prefix='grove-native-claude-parity-',dir='/tmp')).resolve()
    root.chmod(0o700)
    config,project,fakebin=(root/name for name in ('config','project','bin'))
    for path in (config,project,fakebin): path.mkdir()
    (project/'note.txt').write_text('Before either turn\n')
    subprocess.run(['git','init',str(project)],check=True,capture_output=True)
    subprocess.run(['git','-C',str(project),'add','note.txt'],check=True,capture_output=True)
    subprocess.run(['git','-C',str(project),'-c','user.name=Fixture','-c','user.email=fixture@example.invalid','commit','-m','Disposable fixture'],check=True,capture_output=True)
    fake=fakebin/'claude';fake.write_text('#!'+sys.executable+'\n'+FAKE);fake.chmod(0o755)
    shell=root/'shell'
    shell.write_text('#!/bin/sh\ncase "$1" in -ilc|-lic|-lc|-ic) shift; exec /bin/sh -c "$@";; -l) shift;; esac\nexec /bin/sh "$@"\n')
    shell.chmod(0o755)
    env=dict(os.environ,SHELL=str(shell),GROVE_CONFIG_DIR=str(config),GROVE_EDIT_FIXTURE=str(root),GROVE_EDIT_PROJECT=str(project),PATH=str(fakebin)+os.pathsep+os.environ['PATH'])
    for key in ('GROVE_TEST_CONFIG_DIR','GROVE_FORCE_LOGIN_PATH'): env.pop(key,None)
    (config/'projects.json').write_text(json.dumps({'projects':[],'tmux_enabled':False,'native_agents_enabled':True,'onboarded':True,'dangerously_skip_permissions_enabled':False,'telemetry_enabled':False}))
    app_name='Grove Claude '+root.name.rsplit('-',1)[-1]
    contents=root/(app_name+'.app')/'Contents'
    launch=contents/'MacOS'/'grove';launch.parent.mkdir(parents=True);shutil.copy2(binary,launch)
    bundle='dev.grove.claude.parity.'+root.name
    with (contents/'Info.plist').open('wb') as file:
        plistlib.dump({'CFBundleName':app_name,'CFBundleIdentifier':bundle,'CFBundleExecutable':'grove','CFBundlePackageType':'APPL','CFBundleVersion':'1','NSHighResolutionCapable':True},file)
    log=(root/'desktop.log').open('w')
    owner=subprocess.Popen([str(launch)],env=env,stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
    evidence={'root':str(root),'bundle_id':bundle,'app':str(contents.parent),'pid':owner.pid,'config':str(config),'project':str(project),'note':str(project/'note.txt'),'created_file':str(project/'turn-created.txt'),'provider_state':str(root/'provider-state.json'),'request_log':str(root/'requests.jsonl'),'binary':str(binary)}
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
    cli('projects','add','--name','Claude transcript parity','--path',project)
    session=cli('sessions','start','--project',project,'--worktree',project,'--agent','claude','--backend','native')
    session_id=session['id'];evidence['session_id']=session_id
    def idle():
        return next(row for row in cli('sessions','list') if row['id']==session_id).get('native_status','').lower() in ('idle','done')
    wait_until(idle,'initial native idle')
    for index,prompt in enumerate(('First request: make the first change','Second request: make the second change'),1):
        path=root/('prompt-'+str(index)+'.txt');path.write_text(prompt)
        cli('sessions','input',session_id,'--input-file',path)
        wait_until(lambda: len(json.loads((root/'provider-state.json').read_text())['turns'])==index and idle(),'turn '+str(index))
    def transcript():
        for saved in (config/'codex-sessions').glob('*.json'):
            data=json.loads(saved.read_text())
            if data.get('provider')=='claude': return data
        raise AssertionError('Claude recovery transcript missing')
    wait_until(lambda: len([item for item in transcript()['items'] if item.get('type')=='plan']) >= 4, 'normalized recovery rows')
    saved=transcript();items=saved['items']
    assert len([item for item in items if item.get('type')=='fileChange'])==2
    assert len([item for item in items if item.get('type')=='webSearch'])==2
    assert any(item.get('plan') and all(step['status']=='completed' for step in item['plan']) for item in items)
    assert all(item['changes'][0]['diff'] for item in items if item.get('type')=='fileChange')
    assert all(item.get('arguments') and item.get('result') for item in items if item.get('type') in ('fileChange','webSearch','plan'))
    assert len(saved.get('edit_checkpoints',[]))>=2, 'Missing workspace edit checkpoints'
    evidence['normalized_rows_verified']=True
    evidence['recovery_dir']=str(config/'codex-sessions')
    evidence['selectors']=['codex-edit-message-0','codex-inline-editor','codex-send-message-edit','codex-cancel-message-edit','codex-edit-confirmation','codex-confirm-edit','codex-cancel-edit']
    evidence['expected_after_edit']={'provider_turns':1,'composer':'unchanged unrelated draft','original_message_editor':'inline in transcript','confirmation':'inline in transcript','later_replies_removed':True}
    (root/'fixture.json').write_text(json.dumps(evidence,indent=2))
    print(json.dumps(evidence,indent=2),flush=True)

if __name__=='__main__': main()
