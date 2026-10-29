#!/usr/bin/env node
// Rust owns tokens, schema, recipes and generation. The wrapper only sequences
// explicit authoring, immutable baseline checks and a fresh candidate capture.
import { spawnSync } from 'node:child_process';
import { existsSync, rmSync, readFileSync } from 'node:fs';
import { resolve, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
const root=resolve(dirname(fileURLToPath(import.meta.url)),'..');
const [command,document,output]=process.argv.slice(2);
const commands=['export','init','validate','render','generate','check','author','capture','ready'];
if(!commands.includes(command)||!document){
  console.error('Usage: node scripts/ui-reference.mjs <export|init> <new-outdir> [theme]\n       node scripts/ui-reference.mjs <validate|render|generate|check|author|capture|ready> <document.json> <revision-dir>');process.exit(2);
}
const init=['export','init'].includes(command);
if(!init&&!output){console.error('output directory required');process.exit(2);}
const directory=resolve(init?document:output);
const env={...process.env,GROVE_REFERENCE_COMMAND:command,GROVE_REFERENCE_OUTPUT:directory,GROVE_REFERENCE_THEME:init?(output??'grove-dark'):'grove-dark'};
delete env.GROVE_REFERENCE_GENERATED;
if(!init)env.GROVE_REFERENCE_DOCUMENT=resolve(document);
function cargo(test,additional={}){
  const result=spawnSync('cargo',['test','-p','grove','--bin','grove',test,'--','--exact','--ignored','--nocapture'],{cwd:root,env:{...env,...additional},stdio:'inherit'});
  if(result.error)throw result.error;if(result.status!==0)throw new Error(`Rust reference gate failed (${result.status??'no exit status'})`);
}
function capture(author){
  if(process.platform!=='darwin')throw new Error(`Native reference capture is unsupported on ${process.platform}; no parity is implied`);
  if(author){
    if(existsSync(join(directory,'reference-lock.json')))throw new Error('Reference already frozen; author an explicit new revision directory');
    cargo('ui_reference::tests::tool',{GROVE_REFERENCE_COMMAND:'generate'});
  }else{
    cargo('ui_reference::tests::tool',{GROVE_REFERENCE_COMMAND:'check'});
  }
  for(const name of ['native.json','native.png','certificate.json'])rmSync(join(directory,name),{force:true});
  const source=join(directory,'generated.rs');
  const production=spawnSync('cargo',['check','-p','grove','--bin','grove'],{cwd:root,env:{...env,GROVE_REFERENCE_GENERATED:source},stdio:'inherit'});
  if(production.error)throw production.error;if(production.status!==0)throw new Error('Generated composition failed production compilation (cfg(test)=false)');
  cargo('ui_reference::tests::capture',{GROVE_REFERENCE_GENERATED:source,GROVE_REFERENCE_CAPTURE_MODE:author?'author':'candidate'});
  if(!existsSync(join(directory,'native.json'))||!existsSync(join(directory,'native.png')))throw new Error('Native capture produced no fresh evidence (zero tests is not success)');
  const record=JSON.parse(readFileSync(join(directory,'native.json'),'utf8'));
  if(record.schema_version!==1||!record.context?.native_build_hash||!Object.keys(record.nodes??{}).length)throw new Error('Native capture evidence is incomplete');
}
try{
  if(command==='init'&&existsSync(join(directory,'document.json')))throw new Error('Bootstrap already exists; use a new authoring directory');
  if(['author','capture','ready'].includes(command)){
    capture(command==='author');
    if(command==='ready'){
      const browser=spawnSync(process.execPath,[join(root,'scripts/verify-ui-reference.mjs'),directory],{cwd:root,env,stdio:'inherit'});
      if(browser.error)throw browser.error;if(browser.status!==0)throw new Error('Paired verification failed; reference remains uncertified');
      cargo('ui_reference::tests::tool',{GROVE_REFERENCE_COMMAND:'ready',GROVE_REFERENCE_GENERATED:join(directory,'generated.rs')});
    }
  }else{
    cargo('ui_reference::tests::tool');
    if(command==='render')console.log(join(directory,'reference.html'));
  }
}catch(error){console.error(error.message);process.exitCode=1;}
