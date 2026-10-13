import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, copyFileSync, readdirSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { workflows } from '../workflow-graph.mjs';
// Forge artifact downloads and the OIDC attestation action supply these bytes.
// This journey proves transport/handoff; release-targets:packaging proves package validity.
test('actual Darwin bundle staging and Linux release upload preserve every artifact byte',()=>{
  const dir=mkdtempSync(join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR??tmpdir(),'handoff-'));
  try {
    const jobs=workflows()['release.yml'].jobs;
    const stage=jobs.darwin.steps.find(s=>s.name==='Stage attestation bundle');
    const upload=jobs['upload-darwin'].steps.find(s=>s.run);
    const tag='v1.2.3';
    const targets=jobs.darwin.steps.find(s=>s.with?.targets).with.targets.split(',');
    const builder=join(dir,'builder'),linux=join(dir,'linux'),forge=join(dir,'forge'),bin=join(dir,'bin');
    for(const path of [join(builder,'dist'),join(linux,'dist'),forge,bin])mkdirSync(path,{recursive:true});
    const bundle=join(dir,'attestation.json');
    const subjects=targets.map(target=>({name:`llmlint-${tag}-${target}.tar.gz`,digest:{sha256:'fixture-action-digest'}}));
    writeFileSync(bundle,JSON.stringify({subjects,issuer:'fixture GitHub OIDC action response'}));
    for(const target of targets) for(const suffix of ['tar.gz','tar.gz.sha256']) writeFileSync(join(builder,'dist',`llmlint-${tag}-${target}.${suffix}`),`downloaded ${target} ${suffix}`);
    const env={...process.env,GITHUB_REF_NAME:tag,GITHUB_REPOSITORY:'fixture/llmlint',BUNDLE:bundle,FORGE:forge,PATH:`${bin}:${process.env.PATH}`};
    execFileSync('bash',['-euo','pipefail','-c',stage.run],{cwd:builder,env});
    for(const target of targets) assert.equal(readFileSync(join(builder,'dist',`llmlint-${tag}-${target}.sigstore.json`),'utf8'),readFileSync(bundle,'utf8'));
    for(const name of readdirSync(join(builder,'dist')))copyFileSync(join(builder,'dist',name),join(linux,'dist',name));
    writeFileSync(join(bin,'gh'),`#!/usr/bin/env python3
import os,pathlib,shutil,sys
args=sys.argv[1:]
assert args[:3]==['release','upload','v1.2.3'], args
assert args[-3:]==['--repo','fixture/llmlint','--clobber'], args
for path in args[3:-3]:
    assert pathlib.Path(path).is_file(), path
    shutil.copyfile(path,pathlib.Path(os.environ['FORGE'])/pathlib.Path(path).name)
`,{mode:0o755});
    execFileSync('bash',['-euo','pipefail','-c',upload.run],{cwd:linux,env});
    assert.deepEqual(readdirSync(forge).sort(),readdirSync(join(builder,'dist')).sort());
    for(const name of readdirSync(forge))assert.deepEqual(readFileSync(join(forge,name)),readFileSync(join(builder,'dist',name)));
    const refused=spawnSync('bash',['-euo','pipefail','-c',stage.run],{cwd:builder,env:{...env,BUNDLE:join(dir,'missing')},encoding:'utf8'});assert.notEqual(refused.status,0);
    rmSync(join(linux,'dist'),{recursive:true});mkdirSync(join(linux,'dist'));
    const missing=spawnSync('bash',['-euo','pipefail','-c',upload.run],{cwd:linux,env,encoding:'utf8'});assert.notEqual(missing.status,0);
  } finally {rmSync(dir,{recursive:true,force:true});}
});
