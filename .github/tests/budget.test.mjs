import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, readFileSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { spawnSync } from 'node:child_process';
import { graph, digest } from '../workflow-graph.mjs';
import { event } from './history.mjs';
const cli=fileURLToPath(new URL('../macos-budget.mjs',import.meta.url));
test('real budget reporter writes both graph-derived metrics and refuses invalid/stale telemetry',()=>{
  const dir=mkdtempSync(join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR??tmpdir(),'reporter-'));
  try {
    const perRun=[['ordinary','pull_request',event()],['release','release',{action:'published'}]].flatMap(([id,eventName,event])=>{
      const rows=graph({id,eventName,event,paths:['src/lib.rs'],tag:'v0.4.7',publish:true});
      return [...new Set(rows.map(r=>r.workflow))].map(workflow=>({workflow,event:id,jobs:rows.filter(r=>r.workflow===workflow&&r.macos)}));
    });
    const runs=perRun.filter(r=>r.jobs.length);
    const telemetry={schemaVersion:1,sourceDigest:digest(),perRun,lifecycles:[{runs,macosRuns:runs.length}],jobsPerRun:Math.max(...perRun.map(r=>r.jobs.length)),runsPerChange:runs.length};
    const path=join(dir,'telemetry.json'),result=join(dir,'result.json');
    const run=args=>spawnSync('node',[cli,...args],{encoding:'utf8',env:{...process.env,ONEBUDGETSPEC_RESULT:result}});
    const args=metric=>['--metric',metric,'--telemetry',path];
    writeFileSync(path,JSON.stringify(telemetry));
    for(const [metric,key] of [['jobs-per-run','jobsPerRun'],['runs-per-change','runsPerChange']]) {
      const output=run(args(metric));assert.equal(output.status,0,output.stderr);
      const report=JSON.parse(readFileSync(result,'utf8'));assert.equal(report.value,telemetry[key]);assert.match(report.detail,/macos-latest/);
    }
    for(const change of [{sourceDigest:'stale'},{schemaVersion:2},{perRun:[{workflow:'ci.yml',event:'pr',jobs:[{job:'mac',runner:'macos-latest',macos:true,targets:null}]}]},{lifecycles:[{runs,macosRuns:runs.length+1}]},{runsPerChange:runs.length+1},{jobsPerRun:-1},{jobsPerRun:null},{perRun:{}},{lifecycles:[]},{jobsPerRun:telemetry.jobsPerRun+1}]) {
      writeFileSync(path,JSON.stringify({...telemetry,...change}));assert.notEqual(run(args('jobs-per-run')).status,0);
    }
    for(const invalid of [[],['--metric','unknown','--telemetry',path],['--metric','jobs-per-run','--telemetry',join(dir,'missing')],['--telemetry',path,'--metric','jobs-per-run']]) assert.notEqual(run(invalid).status,0);
  } finally {rmSync(dir,{recursive:true,force:true});}
});
