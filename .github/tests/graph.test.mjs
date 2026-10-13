import { test } from 'node:test';
import assert from 'node:assert/strict';
import { writeFileSync, mkdirSync } from 'node:fs';
import { join } from 'node:path';
import { graph, workflows, digest, root } from '../workflow-graph.mjs';
import { policyOutputs } from '../workflow-outputs.mjs';
import { history, event, invoke } from './history.mjs';
const contexts = ['cross (macos-latest)','cross (windows-latest)','deny','gate','install (macos-latest)','install (ubuntu-latest)','install (windows-latest)','llmlint','pr-title','visual-docs / report (arm64)','visual-docs / report (x86_64)'];
const classes = [
  ['pr-opened','pull_request','opened'],['pr-synchronize','pull_request','synchronize'],['pr-reopened','pull_request','reopened'],
  ['base-push','push'],['release-pr-merge','push'],['release-tag','push'],['release-published','release','published'],
  ['downstream','workflow_run','completed'],['repository-dispatch','repository_dispatch'],['schedule','schedule'],
  ['manual-default','workflow_dispatch'],['manual-opt-in','workflow_dispatch']
];
test('actual workflow call graph, matrices, event fan-out and required names record scheduling telemetry',()=>{
  const docs = workflows(); const scenarios = [];
  for(const diff of ['source','version-only','dependency']) {
    const h = history();
    try {
      let paths;
      if(diff==='source') { paths=['src/lib.rs']; writeFileSync(join(h.dir,'src.rs'),'pub fn changed() {}'); }
      if(diff==='version-only') { paths=['Cargo.toml','Cargo.lock','CHANGELOG.md']; writeFileSync(join(h.dir,'Cargo.toml'),h.manifest.replace('version = "1.0.0"','version = "1.0.1"'));writeFileSync(join(h.dir,'Cargo.lock'),h.lock.replace('version = "1.0.0"','version = "1.0.1"')); }
      if(diff==='dependency') { paths=['Cargo.toml']; writeFileSync(join(h.dir,'Cargo.toml'),h.manifest.replace('llmlint = "2.0.0"','llmlint = "2.0.1"')); }
      h.git('add','.');h.git('commit','-m',diff);
      for(const fork of [false,true]) for(const [id,eventName,action] of classes) for(const publishCrate of [false,true]) for(const publishPypi of [false,true]) {
        const payload = event(fork,diff==='source'?'feature/change':'release-plz-2026'); payload.action=action;
        payload.workflow_run={name:'Release',head_sha:'exact-head',conclusion:'success'};
        const decision = invoke(h,payload,eventName);
        const scenario = {id:`${id}/${diff}/${fork?'fork':'same'}/crate-${publishCrate}/pypi-${publishPypi}`,class:id,eventName,event:payload,paths,diff,fork,publishCrate,publishPypi,diagnostic:id==='manual-opt-in',tag:id==='release-tag'||id==='release-published'?'v1.0.1':undefined,...decision};
        scenario.base=h.base;scenario.head='HEAD';scenario.cwd=h.dir;scenario.policyOutputs=policyOutputs(docs['ci.yml'],scenario,h.dir);
        const rows = graph(scenario,docs); scenarios.push({...scenario,rows});
        if(eventName==='release') {
          for(const [job,active] of [['publish-crate',publishCrate],['verify-crate',publishCrate],['publish-pypi',publishPypi],['verify-pypi',publishPypi]]) assert.equal(rows.find(r=>r.workflow==='release.yml'&&r.job===job).result,active?'success':'skipped',`${scenario.id}: ${job}`);
        }
        if(eventName==='pull_request') {
          for(const ctx of contexts) assert.equal(rows.filter(r=>r.name===ctx).length,1,`${scenario.id}: missing/duplicate ${ctx}`);
          assert.equal(rows.find(r=>r.job==='macos').result,decision.macosRequired?'success':'skipped');
          for(const ctx of ['cross (macos-latest)','install (macos-latest)']) {
            const carrier = rows.find(r=>r.name===ctx);assert.equal(carrier.runner,'ubuntu-latest');assert.equal(carrier.result,'success');
            assert.deepEqual(carrier.needs,['policy','gate','macos']);
          }
        }
        if(eventName==='push'||['schedule','workflow_run','repository_dispatch'].includes(eventName)||id==='manual-default') assert.ok(rows.every(r=>!r.macos),scenario.id);
        for(const row of rows.filter(r=>r.macos)) assert.ok((row.workflow==='ci.yml'&&(eventName==='pull_request'||id==='manual-opt-in'))||(row.workflow==='release.yml'&&eventName==='release'),JSON.stringify(row));
      }
    } finally {h.close();}
  }
  // Follow the complete single-push chain, including downstream completions.
  const lifecycles = [];
  const derived = [];
  for(const fork of [false,true]) for(const publishCrate of [false,true]) for(const publishPypi of [false,true]) {
    const pick=(cls,diff,ownership=fork)=>scenarios.find(s=>s.class===cls&&s.diff===diff&&s.fork===ownership&&s.publishCrate===publishCrate&&s.publishPypi===publishPypi);
    const chain=[pick('pr-synchronize','source'),pick('base-push','source'),pick('pr-opened','version-only',false),pick('release-pr-merge','version-only',false),pick('release-tag','version-only',false),pick('release-published','version-only',false)];
    // Merge closures are derived fan-out, in addition to the twelve explicit event classes.
    const closures=[chain[0],chain[2]].map(s=>{
      const scenario={...s,id:`${s.id}/merged-closure`,event:{...s.event,action:'closed'},policyOutputs:undefined,cwd:undefined};
      return {...scenario,rows:graph(scenario,docs)};
    });
    const downstream=[];
    function completed(s, ancestors=[]) {
      for(const workflow of [...new Set(s.rows.map(r=>r.workflow))]) {
        if(ancestors.includes(workflow))throw new Error(`cyclic workflow_run fan-out: ${[...ancestors,workflow].join(' -> ')}`);
        const scenario={...s,id:`${s.id}/completed/${workflow}`,class:'downstream',eventName:'workflow_run',tag:undefined,policyOutputs:undefined,cwd:undefined,event:{workflow_run:{name:docs[workflow].name,head_sha:s.head??'exact-head',head_branch:s.tag??s.branch??'main',conclusion:'success'},action:'completed'}};
        const next={...scenario,rows:graph(scenario,docs)};
        downstream.push(next);
        if(next.rows.length)completed(next,[...ancestors,workflow]);
      }
    }
    [...chain,...closures].forEach(s=>completed(s));
    derived.push(...closures,...downstream);
    const runs=[...chain,...closures,...downstream].flatMap(s=>Object.keys(docs).map(workflow=>({event:s.id,workflow,jobs:s.rows.filter(r=>r.workflow===workflow&&r.macos)}))).filter(r=>r.jobs.length);
    lifecycles.push({fork,publishCrate,publishPypi,runs,macosRuns:runs.length});
  }
  const perRun=[...scenarios,...derived].flatMap(s=>Object.keys(docs).map(workflow=>({event:s.id,workflow,jobs:s.rows.filter(r=>r.workflow===workflow&&r.macos)})));
  const telemetry={schemaVersion:1,sourceDigest:digest(),scenarios,derived,lifecycles,perRun,jobsPerRun:Math.max(...perRun.map(r=>r.jobs.length)),runsPerChange:Math.max(...lifecycles.map(l=>l.macosRuns))};
  mkdirSync(new URL('.github/.telemetry/',root),{recursive:true});writeFileSync(new URL('.github/.telemetry/macos-policy.json',root),JSON.stringify(telemetry,null,2));
});
test('unsupported runners, reusable calls and matrix expressions fail explicitly',()=>{
  const s={id:'test',eventName:'pull_request',event:event(),paths:['src/lib.rs'],macosRequired:true};
  for(const job of [{'runs-on':'${{ mystery.runner }}'},{uses:'owner/repo/.github/workflows/unknown.yml@v1'},{'runs-on':'ubuntu-latest',strategy:{matrix:{os:'${{ needs.missing.outputs.matrix }}'}}}]) assert.throws(()=>graph(s,{'unknown.yml':{on:{pull_request:null},jobs:{unknown:job}}}),/unresolved|context|Unrecognized/i);
});
test('forged, mixed, unknown-input and ordinary fork PRs route through the actual Mac job',()=>{
  const h=history();
  try {
    writeFileSync(join(h.dir,'Cargo.toml'),h.manifest.replace('version = "1.0.0"','version = "1.0.1"'));
    writeFileSync(join(h.dir,'source.rs'),'fn new_behavior() {}');h.git('add','.');h.git('commit','-m','mixed release');
    for(const payload of [event(),event(true),event(false,'feature/forged'),null]) {
      const decision=invoke(h,payload);
      assert.equal(decision.macosRequired,true);
      const rows=graph({id:'untrusted-pr',eventName:'pull_request',event:payload??{action:'synchronize'},paths:['Cargo.toml','source.rs'],base:h.base,head:'HEAD',cwd:h.dir,...decision});
      assert.equal(rows.find(r=>r.job==='macos').result,'success');
    }
  } finally {h.close();}
});
test('release artifact transfers stay on the exact tagged run and wait for their producers',()=>{
  const docs=workflows(); const release=docs['release.yml'].jobs;
  const rows=graph({id:'release-contract',eventName:'release',event:{action:'published'},paths:[],tag:'v1.0.1',publish:true,macosRequired:false});
  const uploads=rows.flatMap(r=>r.artifacts.filter(a=>a.action.startsWith('actions/upload-artifact@')).map(a=>({row:r,artifact:a})));
  for(const row of rows) for(const a of row.artifacts.filter(a=>a.action.startsWith('actions/download-artifact@'))) {
    assert.equal(a['run-id'],undefined,'release artifacts must come from this exact tagged run');
    const candidates=uploads.filter(u=>u.artifact.name===a.name||(a.pattern==='wheels-*'&&u.artifact.name.startsWith('wheels-')));
    assert.ok(candidates.length,`no producer for ${a.name??a.pattern}`);
    for(const u of candidates) assert.ok(row.needs.includes(u.row.job),`${row.job} must need ${u.row.job}`);
  }
  const steps=release.darwin.steps;
  const build=steps.findIndex(s=>s.run==='bash scripts/release-darwin.sh dist');
  const attest=steps.findIndex(s=>s.uses?.startsWith('actions/attest-build-provenance@'));
  const upload=steps.findIndex(s=>s.uses?.startsWith('actions/upload-artifact@'));
  assert.ok(build>=0&&attest>build&&upload>attest);
  assert.equal(steps[attest].with['subject-path'],'dist/*.tar.gz');
  assert.ok(steps[upload].with.path.includes('dist/*.sha256'));
  assert.ok(steps[upload].with.path.includes('dist/*.sigstore.json'));
  for(const id of ['upload-darwin','publish-pypi','publish-crate']) assert.equal(release[id]['runs-on'],'ubuntu-latest');
  const darwin=rows.find(r=>r.job==='darwin');assert.deepEqual(darwin.targets,['x86_64-apple-darwin','aarch64-apple-darwin']);
});
test('matrix exclusions, include merging and filtered event routing cannot silently undercount',()=>{
  const scenario={id:'filters',eventName:'push',event:{},paths:['src/lib.rs'],branch:'main'};
  const doc={on:{push:{branches:['main'],paths:['src/**']}},jobs:{build:{'runs-on':'${{ matrix.os }}',strategy:{matrix:{os:['macos-latest','ubuntu-latest'],mode:['debug','release'],exclude:[{os:'macos-latest',mode:'debug'}],include:[{os:'macos-latest',mode:'release',target:'aarch64-apple-darwin'},{os:'windows-latest',mode:'release'}]}}}}};
  const rows=graph(scenario,{'filters.yml':doc});assert.equal(rows.length,4);assert.equal(rows.filter(r=>r.macos).length,1);assert.equal(rows.find(r=>r.macos).target,'aarch64-apple-darwin');
  for(const config of [{branches:['other']},{'branches-ignore':['main']},{paths:['docs/**']},{'paths-ignore':['src/**']},{tags:['v*']}]) assert.equal(graph({...scenario,tag:config.tags?'wrong-tag':undefined},{'filtered.yml':{...doc,on:{push:config}}}).length,0);
  assert.equal(graph({...scenario,tag:'v1.0.0'},{'tags.yml':{...doc,on:{push:{tags:['v*']}}}}).length,rows.length);
  assert.equal(graph({...scenario,eventName:'workflow_run',event:{action:'completed',workflow_run:{name:'wrong'}}},{'fanout.yml':{...doc,on:{workflow_run:{workflows:['Release'],types:['completed']}}}}).length,0);
});
test('unsupported triggers fail and dispatch/tag filters are honored',()=>{
  const job={jobs:{job:{'runs-on':'macos-latest'}}};
  const s={id:'filter-contract',eventName:'push',event:{},paths:['src/lib.rs'],tag:'v1.0.0'};
  assert.equal(graph(s,{'filtered.yml':{...job,on:{push:{'tags-ignore':['v*']}}}}).length,0);
  assert.equal(graph({...s,tag:undefined},{'filtered.yml':{...job,on:{push:{tags:['v*']}}}}).length,0);
  assert.equal(graph({...s,eventName:'repository_dispatch',event:{action:'wrong'}},{'filtered.yml':{...job,on:{repository_dispatch:{types:['smoke']}}}}).length,0);
  assert.throws(()=>graph(s,{'unknown.yml':{...job,on:{push:{unknown_filter:true}}}}),/unsupported trigger/);
});
test('merge closures use real default PR types and forwarded visual-docs cleanup only',()=>{
  const rows=graph({id:'merge-closure',eventName:'pull_request',event:{...event(),action:'closed'},paths:['src/lib.rs']});
  assert.ok(rows.every(r=>r.workflow!=='ci.yml'));
  assert.ok(rows.some(r=>r.workflow==='visual-docs.yml'&&r.job.endsWith('cleanup-preview')));
  assert.ok(rows.every(r=>!r.macos));
});
test('unsupported Actions globs refuse measurement instead of treating them as literals',()=>{
  const scenario={id:'unsupported-glob',eventName:'push',event:{},paths:['src/lib.rs']};
  for(const pattern of ['!src/**','src/?oo.rs','src/[ab].rs','**/file.rs','src/{a,b}.rs']) assert.throws(()=>graph(scenario,{'unsupported.yml':{on:{push:{paths:[pattern]}},jobs:{job:{'runs-on':'macos-latest'}}}}),/unsupported Actions glob/);
});
test('unmodeled workflow trigger declarations fail before scenario filtering',()=>{
  const scenario={id:'ordinary',eventName:'pull_request',event:event(),paths:['src/lib.rs']};
  for(const trigger of ['pull_request_target','merge_group'])assert.throws(()=>graph(scenario,{'unmodeled.yml':{on:{[trigger]:null},jobs:{mac:{'runs-on':'macos-latest'}}}}),/unsupported workflow trigger/);
});
