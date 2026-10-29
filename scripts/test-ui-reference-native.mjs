#!/usr/bin/env node
// Explicit Metal runner gate. This does not claim desktop/provider-boundary QA.
import {spawnSync} from 'node:child_process';
import {mkdtempSync,readFileSync,writeFileSync,existsSync} from 'node:fs';
import {join,resolve} from 'node:path';
import {tmpdir} from 'node:os';
if(process.platform!=='darwin')throw new Error('Native reference acceptance requires macOS Metal');
if(!process.env.GROVE_CHROME_EXECUTABLE)throw new Error('Set GROVE_CHROME_EXECUTABLE to the pinned certified browser executable');
const output=resolve(process.argv[2]??mkdtempSync(join(tmpdir(),'grove-reference-native-')));
const resume=process.argv[3]==='--resume';
function run(...args){const r=spawnSync(process.execPath,['scripts/ui-reference.mjs',...args],{stdio:'inherit',env:process.env});if(r.error)throw r.error;if(r.status!==0)throw new Error(`Native gate failed: ${args.join(' ')}`);}
for(const [name,theme] of [['registered-row','grove-dark'],['distinct-actions','grove-light'],['catalog-theme','nord']]){
 const bootstrap=join(output,`${name}-source`),revision=join(output,name);
 const path=join(bootstrap,'document.json');
 const existing=existsSync(path);
 if(existing&&!resume)throw new Error('Existing evidence requires explicit --resume; frozen references will be checked, never rewritten');
 if(!existing)run('init',bootstrap,theme);
 if(!existing&&name==='distinct-actions'){
  const doc=JSON.parse(readFileSync(path,'utf8'));doc.fixture.rem=20;doc.fixture.viewport={width:320,height:240};
  const row=structuredClone(doc.root.children[0]);row.id='inspect-row';
  row.text='A separate application-owned action';
  row.children[0].icon=null;
  row.children[0].id='inspect-action';row.children[0].action='inspect';row.children[0].text_binding='inspect_label';
  doc.actions.push('inspect');doc.values.inspect_label='Inspect';doc.root.children.push(row);
  writeFileSync(path,JSON.stringify(doc,null,2));
 }
 if(!existing&&name==='catalog-theme'){
  const doc=JSON.parse(readFileSync(path,'utf8'));
  const wrap=(id,children)=>({id,kind:'surface',layout:structuredClone(doc.root.layout),state:'rest',text:null,text_role:null,color:null,icon:null,action:null,children});
  doc.root.children=[wrap('z-outer-surface',[wrap('a-inner-surface',doc.root.children)])];
  writeFileSync(path,JSON.stringify(doc,null,2));
 }
 if(!existsSync(join(revision,'reference-lock.json')))run('author',path,revision);
 run('check',path,revision);run('ready',path,revision);
 const cert=JSON.parse(readFileSync(join(revision,'certificate.json'),'utf8'));
 if(cert.status!=='certified')throw new Error('All gates must pass for both independent generated compositions');
}
console.log(`Native component acceptance evidence: ${output}`);
