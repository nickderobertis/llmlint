import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
const run = needs => spawnSync('node',[fileURLToPath(new URL('../macos-result.mjs',import.meta.url))],{encoding:'utf8',env:{...process.env,NEEDS_JSON:JSON.stringify(needs)}});
test('carrier drives real result command across success, cancellation, failure, and omission',()=>{
  const needs = {policy:{result:'success',outputs:{macosRequired:'true',reason:'ordinary PR'}},gate:{result:'success'},macos:{result:'success'}};
  assert.equal(run(needs).status,0);
  for(const result of ['failure','cancelled','skipped',undefined]) {
    assert.notEqual(run({...needs,macos:{result}}).status,0);
    assert.notEqual(run({...needs,gate:{result}}).status,0);
    assert.notEqual(run({...needs,policy:{...needs.policy,result}}).status,0);
  }
  const omitted = {...needs,policy:{result:'success',outputs:{macosRequired:'false',reason:'verified release-only diff'}},macos:{result:'skipped'}};
  const output = run(omitted); assert.equal(output.status,0);assert.match(output.stdout,/intentionally omitted/);assert.doesNotMatch(output.stdout,/assertions succeeded/);
  assert.notEqual(run({...omitted,macos:{result:'success'}}).status,0);
  assert.notEqual(run({...omitted,policy:{result:'success',outputs:{}}}).status,0);
});
