#!/usr/bin/env python3
"""Disposable native desktop and CLI/Codex acceptance. Leaves fixture alive for QA."""
import argparse, hashlib, json, os, pathlib, plistlib, shlex, shutil, subprocess, sys, tempfile, time
p=argparse.ArgumentParser(); p.add_argument('binary',type=pathlib.Path); p.add_argument('--codex',action='store_true'); a=p.parse_args()
binary=a.binary.resolve(); root=pathlib.Path(tempfile.mkdtemp(prefix='grove-cli-acceptance-',dir='/tmp')).resolve(); project=root/'project'; project.mkdir(); config=root/'config'; config.mkdir()
project_name='Acceptance-'+root.name.rsplit('-',1)[-1]
env=dict(os.environ,GROVE_CONFIG_DIR=str(config),PATH=str(binary.parent)+os.pathsep+os.environ['PATH']); print(f'EVIDENCE={root}',flush=True)
def run(command,**kw):
 r=subprocess.run(command,text=True,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,**kw)
 with (root/'commands.log').open('a') as f:f.write(json.dumps(command)+'\n'+r.stdout+'\n')
 if r.returncode:raise RuntimeError(r.stdout)
 return r.stdout
def cli(*command):
 r=json.loads(run([str(binary),*command],env=env)); assert r['ok'],r; return r['data']
run(['git','init',str(project)]); run(['git','-C',str(project),'-c','user.name=Grove acceptance','-c','user.email=grove@example.invalid','commit','--allow-empty','-m','Acceptance baseline'])
(config/'projects.json').write_text(json.dumps({'projects':[],'tmux_enabled':False,'telemetry_enabled':False}))
launch_binary = binary
app_bundle = None
if sys.platform == 'darwin':
 app_bundle = root/'Grove Acceptance.app'
 contents = app_bundle/'Contents'; executable = contents/'MacOS'/'grove'
 executable.parent.mkdir(parents=True)
 shutil.copy2(binary, executable)
 with (contents/'Info.plist').open('wb') as info:
  plistlib.dump({'CFBundleName':'Grove Acceptance','CFBundleDisplayName':'Grove Acceptance','CFBundleIdentifier':'dev.grove.acceptance.'+root.name.rsplit('-',1)[-1], 'CFBundleExecutable':'grove','CFBundlePackageType':'APPL','CFBundleVersion':'1','CFBundleShortVersionString':'1.0','NSHighResolutionCapable':True},info)
 launch_binary = executable
f=(root/'desktop.log').open('w')
app=subprocess.Popen([str(launch_binary)],env=env,stdout=f,stderr=subprocess.STDOUT,start_new_session=True)
process = {'pid':app.pid,'config':str(config),'project':str(project),'project_name':project_name,'binary':str(binary),'source_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'launch_binary':str(launch_binary),'app_bundle':str(app_bundle) if app_bundle else None}
(root/'process.json').write_text(json.dumps(process,indent=2))
print('DESKTOP='+json.dumps(process),flush=True)
for _ in range(100):
 if app.poll() is not None:raise RuntimeError((root/'desktop.log').read_text())
 r=subprocess.run([str(binary),'projects','list'],env=env,capture_output=True,text=True)
 if r.returncode==0:break
 time.sleep(.1)
else:raise RuntimeError('Desktop not ready')
cli('projects','add','--name',project_name,'--path',str(project)); workspace=cli('workspaces','create','--name','Agents')['id']; cli('projects','move','--project',str(project),'--workspace',str(workspace)); cli('skills','install','--agent','codex','--project',str(project))
wt=cli('worktrees','create','--project',str(project),'--name','acceptance-worker')['path']; anchor=cli('sessions','start','--project',str(project),'--worktree',wt,'--agent','terminal','--backend','native'); shell=cli('sessions','shell','--project',str(project),'--worktree',wt)
proof_file=root/'literal-input-proof.txt'
f=root/'input.txt'
f.write_text("printf '%s\\n' GROVE_LITERAL_INPUT_OK > "+shlex.quote(str(proof_file))+"\r")
cli('sessions','input',shell['id'],'--input-file',str(f))
for _ in range(100):
 logs=cli('sessions','logs',shell['id'],'--lines','100')
 if proof_file.exists() and proof_file.read_text() == 'GROVE_LITERAL_INPUT_OK\n': break
 time.sleep(.1)
else: raise RuntimeError('Literal PTY input did not create exact proof in 10 seconds: '+json.dumps(logs))
(root/'literal-input-logs.json').write_text(json.dumps(logs,indent=2))
cli('sessions','focus',shell['id']); cli('sessions','stop',shell['id']); cli('sessions','stop',anchor['id']); cli('worktrees','remove','--project',str(project),'--worktree',wt,'--confirm',wt)
for _ in range(100):
 status=cli('worktrees','removal-status','--path',wt)
 if status['finished']:assert status['error'] is None,status; break
 time.sleep(.1)
else:raise RuntimeError('Removal not finished')
cli('projects','archive','--project',str(project)); cli('projects','restore','--project',str(project))
prompt=f'''Use $grove installed in this project. Desktop already running with GROVE_CONFIG_DIR={config}; grove binary {binary}. Operate ONLY disposable project {project}, config {config}, and worktrees returned by Grove for this unique project {project_name} under its default Grove worktrees root (normally ~/.config/grove/worktrees/{project_name}). Those returned test-owned worktrees are authorized for child launch, proof-file reads, and removal. Preserve all other project/worktree paths and existing user sessions. Read skill and CLI help. Using Grove CLI: create workspace Codex managed, rename Codex verified, move project to it, rename project Codex acceptance. Create worktree codex-child. Launch real Codex session with --backend native --prompt-file plus --task-title and --task-file. Write child prompt at {root}/child-prompt.txt: child writes grove-child-proof.txt containing PROMPT_RECEIVED and submits GROVE_TASK_ID completed via grove tasks complete with JSON result; no commits/push/network. Parent inspect child session/logs/task and wait bounded 120 seconds. Verify marker file and completed task. Before removal copy marker text into {root}/child-marker-evidence.txt and include exact child_task_id and child_session_id string fields in codex-proof.json. Stop only child, remove its worktree with exact confirmation and poll finished/errors. Move project back to workspace {workspace}; delete empty Codex verified with exact confirmation; archive/restore project and rename back {project_name}. Write {root}/codex-proof.json with commands, child task/session IDs, actual checks. Do not claim success without child marker/completed task. Never touch other Grove configs or user sessions.'''
(root/'codex-prompt.txt').write_text(prompt)
if a.codex:
 r=subprocess.run(['codex','exec','--approve-for-me','--add-dir',str(root),'-C',str(project),prompt],env=env,text=True,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,timeout=360); (root/'codex.log').write_text(r.stdout); print(r.stdout,flush=True); assert r.returncode==0,r.returncode; assert (root/'codex-proof.json').is_file(),'Codex proof missing'; proof=json.loads((root/'codex-proof.json').read_text()); (root/'final-projects.json').write_text(json.dumps(cli('projects','list'),indent=2)); (root/'final-tasks.json').write_text(json.dumps(cli('tasks','list'),indent=2)); tasks=cli('tasks','list'); child_id=proof['child_task_id']; child=next(task for task in tasks if task['id']==child_id); assert child['status']=='completed',child; assert child['session_id']==proof['child_session_id'],child; assert 'PROMPT_RECEIVED' in (root/'child-marker-evidence.txt').read_text(); assert any(row['name']==project_name and row['path']==str(project) and not row['archived'] and row['workspace']==workspace for row in cli('projects','list')); assert all(row['is_main'] for row in cli('worktrees','list','--project',str(project))); assert all(row['id']!=proof['child_session_id'] for row in cli('sessions','list'))
print(f'Fixture desktop PID {app.pid}; config {config}; project {project}; Codex prompt {root}/codex-prompt.txt',flush=True)
