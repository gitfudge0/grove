#!/usr/bin/env python3
"""Launch an isolated native questions fixture; inspect/answer in its own app.

No real provider credentials, user's app, or project are touched. The fixture
records provider responses and supports repeated prompt-driven question rounds.
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
root = pathlib.Path(os.environ['GROVE_QUESTION_FIXTURE'])
if '--input-format' not in sys.argv:
    print('[]', flush=True)
    raise SystemExit()
def send(value): print(json.dumps(value), flush=True)
turn = 0
for line in sys.stdin:
    request = json.loads(line)
    if request.get('type') == 'control_request':
        kind = request['request']['subtype']
        data = {'models': [{'value':'fixture-model','displayName':'Fixture model'}], 'commands':[]} if kind in ('initialize','list_models') else {}
        send({'type':'control_response','response':{'subtype':'success','request_id':request['request_id'],'response':data}})
    elif request.get('type') == 'user':
        turn += 1
        send({'type':'system','subtype':'init','session_id':'questions-fixture','model':'fixture-model','permissionMode':'default'})
        send(dict(request, session_id='questions-fixture'))
        send({'type':'assistant','session_id':'questions-fixture','message':{'id':'intro-'+str(turn),'content':[{'type':'text','text':'Choose how Grove should present context usage. You can answer all questions together.'}]}})
        questions = [
          {'header':'Usage meter','question':'Where should the context usage meter appear?','multiSelect':False,'options':[{'label':'Footer ring (Recommended)','description':'A compact ring beside the attachment button.'},{'label':'Context remaining','description':'Show a short text label in the footer.'},{'label':'Top edge bar','description':'A thin bar along the composer top edge.'}]},
          {'header':'Details','question':'Which context details should Grove show?','multiSelect':True,'options':[{'label':'Tokens used','description':'Include current token consumption.'},{'label':'Capacity','description':'Include the model context capacity.'},{'label':'Compaction trigger','description':'Explain automatic or manual compaction.'}]},
          {'header':'Custom wording','question':'What should the compacted-context divider say?','options':[]},
          {'header':'Secret','question':'Enter a disposable fixture secret.','isSecret':True,'options':[]}
        ]
        send({'type':'control_request','request_id':'fixture-questions-'+str(turn),'request':{'subtype':'can_use_tool','tool_name':'AskUserQuestion','tool_use_id':'ask-'+str(turn),'input':{'questions':questions}}})
    elif request.get('type') == 'control_response':
        with (root/'responses.jsonl').open('a') as file: file.write(json.dumps(request)+'\n')
        response = request.get('response',{}).get('response',{})
        if response.get('behavior') == 'allow':
            answers = response['updatedInput']['answers']
            assert set(answers) == {'Where should the context usage meter appear?','Which context details should Grove show?','What should the compacted-context divider say?','Enter a disposable fixture secret.'}, answers
            assert all(isinstance(value,str) and value.strip() for value in answers.values()), answers
        else:
            assert response.get('behavior') == 'deny', response
        send({'type':'assistant','session_id':'questions-fixture','message':{'id':'result-'+str(turn),'content':[{'type':'text','text':'Fixture response validated: '+response.get('behavior','unknown')+'. Send another message to reopen the question group.'}]}})
        send({'type':'result','subtype':'success','session_id':'questions-fixture','is_error':False,'result':'Fixture round complete'})
'''

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True, type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve()
    root = Path(tempfile.mkdtemp(prefix='grove-native-questions-', dir='/tmp')).resolve()
    root.chmod(0o700)
    config, project, fakebin = (root/name for name in ('config','project','bin'))
    for path in (config, project, fakebin): path.mkdir()
    subprocess.run(['git','init',str(project)],check=True,capture_output=True)
    subprocess.run(['git','-C',str(project),'-c','user.name=Fixture','-c','user.email=fixture@example.invalid','commit','--allow-empty','-m','Disposable fixture'],check=True,capture_output=True)
    fake = fakebin/'claude'
    fake.write_text('#!'+sys.executable+'\n'+FAKE)
    fake.chmod(0o755)
    shell = root/'shell'
    shell.write_text('#!/bin/sh\ncase "$1" in -ilc|-lic|-lc|-ic) shift; exec /bin/sh -c "$@";; -l) shift;; esac\nexec /bin/sh "$@"\n')
    shell.chmod(0o755)
    env = dict(os.environ, SHELL=str(shell), GROVE_CONFIG_DIR=str(config), GROVE_QUESTION_FIXTURE=str(root), PATH=str(fakebin)+os.pathsep+os.environ['PATH'])
    env.pop('GROVE_TEST_CONFIG_DIR',None)
    env.pop('GROVE_FORCE_LOGIN_PATH',None)
    (config/'projects.json').write_text(json.dumps({'projects':[],'tmux_enabled':False,'native_agents_enabled':True,'onboarded':True,'dangerously_skip_permissions_enabled':False,'telemetry_enabled':False}))
    app_name = 'Grove Questions ' + root.name.rsplit('-', 1)[-1]
    contents = root/(app_name + '.app')/'Contents'
    launch = contents/'MacOS'/'grove'
    launch.parent.mkdir(parents=True)
    shutil.copy2(binary,launch)
    bundle = 'dev.grove.questions.'+root.name
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
    cli('projects','add','--name','Question controls','--path',project)
    session=cli('sessions','start','--project',project,'--worktree',project,'--agent','claude','--backend','native')
    session_id=session['id']
    for attempt in range(200):
        row=next(row for row in cli('sessions','list') if row['id']==session_id)
        if row.get('native_status','').lower()=='idle': break
        time.sleep(.1)
    prompt=root/'prompt.txt'
    prompt.write_text('Show the structured fixture questions.')
    cli('sessions','input',session_id,'--input-file',prompt)
    evidence={'root':str(root),'bundle_id':bundle,'app':str(contents.parent),'pid':owner.pid,'session_id':session_id,'config':str(config),'binary':str(binary),'reopen_command':[str(binary),'sessions','input',session_id,'--input-file',str(prompt)]}
    (root/'fixture.json').write_text(json.dumps(evidence,indent=2))
    print(json.dumps(evidence,indent=2),flush=True)

if __name__=='__main__': main()
