import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { history, event, invoke } from './history.mjs';
test('real classifier accepts only same-repository semantic release changes, independent of author', () => {
  const h = history();
  try {
    writeFileSync(join(h.dir,'Cargo.toml'),h.manifest.replace('version = "1.0.0"','version = "1.0.1"'));
    writeFileSync(join(h.dir,'Cargo.lock'),h.lock.replace('version = "1.0.0"','version = "1.0.1"'));
    writeFileSync(join(h.dir,'CHANGELOG.md'),'Released 1.0.1\n'); h.git('add','.');h.git('commit','-m','release');
    const payload = event(); payload.pull_request.user.login = 'human';
    assert.equal(invoke(h,payload).isReleaseOnly,true);
    assert.equal(invoke(h,event(true)).macosRequired,true);
    assert.equal(invoke(h,event(false,'feature/forged-release')).macosRequired,true);
    writeFileSync(join(h.dir,'Cargo.lock'),h.lock.replace('version = "1.0.0"','version = "1.0.1"').replace('version = "2.0.0"','version = "2.0.1"'));
    h.git('add','Cargo.lock');h.git('commit','-m','same-named registry dependency');
    assert.equal(invoke(h,event()).macosRequired,true);
  } finally { h.close(); }
});
test('bot-only, source, dependency and malformed classifications require coverage', () => {
  for (const change of ['empty','source','dependency']) {
    const h = history();
    try {
      if(change==='source') writeFileSync(join(h.dir,'src.rs'),'fn main() {}');
      if(change==='dependency') writeFileSync(join(h.dir,'Cargo.toml'),h.manifest.replace('llmlint = "2.0.0"','llmlint = "2.0.1"'));
      if(change!=='empty') {h.git('add','.');h.git('commit','-m',change);}
      assert.equal(invoke(h,event()).macosRequired,true);
      assert.equal(invoke(h,'{broken').macosRequired,true);
      const unknown = invoke(h,{},'future_event'); assert.equal(unknown.macosRequired,true); assert.match(unknown.reason,/future_event/);
      for(const name of ['push','release','workflow_run','repository_dispatch','schedule','workflow_dispatch']) assert.equal(invoke(h,'{broken',name).macosRequired,false);
    } finally {h.close();}
  }
});

test('classifier CLI refuses ambiguous arguments before any revision lookup',()=>{
  const h=history();
  try {
    for(const args of [[],['--event','a','--base','HEAD'],['--event','a','--base','HEAD','--base','HEAD'],['--event','a','--base','HEAD','--unexpected','HEAD'],['--event','a','--base','--bad','--head','HEAD']]) {
      const out=spawnSync('node',[fileURLToPath(new URL('../macos-policy.mjs',import.meta.url)),'classify',...args],{cwd:h.dir,encoding:'utf8'});assert.notEqual(out.status,0);assert.match(out.stderr,/classify|argument/);
    }
  } finally {h.close();}
});
test('malformed repository identity cannot authorize a release-only omission',()=>{
  const h=history();
  try {
    writeFileSync(join(h.dir,'CHANGELOG.md'),'Release notes');h.git('add','CHANGELOG.md');h.git('commit','-m','notes');
    for(const value of [true,1,{},'not-a-repository','']) {
      const payload=event();payload.pull_request.head.repo.full_name=value;payload.pull_request.base.repo.full_name=value;
      assert.equal(invoke(h,payload).macosRequired,true);
    }
  } finally {h.close();}
});
test('revision, version-file and TOML failures keep Mac coverage at the real CLI boundary',()=>{
  for(const failure of ['missing-base','missing-head','added-manifest','deleted-lock','malformed-toml']) {
    const h=history();
    try {
      writeFileSync(join(h.dir,'CHANGELOG.md'),'Release notes');h.git('add','CHANGELOG.md');h.git('commit','-m','notes');
      assert.equal(invoke(h,event()).isReleaseOnly,true);
      if(failure==='missing-base')h.base='unavailable-revision';
      if(failure==='added-manifest') {
        h.git('rm','Cargo.toml');h.git('commit','-m','baseline without manifest');h.base=h.git('rev-parse','HEAD');
        writeFileSync(join(h.dir,'Cargo.toml'),h.manifest);h.git('add','Cargo.toml');h.git('commit','-m','add manifest');
      }
      if(failure==='deleted-lock') {h.git('rm','Cargo.lock');h.git('commit','-m','delete lock');}
      if(failure==='malformed-toml') {writeFileSync(join(h.dir,'Cargo.toml'),'[package\ninvalid =');h.git('add','Cargo.toml');h.git('commit','-m','malformed manifest');}
      const decision=invoke(h,event(),'pull_request',failure==='missing-head'?'unavailable-revision':'HEAD');
      assert.equal(decision.macosRequired,true,failure);assert.equal(decision.isReleaseOnly,false,failure);assert.ok(decision.reason);assert.doesNotMatch(decision.reason,/\n/);
    } finally {h.close();}
  }
});
test('the automation identity is a same-repository release-plz head, never the PR author',()=>{
  const h=history();
  try {
    writeFileSync(join(h.dir,'Cargo.toml'),h.manifest.replace('version = "1.0.0"','version = "1.0.1"'));
    writeFileSync(join(h.dir,'Cargo.lock'),h.lock.replace('version = "1.0.0"','version = "1.0.1"'));
    writeFileSync(join(h.dir,'CHANGELOG.md'),'Released 1.0.1\n');h.git('add','.');h.git('commit','-m','release');
    for(const user of [{login:'ordinary-contributor'},undefined,null,42,{login:''}]) {
      const payload=event();
      if(user===undefined) delete payload.pull_request.user; else payload.pull_request.user=user;
      const out=invoke(h,payload);
      assert.equal(out.isReleaseOnly,true,JSON.stringify(user));
      assert.equal(out.macosRequired,false,JSON.stringify(user));
    }
    for(const edit of [p=>{p.head.repo=null;},p=>{delete p.head.repo;},p=>{delete p.head;},p=>{p.head.repo.full_name=42;},p=>{p.head.repo.full_name='other/llmlint';}]) {
      const payload=event();edit(payload.pull_request);
      const out=invoke(h,payload);
      assert.equal(out.isReleaseOnly,false,edit.toString());
      assert.equal(out.macosRequired,true,edit.toString());
    }
  } finally {h.close();}
});
