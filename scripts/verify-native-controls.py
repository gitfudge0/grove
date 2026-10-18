#!/usr/bin/env python3
"""Launch an isolated native controls fixture; inspect/answer in its own app.

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
root = pathlib.Path(os.environ['GROVE_CONTROL_FIXTURE'])
claude = pathlib.Path(sys.argv[0]).name == 'claude'
if claude and '--input-format' not in sys.argv:
    print('[]', flush=True)
    raise SystemExit()
def send(value): print(json.dumps(value), flush=True)
thread, turn, scenario, seq = 'controls-fixture', '', '', 0
suggestion = {'type':'addRules','rules':[{'toolName':'Bash','ruleContent':'printf fixture-*'}],'behavior':'allow','destination':'session'}
schema = {'type':'object','properties':{
 'name':{'type':'string','title':'Display name','minLength':2,'maxLength':30,'default':'Fixture'},
 'email':{'type':'string','format':'email','default':'fixture@example.com'},
 'website':{'type':'string','format':'uri','default':'https://example.com'},
 'day':{'type':'string','format':'date','default':'2024-02-29'},
 'meeting':{'type':'string','format':'date-time','default':'2026-10-18T12:30:00Z'},
 'count':{'type':'integer','minimum':1,'maximum':5,'default':2},
 'ratio':{'type':'number','minimum':0,'maximum':1,'default':0.5},
 'enabled':{'type':'boolean','default':False},
 'color':{'type':'string','oneOf':[{'const':'r','title':'Red'},{'const':'b','title':'Blue'}],'default':'r'},
 'tags':{'type':'array','items':{'anyOf':[{'const':'a','title':'Alpha'},{'const':'b','title':'Beta'}]},'minItems':1,'maxItems':2,'default':['a']},
 'optional':{'type':'string','title':'Optional note'}},
 'required':['name','email','website','day','meeting','count','ratio','enabled','color','tags']}
def elicitation(which):
    p = {'mode':which,'message':'Disposable fixture: review before sending.','serverName':'Fixture MCP','mcp_server_name':'Fixture MCP','threadId':thread,'turnId':turn}
    if which == 'url': p.update(url='https://example.com/fixture',elicitationId='fixture-url')
    else: p['requestedSchema' if not claude else 'requested_schema']=schema
    return p

def ask():
    if scenario in ('form','url'):
        p=elicitation(scenario)
        if claude: p['subtype']='elicitation'
        method='mcpServer/elicitation/request'
    elif claude:
        p={'subtype':'can_use_tool','tool_name':'Bash','tool_use_id':'tool-'+str(seq),'input':{'command':'printf fixture-safe','description':'Disposable command'},'permission_suggestions':[suggestion],'decision_reason':'Review editable arguments and optional permission scope.'}
        method='unused'
    elif scenario == 'permissions':
        p={'threadId':thread,'turnId':turn,'itemId':'permission-item','reason':'Fixture needs read access to its disposable directory','permissions':{'fileSystem':{'read':[str(root)]}}}
        method='item/permissions/requestApproval'
    else:
        p={'threadId':thread,'turnId':turn,'itemId':'command-item','approvalId':'fixture-approval','reason':'Review this disposable command','cwd':str(root),'command':'printf fixture-safe','kind':'writeStdin' if scenario=='stdin' else 'command','proposedExecpolicyAmendment':['printf','fixture-safe'],'proposedNetworkPolicyAmendments':[{'host':'example.com','action':'allow'}]}
        method='item/commandExecution/requestApproval'
    if claude: send({'type':'control_request','request_id':'fixture-'+str(seq),'request':p})
    else: send({'id':'fixture-'+str(seq),'method':method,'params':p})

def verify(response):
    if scenario in ('form','url'):
        assert response['action'] in ('accept','decline','cancel'), response
        if response['action']=='accept' and scenario=='form':
            c=response['content']
            for k in schema['required']: assert k in c, (k,c)
            assert isinstance(c['count'],int) and not isinstance(c['count'],bool) and 1<=c['count']<=5,c
            assert isinstance(c['ratio'],(int,float)) and not isinstance(c['ratio'],bool) and 0<=c['ratio']<=1,c
            assert isinstance(c['enabled'],bool),c
            assert all(isinstance(c[k],str) for k in ['name','email','website','day','meeting','color']),c
            assert c['color'] in ('r','b') and isinstance(c['tags'],list) and 1<=len(c['tags'])<=2 and len(set(c['tags']))==len(c['tags']) and set(c['tags'])<={'a','b'},c
            assert c.get('optional') != '',c
        if response['action']=='accept' and scenario=='url': assert response.get('content') is None,response
    elif claude:
        assert response['behavior'] in ('allow','deny'),response
        if response['behavior']=='allow':
            assert isinstance(response['updatedInput'],dict),response
            if 'updatedPermissions' in response: assert response['updatedPermissions']==[suggestion],response
        else: assert isinstance(response['message'],str) and isinstance(response['interrupt'],bool),response
    elif scenario=='permissions':
        assert response['scope'] in ('turn','session'),response
        assert response['permissions'] in ({},{'fileSystem':{'read':[str(root)]}}),response
    else:
        d=response['decision']
        assert d in ('accept','acceptForSession','decline','cancel') if isinstance(d,str) else d in ({'acceptWithExecpolicyAmendment':{'execpolicy_amendment':['printf','fixture-safe']}},{'applyNetworkPolicyAmendment':{'network_policy_amendment':{'host':'example.com','action':'allow'}}}),response

for line in sys.stdin:
    request=json.loads(line)
    if claude:
        if request.get('type')=='control_request':
            kind=request['request']['subtype']
            data={'models':[{'value':'fixture-model','displayName':'Fixture model'}],'commands':[]} if kind in ('initialize','list_models') else {}
            send({'type':'control_response','response':{'subtype':'success','request_id':request['request_id'],'response':data}})
        elif request.get('type')=='user':
            seq+=1; turn='turn-'+str(seq)
            content=request['message']['content']
            prompt=content if isinstance(content,str) else next(x['text'] for x in content if x.get('type')=='text')
            scenario=next((s for s in ('form','url','approval','stdin','permissions') if s in prompt.lower()),'form')
            send({'type':'system','subtype':'init','session_id':thread,'model':'fixture-model','permissionMode':'default'})
            send(dict(request,session_id=thread))
            send({'type':'assistant','session_id':thread,'message':{'id':'intro-'+str(seq),'content':[{'type':'text','text':'Fixture scenario: '+scenario+'. Select, edit, decline, or cancel the controls below.'}]}})
            ask()
        elif request.get('type')=='control_response':
            (root/'responses.jsonl').open('a').write(json.dumps(request)+'\n')
            response=request.get('response',{}).get('response',{})
            verify(response)
            (root/'validated.jsonl').open('a').write(json.dumps({'agent':'claude','scenario':scenario,'response':response})+'\n')
            send({'type':'assistant','session_id':thread,'message':{'id':'result-'+str(seq),'content':[{'type':'text','text':'Response validated. Send form, url, or approval to start another scenario.'}]}})
            send({'type':'result','subtype':'success','session_id':thread,'is_error':False,'result':'Fixture response validated'})
    else:
        method=request.get('method'); ident=request.get('id')
        if method=='initialize': send({'id':ident,'result':{'userAgent':'fixture'}})
        elif method=='config/read': send({'id':ident,'result':{'config':{'sandbox_workspace_write':{'writable_roots':[]}}}})
        elif method in ('thread/start','thread/resume'): send({'id':ident,'result':{'thread':{'id':thread,'turns':[]}}})
        elif method=='model/list': send({'id':ident,'result':{'data':[{'id':'fixture-model','model':'fixture-model','displayName':'Fixture model','supportedReasoningEfforts':[],'defaultReasoningEffort':'medium','isDefault':True}],'nextCursor':None}})
        elif method=='skills/list': send({'id':ident,'result':{'data':[]}})
        elif method=='turn/start':
            seq+=1;turn='turn-'+str(seq);prompt=request['params']['input'][0]['text']
            scenario=next((s for s in ('form','url','approval','stdin','permissions') if s in prompt.lower()),'form')
            send({'id':ident,'result':{'turn':{'id':turn,'status':'inProgress'}}})
            send({'method':'turn/started','params':{'threadId':thread,'turn':{'id':turn}}})
            ask()
        elif method=='turn/interrupt': send({'id':ident,'result':{}})
        elif method is None and isinstance(ident,str) and ident.startswith('fixture-'):
            (root/'responses.jsonl').open('a').write(json.dumps(request)+'\n')
            verify(request['result'])
            (root/'validated.jsonl').open('a').write(json.dumps({'agent':'codex','scenario':scenario,'response':request['result']})+'\n')
            send({'method':'turn/completed','params':{'threadId':thread,'turn':{'id':turn,'status':'completed'}}})
        elif ident is not None: send({'id':ident,'result':{}})
'''

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True, type=Path)
    parser.add_argument('--agent', choices=['claude','codex'], default='claude')
    parser.add_argument('--scenario', choices=['form','url','approval','stdin','permissions'], default='form')
    args = parser.parse_args()
    binary = args.binary.resolve()
    root = Path(tempfile.mkdtemp(prefix='grove-native-controls-', dir='/tmp')).resolve()
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
    env = dict(os.environ, SHELL=str(shell), GROVE_CONFIG_DIR=str(config), GROVE_CONTROL_FIXTURE=str(root), PATH=str(fakebin)+os.pathsep+os.environ['PATH'])
    env.pop('GROVE_TEST_CONFIG_DIR',None)
    env.pop('GROVE_FORCE_LOGIN_PATH',None)
    (config/'projects.json').write_text(json.dumps({'projects':[],'tmux_enabled':False,'native_agents_enabled':True,'onboarded':True,'dangerously_skip_permissions_enabled':False,'telemetry_enabled':False}))
    app_name = 'Grove Controls ' + root.name.rsplit('-', 1)[-1]
    contents = root/(app_name + '.app')/'Contents'
    launch = contents/'MacOS'/'grove'
    launch.parent.mkdir(parents=True)
    shutil.copy2(binary,launch)
    bundle = 'dev.grove.controls.'+root.name
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
    cli('projects','add','--name','Agent controls','--path',project)
    session=cli('sessions','start','--project',project,'--worktree',project,'--agent',args.agent,'--backend','native')
    session_id=session['id']
    for attempt in range(200):
        row=next(row for row in cli('sessions','list') if row['id']==session_id)
        if row.get('native_status','').lower()=='idle': break
        time.sleep(.1)
    prompt=root/'prompt.txt'
    prompt.write_text(args.scenario)
    cli('sessions','input',session_id,'--input-file',prompt)
    evidence={'root':str(root),'bundle_id':bundle,'app':str(contents.parent),'pid':owner.pid,'session_id':session_id,'config':str(config),'binary':str(binary),'agent':args.agent,'scenario':args.scenario,'reopen_command':[str(binary),'sessions','input',session_id,'--input-file',str(prompt)]}
    (root/'fixture.json').write_text(json.dumps(evidence,indent=2))
    print(json.dumps(evidence,indent=2),flush=True)

if __name__=='__main__': main()
