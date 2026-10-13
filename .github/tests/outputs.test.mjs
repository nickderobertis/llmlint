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
