import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, readFileSync, existsSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { parse } from 'yaml';
import { archesOutputs } from '../workflow-outputs.mjs';
test('production architecture output step preserves shell metacharacters as data',()=>{
  const dir=mkdtempSync(join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR??tmpdir(),'arches-'));
  try {
    const sentinel=join(dir,'injected');
    const values=[`x86_64'; touch ${sentinel}; #`];
    writeFileSync(join(dir,'screencomp.toml'),`[capture]\narches = ${JSON.stringify(values)}\n`);
    const workflow=parse(readFileSync(new URL('./reusable/screencomp-v0.4.2.yml',import.meta.url),'utf8'));
    const output=archesOutputs(workflow.jobs.arches.steps.find(s=>s.id==='read'),{},dir);
    assert.deepEqual(JSON.parse(output.matrix),values);
    assert.equal(existsSync(sentinel),false);
  } finally {rmSync(dir,{recursive:true,force:true});}
});
test('workflow output adapters reject changed production environment bindings',async()=>{
  const { policyOutputs }=await import('../workflow-outputs.mjs');
  const { workflows, graph }=await import('../workflow-graph.mjs');
  const docs=workflows();
  const changed=structuredClone(docs['ci.yml']);
  changed.jobs.policy.steps.find(s=>s.id==='classify').env.BASE='HEAD';
  assert.throws(()=>policyOutputs(changed,{event:{},eventName:'push'}),/unsupported CI classifier environment/);
  const release=structuredClone(docs['release.yml']);
  release.jobs.guard.steps.find(s=>s.id==='check').env.TOKEN='';
  assert.throws(()=>graph({id:'bad-binding',eventName:'release',event:{action:'published'},paths:[]},{'release.yml':release}),/unsupported release guard secret binding/);
});
