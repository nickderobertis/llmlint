import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';

const git = (...args) => execFileSync('git', args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
const toml = text => JSON.parse(execFileSync('python3', ['-c', 'import sys,json,tomllib; print(json.dumps(tomllib.loads(sys.stdin.read())))'], { input: text, encoding: 'utf8' }));
function withoutVersion(path, text) {
  const value = toml(text);
  if (path === 'Cargo.toml') {
    if (typeof value.package?.version !== 'string') throw new Error('missing package version');
    delete value.package.version;
  } else if (path === 'Cargo.lock') {
    for (const pkg of value.package ?? []) if (pkg.name === 'llmlint' && !pkg.source) delete pkg.version;
  } else throw new Error(`unapproved version file: ${path}`);
  return JSON.stringify(value);
}
export function classify(event, base, head, eventName = process.env.GITHUB_EVENT_NAME ?? 'pull_request') {
  if (eventName !== 'pull_request') return { isReleaseOnly: false, macosRequired: false, reason: `automatic ${eventName} has no Mac lane` };
  try {
    const pr = event.pull_request;
    if (!pr || typeof pr.head?.ref !== 'string' || !pr.head.repo?.full_name || !pr.base?.repo?.full_name) throw new Error('unreadable PR');
    if (pr.head.repo.full_name !== pr.base.repo.full_name || !pr.head.ref.startsWith('release-plz-')) throw new Error('ordinary or fork PR');
    const paths = git('diff', '--name-only', '--no-renames', `${base}...${head}`).trim().split('\n').filter(Boolean);
    if (!paths.length) throw new Error('empty diff');
    for (const path of paths) {
      const before = git('show', `${base}:${path}`);
      const after = git('show', `${head}:${path}`);
      if (path === 'CHANGELOG.md') continue;
      if (withoutVersion(path, before) !== withoutVersion(path, after)) throw new Error(`non-version content in ${path}`);
    }
    return { isReleaseOnly: true, macosRequired: false, reason: 'same-repository release-plz branch with verified version/changelog-only diff' };
  } catch (error) {
    return { isReleaseOnly: false, macosRequired: true, reason: error.message };
  }
}
if (process.argv[1] === new URL(import.meta.url).pathname) {
  const [command, ...args] = process.argv.slice(2);
  if (command !== 'classify') throw new Error('usage: classify --event PATH --base REF --head REF');
  const options = Object.fromEntries(Array.from({ length: args.length / 2 }, (_, i) => [args[i * 2], args[i * 2 + 1]]));
  let event;
  try { event = JSON.parse(readFileSync(options['--event'], 'utf8')); } catch { event = null; }
  console.log(JSON.stringify(classify(event, options['--base'], options['--head'])));
}
