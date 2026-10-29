#!/usr/bin/env node
// Deliberately render native defects against one immutable baseline, then restore.
import {spawnSync} from 'node:child_process';
import {cpSync,mkdirSync,readFileSync,writeFileSync,existsSync,rmSync} from 'node:fs';
import {resolve,join} from 'node:path';
import {createHash} from 'node:crypto';
const [referenceArg,outputArg]=process.argv.slice(2);
if(process.platform!=='darwin'||!referenceArg||!outputArg||!process.env.GROVE_CHROME_EXECUTABLE)throw new Error('Usage: pinned-browser env node scripts/test-ui-reference-native-mutations.mjs <frozen-reference> <new-evidence-dir> (macOS Metal)');
const reference=resolve(referenceArg),output=resolve(outputArg),candidate=join(output,'candidate');
if(existsSync(output))throw new Error('Use a new evidence directory');
mkdirSync(output,{recursive:true});cpSync(reference,candidate,{recursive:true});
const sha=bytes=>createHash('sha256').update(bytes).digest('hex');
const frozen=['document.json','snapshot.json','generated.rs','manifest.json','measured-layout.json','reference.html','reference-lock.json'];
const before=Object.fromEntries(frozen.map(name=>[name,sha(readFileSync(join(reference,name)))]));
function invoke(file,args,mutation){const env={...process.env};delete env.GROVE_REFERENCE_MUTATION;if(mutation)env.GROVE_REFERENCE_MUTATION=mutation;const r=spawnSync(process.execPath,[file,...args],{env,encoding:'utf8',maxBuffer:8*1024*1024});if(r.error)throw r.error;return r;}
function immutable(){for(const name of frozen)for(const dir of [reference,candidate])if(sha(readFileSync(join(dir,name)))!==before[name])throw new Error(`Mutation rewrote frozen reference: ${name}`);}
for(const mutation of ['inset','font','radius','icon','missing-text','flat-fill']){
 const dir=join(output,mutation);mkdirSync(dir);
 for(const name of ['native.png','native.json','certificate.json','browser.png','browser.json','geometry-diff.json','raster-diff.json','diff.png','overlay.png'])rmSync(join(candidate,name),{force:true});
 const capture=invoke('scripts/ui-reference.mjs',['capture',join(candidate,'document.json'),candidate],mutation);
 writeFileSync(join(dir,'capture.log'),capture.stdout+capture.stderr);
 if(!existsSync(join(candidate,'native.png')))throw new Error(`${mutation}: no actual native screenshot; compiler failure is not mutation evidence`);
 if(sha(readFileSync(join(candidate,'native.png')))===sha(readFileSync(join(reference,'native.png'))))throw new Error(`${mutation}: actual pixels did not change`);
 let rejected=capture.status!==0;
 if(!rejected){const comparison=invoke('scripts/verify-ui-reference.mjs',[candidate]);writeFileSync(join(dir,'verify.log'),comparison.stdout+comparison.stderr);rejected=comparison.status!==0;}
 if(!rejected)throw new Error(`${mutation}: deliberately defective native output accepted`);
 for(const name of ['native.png','native.json','certificate.json','geometry-diff.json','raster-diff.json','diff.png'])if(existsSync(join(candidate,name)))cpSync(join(candidate,name),join(dir,name));
 immutable();console.log(`Rejected actual native mutation: ${mutation}`);
}
const restored=invoke('scripts/ui-reference.mjs',['ready',join(candidate,'document.json'),candidate]);writeFileSync(join(output,'restored.log'),restored.stdout+restored.stderr);
if(restored.status!==0)throw new Error(`Restored native render failed; inspect ${join(output,'restored.log')}`);
const cert=JSON.parse(readFileSync(join(candidate,'certificate.json'),'utf8'));if(cert.status!=='certified')throw new Error('Restored candidate is not certified');immutable();
writeFileSync(join(output,'result.json'),JSON.stringify({schema_version:1,mutations:6,restored:'certified',frozen_artifacts:before},null,2));
console.log(`Restored valid native render certified; evidence ${output}`);
