import { fileURLToPath } from 'node:url';
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
const cli = fileURLToPath(new URL('../macos-policy.mjs', import.meta.url));
const gitEnv = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith('GIT_')));
export function history() {
  const dir = mkdtempSync(join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), 'policy-'));
  const git = (...args) => execFileSync('git', args, { cwd: dir, env: gitEnv, encoding: 'utf8', stdio: ['ignore','pipe','pipe'] }).trim();
  git('init'); git('config','user.email','test@example.com'); git('config','user.name','Fixture');
  const manifest = '[package]\nname = "llmlint"\nversion = "1.0.0"\n[dependencies]\nllmlint = "2.0.0"\n';
  const lock = 'version = 4\n[[package]]\nname = "llmlint"\nversion = "1.0.0"\n[[package]]\nname = "llmlint"\nversion = "2.0.0"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\n';
  writeFileSync(join(dir,'Cargo.toml'),manifest); writeFileSync(join(dir,'Cargo.lock'),lock); writeFileSync(join(dir,'CHANGELOG.md'),'Initial\n'); git('add','.');git('commit','-m','base'); const base = git('rev-parse','HEAD');
  return {dir,git,base,manifest,lock, close: () => rmSync(dir,{recursive:true,force:true})};
}
export function event(fork = false, branch = 'release-plz-2026') {
  return { action: 'synchronize', pull_request: { title: 'chore: release', user: {login:'release-plz[bot]'}, head: {ref:branch, repo:{full_name:fork?'fork/llmlint':'nickderobertis/llmlint'}}, base:{repo:{full_name:'nickderobertis/llmlint'}} } };
}
export function invoke(h, payload, eventName = 'pull_request', head = 'HEAD') {
  const path = join(h.dir,'event.json'); writeFileSync(path,typeof payload === 'string' ? payload : JSON.stringify(payload));
  const run = spawnSync('node',[cli,'classify','--event',path,'--base',h.base,'--head',head],{cwd:h.dir,encoding:'utf8',env:{...gitEnv,GITHUB_EVENT_NAME:eventName}});
  assert.equal(run.status,0,run.stderr); return JSON.parse(run.stdout);
}
