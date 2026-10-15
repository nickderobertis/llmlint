import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { workflows, root } from '../workflow-graph.mjs';
const jobs=workflows()['ci.yml'].jobs;
const run = (needs,id) => {
  const step=jobs[id].steps.find(s=>s.run);
  assert.equal(step.env.NEEDS_JSON,'${{ toJSON(needs) }}');
  return spawnSync('bash',['-euo','pipefail','-c',step.run],{cwd:root,encoding:'utf8',env:{...process.env,NEEDS_JSON:JSON.stringify(needs)}});
};
for(const id of ['cross-macos','install-macos']) test(`${id} drives its production step across success, failure, cancellation and omission`,()=>{
  const needs = {policy:{result:'success',outputs:{macosRequired:'true',reason:'ordinary PR'}},gate:{result:'success'},macos:{result:'success'}};
  assert.equal(run(needs,id).status,0);
  for(const result of ['failure','cancelled','skipped',undefined]) {
    assert.notEqual(run({...needs,macos:{result}},id).status,0);
    assert.notEqual(run({...needs,gate:{result}},id).status,0);
    assert.notEqual(run({...needs,policy:{...needs.policy,result}},id).status,0);
  }
  const omitted = {...needs,policy:{result:'success',outputs:{macosRequired:'false',reason:'verified release-only diff'}},macos:{result:'skipped'}};
  const output = run(omitted,id); assert.equal(output.status,0);assert.match(output.stdout,/intentionally omitted/);assert.doesNotMatch(output.stdout,/assertions succeeded/);
  assert.notEqual(run({...omitted,macos:{result:'success'}},id).status,0);
  assert.notEqual(run({...omitted,policy:{result:'success',outputs:{}}},id).status,0);
});
