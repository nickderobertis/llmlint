// Repository-specific Actions graph proof. Unsupported routing is an error.
import { readFileSync, readdirSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { parse } from 'yaml';
import { policyOutputs, stepOutputs, archesOutputs } from './workflow-outputs.mjs';
import { Lexer, Parser, Evaluator, data } from '@actions/expressions';
export const root = new URL('../', import.meta.url);
const read = path => readFileSync(new URL(path, root), 'utf8');
export const workflows = () => Object.fromEntries(readdirSync(new URL('.github/workflows/', root)).filter(p => /\.ya?ml$/.test(p)).sort().map(p => [p, parse(read(`.github/workflows/${p}`))]));
export function digest() {
  const files = execFileSync('git', ['ls-files', '.github', 'screencomp.toml', 'release-targets.toml', 'pyproject.toml'], { cwd: root, encoding: 'utf8' }).trim().split('\n').filter(p => !p.startsWith('.github/.telemetry/'));
  return createHash('sha256').update(files.map(p => `${p}\0${read(p)}`).join('\0')).digest('hex');
}
function wrap(v) {
  if (v === null || v === undefined) return new data.Null();
  if (typeof v === 'string') return new data.StringData(v);
  if (typeof v === 'boolean') return new data.BooleanData(v);
  if (typeof v === 'number') return new data.NumberData(v);
  if (Array.isArray(v)) return new data.Array(...v.map(wrap));
  return new data.Dictionary(...Object.entries(v).map(([key, value]) => ({ key, value: wrap(value) })));
}
function unwrap(v) {
  if (v instanceof data.Array) return v.values().map(unwrap);
  if (v instanceof data.Dictionary) return Object.fromEntries(v.pairs().map(p => [p.key, unwrap(p.value)]));
  return v.value ?? null;
}
export function expression(source, context) {
  const text = source.trim().replace(/^\$\{\{\s*|\s*\}\}$/g, '');
  const ast = new Parser(new Lexer(text).lex().tokens, Object.keys(context), ['always','success','failure','cancelled'].map(name => ({ name, minArgs: 0, maxArgs: 0 }))).parse();
  const results = Object.values(context.needs ?? {}).map(n => n.result);
  const functions = new Map(Object.entries({ always: true, success: results.every(r => r === 'success'), failure: results.includes('failure'), cancelled: results.includes('cancelled') }).map(([name,value]) => [name, { name, minArgs: 0, maxArgs: 0, call: () => wrap(value) }]));
  return unwrap(new Evaluator(ast, wrap(context), functions).evaluate());
}
export function resolve(value, context) {
  if (typeof value !== 'string') return value;
  if (/^\$\{\{.*\}\}$/.test(value.trim())) return expression(value, context);
  return value.replace(/\$\{\{(.*?)\}\}/g, (_, e) => {
    const result = expression(e, context);
    if (result === null) throw new Error(`unresolved expression ${e}`);
    return String(result);
  });
}
export function expand(matrix, context) {
  if (!matrix) return [{}];
  matrix = typeof matrix === 'string' ? resolve(matrix, context) : matrix;
  if (!matrix || typeof matrix !== 'object') throw new Error('unresolved matrix');
  let rows = [{}];
  for (const [axis, raw] of Object.entries(matrix).filter(([k]) => !['include','exclude'].includes(k))) {
    const values = resolve(raw, context);
    if (!Array.isArray(values)) throw new Error(`unresolved matrix axis ${axis}`);
    rows = rows.flatMap(row => values.map(value => ({ ...row, [axis]: value })));
  }
  if (Object.keys(matrix).every(k => ['include','exclude'].includes(k))) rows = [];
  rows = rows.filter(row => !(matrix.exclude ?? []).some(ex => Object.entries(ex).every(([k,v]) => row[k] === v)));
  const originalRows = [...rows];
  for (const inc of matrix.include ?? []) {
    const axes = Object.keys(matrix).filter(k => !['include','exclude'].includes(k));
    const matches = originalRows.filter(row => axes.every(k => !(k in inc) || row[k] === inc[k]));
    if (matches.length) matches.forEach(row => Object.assign(row, inc)); else rows.push(inc);
  }
  return rows;
}
const glob = (pattern, value) => {
  if(typeof pattern !== 'string' || !pattern || /[!+?\[\]{}()\\]/.test(pattern) || (pattern.includes('**') && pattern !== '**' && !/^[^*]*\/\*\*$/.test(pattern))) throw new Error(`unsupported Actions glob ${pattern}`);
  return new RegExp(`^${pattern.split('**').map(s => s.split('*').map(p => p.replace(/[.+?^${}()|[\]\\]/g, '\\$&')).join('[^/]*')).join('.*')}$`).test(value);
};
export function triggered(doc, scenario) {
  const on = doc.on;
  const event = scenario.eventName;
  const events=typeof on==='string'?[on]:Array.isArray(on)?on:Object.keys(on??{});
  const supportedEvents=['push','pull_request','release','workflow_run','repository_dispatch','schedule','workflow_dispatch','workflow_call'];
  if(events.some(name=>!supportedEvents.includes(name))) throw new Error(`unsupported workflow trigger ${events.filter(name=>!supportedEvents.includes(name)).join(', ')}`);
  const defaultPr = () => event !== 'pull_request' || ['opened','synchronize','reopened'].includes(scenario.event.action);
  if (typeof on === 'string') return on === event && defaultPr();
  if (Array.isArray(on)) return on.includes(event) && defaultPr();
  if (!Object.hasOwn(on ?? {}, event)) return false;
  const config = on[event];
  const supported = { push: ['branches','branches-ignore','tags','tags-ignore','paths','paths-ignore'], pull_request: ['branches','branches-ignore','paths','paths-ignore','types'], release: ['types'], workflow_run: ['workflows','types','branches','branches-ignore'], repository_dispatch: ['types'], workflow_dispatch: ['inputs'], schedule: [] };
  if (config && !Array.isArray(config) && (typeof config !== 'object' || Object.keys(config).some(key => !supported[event]?.includes(key)))) throw new Error(`unsupported trigger configuration for ${event}`);
  if (Array.isArray(config)) {
    if (event !== 'schedule' || !config.length || config.some(entry => !entry || typeof entry !== 'object' || Array.isArray(entry) || Object.keys(entry).some(key=>key!=='cron') || typeof entry.cron !== 'string' || !entry.cron)) throw new Error(`invalid trigger configuration for ${event}`);
    return true;
  }
  if (!config) return defaultPr();
  for (const key of ['types','branches','branches-ignore','tags','tags-ignore','paths','paths-ignore','workflows']) if (Object.hasOwn(config,key) && (!Array.isArray(config[key]) || !config[key].length || config[key].some(value=>typeof value!=='string' || !value))) throw new Error(`invalid trigger ${key} for ${event}`);
  if (event === 'pull_request' && !config.types && !defaultPr()) return false;
  if (config.types && !config.types.includes(scenario.event.action)) return false;
  const branch = event === 'workflow_run' ? scenario.event.workflow_run?.head_branch ?? scenario.branch ?? 'main' : scenario.branch ?? 'main';
  if (event === 'push' && scenario.tag) return (!config.branches && !config['branches-ignore'] || !!config.tags || !!config['tags-ignore']) && (!config.tags || config.tags.some(p => glob(p, scenario.tag))) && !config['tags-ignore']?.some(p => glob(p, scenario.tag));
  if (event === 'push' && !scenario.tag && (config.tags || config['tags-ignore']) && !config.branches && !config['branches-ignore']) return false;
  if (config.branches && !config.branches.some(p => glob(p, branch))) return false;
  if (config['branches-ignore']?.some(p => glob(p, branch))) return false;
  if (config.paths && !scenario.paths.some(p => config.paths.some(g => glob(g,p)))) return false;
  if (config['paths-ignore'] && scenario.paths.every(p => config['paths-ignore'].some(g => glob(g,p)))) return false;
  if (event === 'workflow_run' && config.workflows && !config.workflows.includes(scenario.event.workflow_run?.name)) return false;
  return true;
}
const source = JSON.parse(read('.github/tests/reusable/screencomp-v0.4.2.source.json'));
const snapshot = read('.github/tests/reusable/screencomp-v0.4.2.yml').split('\n').slice(1).join('\n');
if (createHash('sha256').update(snapshot).digest('hex') !== source.sha256) throw new Error('reusable workflow differs from pinned upstream content; fetch the pinned URL and reconcile its source record');
const reusable = source.call;
export function graph(scenario, docs = workflows()) {
  const rows = [];
  function jobs(doc, workflow, prefix = '', inputs = {}, stack = []) {
    const outcomes = {};
    const pending = new Set(Object.keys(doc.jobs));
    while (pending.size) {
      let progressed = false;
      for (const id of pending) {
        const job = doc.jobs[id];
        const names = typeof job.needs === 'string' ? [job.needs] : job.needs ?? [];
        if (names.some(n => !doc.jobs[n])) throw new Error(`${workflow}/${id}: missing needs`);
        if (names.some(n => !outcomes[n])) continue;
        progressed = true; pending.delete(id);
        const needs = Object.fromEntries(names.map(n => [n,outcomes[n]]));
        const context = { github: { event_name: scenario.eventName, event: scenario.event, repository: 'nickderobertis/llmlint', ref: scenario.tag ? `refs/tags/${scenario.tag}` : 'refs/heads/main', ref_name: scenario.tag ?? 'main', sha: 'exact-head' }, needs, inputs, vars: { PYPI_PUBLISH: (scenario.publishPypi ?? scenario.publish) ? 'true' : 'false' }, matrix: {} };
        const condition = job.if ?? 'success()';
        const enabled = !!expression(condition, context) && (/\b(always|success|failure|cancelled)\(/.test(condition) || expression('success()', context));
        const result = enabled ? scenario.conclusions?.[workflow]?.[id] ?? 'success' : 'skipped';
        if(!['success','failure','cancelled','skipped'].includes(result)) throw new Error(`${workflow}/${id}: unknown GitHub conclusion ${result}`);
        let outputs = {};
        if (id === 'policy' && enabled) outputs = scenario.policyOutputs ?? policyOutputs(doc, scenario, scenario.cwd);
        if (id === 'guard' && enabled) {
          const step=job.steps.find(s => s.id === 'check');
          const binding=workflow==='release.yml'?'${{ secrets.CARGO_REGISTRY_TOKEN }}':workflow==='release-plz.yml'?'${{ secrets.RELEASE_PLZ_TOKEN }}':null;
          if(!binding || step?.env?.TOKEN !== binding) throw new Error('unsupported release guard secret binding');
          const present=workflow==='release-plz.yml'?(scenario.releaseAutomation??true):(scenario.publishCrate??scenario.publish);
          outputs=stepOutputs(step, { TOKEN: present ? 'fixture-secret' : '' }, root);
        }
        if (id === 'arches' && enabled) outputs = archesOutputs(job.steps.find(s => s.id === 'read'), inputs, root);
        if (enabled && ['policy','guard','arches'].includes(id)) {
          const stepId=id==='policy'?'classify':id==='guard'?'check':'read';
          const stepContext={...context,steps:{[stepId]:{outputs}}};
          outputs=Object.fromEntries(Object.entries(job.outputs??{}).map(([name,value])=>{
            const resolved=resolve(value,stepContext);
            if(typeof resolved!=='string'||!resolved)throw new Error(`${workflow}/${id}: unresolved job output ${name}`);
            return [name,resolved];
          }));
        }
        // Skipped arches has no output, but skipped dependants do not request runners.
        if (id === 'arches' && !enabled) outputs.matrix = '[]';
        outcomes[id] = { result, outputs };
        for (const matrix of expand(job.strategy?.matrix, { ...context, needs })) {
          const ctx = { ...context, matrix };
          if (job.uses) {
            if (stack.includes(job.uses)) throw new Error(`recursive workflow call ${job.uses}`);
            let called;
            if (job.uses === reusable) called = parse(read('.github/tests/reusable/screencomp-v0.4.2.yml'));
            else if (job.uses.startsWith('./.github/workflows/')) called = parse(read(job.uses.slice(2)));
            else throw new Error(`unresolved reusable workflow ${job.uses}`);
            if (enabled) {
              const declarations=called.on.workflow_call?.inputs ?? {};
              const supplied = Object.fromEntries(Object.entries(job.with ?? {}).map(([k,v]) => [k,resolve(v,ctx)]));
              for (const key of Object.keys(supplied)) if (!Object.hasOwn(declarations,key)) throw new Error(`unknown reusable input ${key}`);
              for (const [key,definition] of Object.entries(declarations)) {
                if (!['string','number','boolean'].includes(definition.type)) throw new Error(`unsupported reusable input type ${key}`);
                if (definition.required && !Object.hasOwn(supplied,key)) throw new Error(`missing required reusable input ${key}`);
                const value=supplied[key] ?? definition.default ?? ({string:'',number:0,boolean:false}[definition.type]);
                if (typeof value !== definition.type || (definition.type==='number' && !Number.isFinite(value))) throw new Error(`incorrect reusable input type ${key}`);
              }
              const defaults = Object.fromEntries(Object.entries(declarations).map(([k,v]) => [k,v.default ?? ({string:'',number:0,boolean:false}[v.type])]));
              jobs(called, workflow, `${prefix}${job.name ?? id} / `, {...defaults,...supplied}, [...stack,job.uses]);
            }
            continue;
          }
          const runner = resolve(job['runs-on'], ctx);
          if (typeof runner !== 'string' || !/^(ubuntu|windows|macos)-/.test(runner)) throw new Error(`${workflow}/${id}: unresolved runner ${JSON.stringify(runner)}`);
          const base = resolve(job.name ?? id, ctx);
          const name = job.name ? base : Object.keys(matrix).length ? `${base} (${Object.values(matrix).join(', ')})` : base;
          rows.push({ workflow, event: scenario.id, job: `${prefix}${id}`, name: `${prefix}${name}`, runner, target: matrix.target ?? matrix.arch ?? '', targets: (job.steps ?? []).flatMap(s => typeof s.with?.targets === 'string' ? s.with.targets.split(',') : []), result, macos: enabled && runner.startsWith('macos-'), needs: names, artifacts: (job.steps ?? []).filter(s => /actions\/(upload|download)-artifact@/.test(s.uses ?? '')).map(s => ({ action:s.uses, ...Object.fromEntries(Object.entries(s.with ?? {}).map(([k,v]) => [k,resolve(v,ctx)])) })) });
        }
      }
      if (!progressed) throw new Error(`${workflow}: cyclic/unresolved needs`);
    }
  }
  for (const [file,doc] of Object.entries(docs)) if (triggered(doc,scenario)) jobs(doc,file,'',{macos_diagnostic:scenario.diagnostic ?? false});
  return rows;
}
