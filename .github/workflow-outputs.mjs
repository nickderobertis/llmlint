// Execute the production output steps; only secrets and screencomp's CLI response are doubled.
import { execFileSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync, copyFileSync, existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
export function stepOutputs(step, env, cwd) {
  const dir = mkdtempSync(join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), 'outputs-'));
  const output = join(dir,'output');
  try {
    execFileSync('bash',['-euo','pipefail','-c',step.run],{cwd,env:{...Object.fromEntries(Object.entries(process.env).filter(([key])=>!key.startsWith('GIT_'))),...env,GITHUB_OUTPUT:output},stdio:['ignore','pipe','pipe']});
    return Object.fromEntries(readFileSync(output,'utf8').trim().split('\n').map(line=>{const i=line.indexOf('=');if(i<1)throw new Error(`invalid step output ${line}`);return [line.slice(0,i),line.slice(i+1)];}));
  } finally {rmSync(dir,{recursive:true,force:true});}
}
export function policyOutputs(doc, scenario, cwd = fileURLToPath(new URL('../',import.meta.url))) {
  const dir = mkdtempSync(join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), 'event-'));
  const event = join(dir,'event.json');
  try {
    writeFileSync(event,JSON.stringify(scenario.event));
    const github = join(cwd,'.github');
    if(!existsSync(github)) {
      mkdirSync(github);
      copyFileSync(fileURLToPath(new URL('./macos-policy.mjs',import.meta.url)),join(github,'macos-policy.mjs'));
    }
    const step=doc.jobs.policy.steps.find(s=>s.id==='classify');
    if(!step)throw new Error('CI policy lacks classify step');
    return stepOutputs(step,{GITHUB_EVENT_PATH:event,GITHUB_EVENT_NAME:scenario.eventName,BASE:scenario.base??'HEAD',HEAD:scenario.head??'HEAD',DIAGNOSTIC:String(scenario.diagnostic??false)},cwd);
  } finally {rmSync(dir,{recursive:true,force:true});}
}
export function archesOutputs(step, inputs, root) {
  const dir=mkdtempSync(join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR??tmpdir(),'screencomp-'));
  try {
    const arches=execFileSync('python3',['-c','import tomllib,json; print(json.dumps(tomllib.load(open("screencomp.toml","rb"))["capture"]["arches"]))'],{cwd:root,encoding:'utf8'}).trim();
    mkdirSync(join(dir,'bin'));
    writeFileSync(join(dir,'bin','screencomp'),`#!/usr/bin/env bash\n[ "$*" = 'arches --format json' ] || exit 1\nprintf '%s\\n' "$SCREENCOMP_ARCHES_FIXTURE"\n`,{mode:0o755});
    return stepOutputs(step,{ARCHES_OVERRIDE:inputs.arches??'',SCREENCOMP_ARCHES_FIXTURE:arches,PATH:`${join(dir,'bin')}:${process.env.PATH}`},root);
  } finally {rmSync(dir,{recursive:true,force:true});}
}
