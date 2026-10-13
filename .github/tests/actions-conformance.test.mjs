import { test } from 'node:test';
import assert from 'node:assert/strict';
import { expand, graph } from '../workflow-graph.mjs';
// GitHub's published examples are the conformance oracle for this repository's
// supported subset, not synthetic budget counts. Unsupported syntax is refused.
const matrixSource='https://docs.github.com/en/actions/how-tos/write-workflows/choose-what-workflows-do/run-job-variations#expanding-or-adding-matrix-configurations';
test(`matrix include conforms to the published GitHub example: ${matrixSource}`,()=>{
  const matrix={fruit:['apple','pear'],animal:['cat','dog'],include:[{color:'green'},{color:'pink',animal:'cat'},{fruit:'apple',shape:'circle'},{fruit:'banana'},{fruit:'banana',animal:'cat'}]};
  assert.deepEqual(expand(matrix,{}),[
    {fruit:'apple',animal:'cat',color:'pink',shape:'circle'},
    {fruit:'apple',animal:'dog',color:'green',shape:'circle'},
    {fruit:'pear',animal:'cat',color:'pink'},
    {fruit:'pear',animal:'dog',color:'green'},
    {fruit:'banana'},{fruit:'banana',animal:'cat'}
  ]);
});
const eventsSource='https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#pull_request';
test(`default PR activities and conjunctive filters conform to GitHub: ${eventsSource}`,()=>{
  const job={jobs:{proof:{'runs-on':'ubuntu-latest'}}};
  for(const action of ['opened','synchronize','reopened']) assert.ok(graph({id:action,eventName:'pull_request',event:{action},paths:[]},{'default.yml':{...job,on:{pull_request:null}}}).some(r=>r.job==='proof'));
  assert.deepEqual(graph({id:'closed',eventName:'pull_request',event:{action:'closed'},paths:[]},{'default.yml':{...job,on:{pull_request:null}}}),[]);
  const doc={...job,on:{pull_request:{branches:['releases/**'],paths:['src/**']}}};
  for(const [branch,paths,runs] of [['releases/10',['src/main.rs'],true],['main',['src/main.rs'],false],['releases/10',['docs/main.md'],false]]) {
    const rows=graph({id:'filters',eventName:'pull_request',event:{action:'opened'},branch,paths},{'filters.yml':doc});
    assert.equal(rows.some(r=>r.job==='proof'),runs);
  }
});
const needsSource='https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax#jobsjob_idneeds';
test(`dependency skip propagation and always conform to GitHub: ${needsSource}`,()=>{
  const doc={on:{workflow_dispatch:null},jobs:{
    first:{'runs-on':'ubuntu-latest'},
    second:{needs:'first','runs-on':'ubuntu-latest'},
    third:{needs:'second','runs-on':'ubuntu-latest'},
    report:{needs:['first','second','third'],if:'always()','runs-on':'ubuntu-latest'},
    guarded:{needs:'first',if:"github.event_name == 'workflow_dispatch'",'runs-on':'ubuntu-latest'}
  }};
  for(const result of ['success','failure','cancelled','skipped']) {
    const rows=graph({id:result,eventName:'workflow_dispatch',event:{},paths:[],conclusions:{'needs.yml':{first:result}}},{'needs.yml':doc});
    const outcome=Object.fromEntries(rows.map(r=>[r.job,r.result]));
    assert.equal(outcome.first,result);
    assert.equal(outcome.second,result==='success'?'success':'skipped');
    assert.equal(outcome.third,result==='success'?'success':'skipped');
    assert.equal(outcome.guarded,result==='success'?'success':'skipped');
    assert.equal(outcome.report,'success');
  }
});
