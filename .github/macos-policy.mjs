import { execFileSync } from 'node:child_process';
import { readFileSync, realpathSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const git = (...args) => execFileSync('git', args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
const toml = text => JSON.parse(execFileSync('python3', ['-c', 'import sys,json,tomllib; print(json.dumps(tomllib.loads(sys.stdin.read())))'], { input: text, encoding: 'utf8' }));
function withoutVersion(path, text) {
  const value = toml(text);
  if (path === 'Cargo.toml') {
    if (typeof value.package?.version !== 'string') throw new Error('missing package version');
    delete value.package.version;
  } else if (path === 'Cargo.lock') {
    for (const pkg of value.package ?? []) {
      if (pkg.name === 'llmlint' && !pkg.source) {
        if (typeof pkg.version !== 'string') throw new Error('missing local package version');
        delete pkg.version;
      }
    }
  } else throw new Error(`unapproved version file: ${path}`);
  return JSON.stringify(value);
}
export function classify(event, base, head, eventName = process.env.GITHUB_EVENT_NAME ?? 'pull_request') {
  if (!['pull_request', 'push', 'release', 'workflow_run', 'repository_dispatch', 'schedule', 'workflow_dispatch'].includes(eventName)) return { isReleaseOnly: false, macosRequired: true, reason: `unrecognised event ${eventName} requires Mac coverage` };
  if (eventName !== 'pull_request') return { isReleaseOnly: false, macosRequired: false, reason: `automatic ${eventName} has no Mac lane` };
  try {
    const pr = event.pull_request;
    if (!pr || typeof pr.head?.ref !== 'string' || typeof pr.head.repo?.full_name !== 'string' || !/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(pr.head.repo.full_name) || typeof pr.base?.repo?.full_name !== 'string' || !/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(pr.base.repo.full_name)) throw new Error('unreadable PR');
    if (pr.head.repo.full_name !== pr.base.repo.full_name || !pr.head.ref.startsWith('release-plz-')) throw new Error('ordinary or fork PR');
    const paths = git('diff', '--name-only', '--no-renames', `${base}...${head}`).trim().split('\n').filter(Boolean);
    if (!paths.length) throw new Error('empty diff');
    for (const path of paths) {
      const mergeBase = git('merge-base', base, head).trim();
      const before = git('show', `${mergeBase}:${path}`);
      const after = git('show', `${head}:${path}`);
      if (path === 'CHANGELOG.md') continue;
      if (withoutVersion(path, before) !== withoutVersion(path, after)) throw new Error(`non-version content in ${path}`);
    }
    return { isReleaseOnly: true, macosRequired: false, reason: 'same-repository release-plz branch with verified version/changelog-only diff' };
  } catch (error) {
    return { isReleaseOnly: false, macosRequired: true, reason: String(error.message).replace(/\s+/g, ' ').trim() };
  }
}
if (process.argv[1] && realpathSync(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [command, ...args] = process.argv.slice(2);
  if (command !== 'classify') throw new Error('usage: classify --event PATH --base REF --head REF');
  const options = {};
  const allowed = ['--event', '--base', '--head'];
  if (args.length !== 6) throw new Error('classify requires exactly --event PATH --base REF --head REF');
  for (let i = 0; i < args.length; i += 2) {
    const key = args[i], value = args[i + 1];
    if (!allowed.includes(key) || Object.hasOwn(options, key) || !value || value.startsWith('-') || /[\x00-\x1f]/.test(value) || (key !== '--event' && !/^[A-Za-z0-9_][A-Za-z0-9_./-]*$/.test(value))) throw new Error(`invalid classify argument ${key}; use --event PATH --base REF --head REF`);
    options[key] = value;
  }
  if (allowed.some(key => !options[key])) throw new Error('missing classify argument; use --event PATH --base REF --head REF');
  let event;
  try { event = JSON.parse(readFileSync(options['--event'], 'utf8')); } catch { event = null; }
  console.log(JSON.stringify(classify(event, options['--base'], options['--head'])));
}
