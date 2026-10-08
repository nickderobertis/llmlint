// What the gate tiers reach, read from the project graph Nx itself resolves
// (`nx graph`), not from any one project.json: every shell script is under some
// project's shfmt and shellcheck, the workflows are under actionlint, the shell
// coverage gate merges every shell-measured project, and `just check` runs the
// targets those checks hang off. Dropping one of them would silently
// un-gate a whole class of files while every remaining target stayed green.
// llmlint: ignore-file[shell_test_tiers_stay_split] these tests are offline and hermetic: they run the boundary checker and Nx's own graph resolution (the gate's orchestrator, installed from bun.lock) and cargo metadata --offline over scratch or the real workspace; nothing reaches a network, and the files they read are this project's inputs, so a separate project would be selected by exactly the same edits
import { beforeAll, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const REPO = resolve(import.meta.dir, "../..");

function run(cmd, args, opts = {}) {
  const r = spawnSync(cmd, args, { encoding: "utf8", cwd: REPO, ...opts });
  if (r.error) throw r.error;
  return { code: r.status, stdout: r.stdout ?? "", stderr: r.stderr ?? "" };
}

let nodes;
beforeAll(() => {
  const dir = mkdtempSync(join(tmpdir(), "llmlint-graph-"));
  try {
    const file = join(dir, "graph.json");
    const r = run("bash", ["scripts/nx", "graph", `--file=${file}`]);
    if (r.code !== 0) throw new Error(`nx graph failed: ${r.stderr}`);
    nodes = JSON.parse(readFileSync(file, "utf8")).graph.nodes;
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

const targetsOf = (project) => nodes[project]?.data?.targets ?? {};
const commandOf = (target) => target?.options?.command ?? "";

const commandsOf = (target) =>
  [target?.options?.command, ...(target?.options?.commands ?? [])].filter((c) => typeof c === "string");

// The files a target hands tools/shell/shell.sh <step>, its globs expanded the way
// the target's own shell expands them.
function shellFilesOf(target, step) {
  const files = [];
  for (const c of commandsOf(target)) {
    const words = c.split(/\s+/);
    if (words[0] !== "bash" || words[1] !== "tools/shell/shell.sh" || words[2] !== step) continue;
    const globs = words.slice(3).filter((w) => !w.startsWith("-"));
    const expanded = run("bash", ["-c", `shopt -s nullglob; for f in ${globs.join(" ")}; do printf '%s\\n' "$f"; done`]);
    files.push(...expanded.stdout.split("\n").filter(Boolean));
  }
  return files;
}

// The tree's shell scripts. A scan that fails part-way has printed the scripts
// before the failure, so its exit status is checked rather than its list trusted.
function shellScripts() {
  const r = run("bash", ["tools/shell/shell.sh", "files"]);
  expect(r.code, r.stderr).toBe(0);
  return r.stdout.split("\n").filter(Boolean);
}

test("every shell script in the tree is under some project's shfmt format and shellcheck lint", () => {
  const formatted = new Set();
  const linted = new Set();
  for (const node of Object.values(nodes)) {
    const targets = node.data.targets ?? {};
    for (const f of shellFilesOf(targets.format, "format")) formatted.add(f);
    for (const f of shellFilesOf(targets.lint, "lint")) linted.add(f);
  }
  // The tree scan, not a list: every .sh/.bash file and every sh-family shebang.
  const scripts = shellScripts();
  expect(scripts).toContain(".githooks/pre-push");
  expect(scripts).toContain("scripts/nx");
  expect(scripts.length).toBeGreaterThan(20);
  expect(scripts.filter((f) => !formatted.has(f)), "shell scripts no project's format checks with shfmt").toEqual([]);
  expect(scripts.filter((f) => !linted.has(f)), "shell scripts no project's lint checks with shellcheck").toEqual([]);
});

test("a sourced shell library sits in the project of every script that sources it, or that project names it", () => {
  // shellcheck's `source=` directive is how each script declares what it sources
  // (shellcheck fails a `.`/`source` it cannot follow), so it is the edge list.
  const roots = Object.entries(nodes).map(([name, node]) => [name, node.data.root === "." ? "" : `${node.data.root}/`]);
  const owner = (file) =>
    roots.filter(([, root]) => file.startsWith(root)).sort((a, b) => b[1].length - a[1].length)[0][0];
  const scripts = shellScripts();
  const edges = [];
  for (const f of scripts) {
    for (const m of readFileSync(join(REPO, f), "utf8").matchAll(/^\s*# shellcheck source=(\S+)/gm)) {
      if (m[1] !== "/dev/null") edges.push([f, m[1]]);
    }
  }
  expect(edges.map(([, lib]) => lib)).toContain("scripts/setup-lib.sh");
  expect(edges.map(([, lib]) => lib)).toContain("tests/live/live-lib.sh");
  const stray = edges.filter(([f, lib]) => {
    const [from, to] = [owner(f), owner(lib)];
    return from !== to && !(nodes[from].data.implicitDependencies ?? []).includes(to);
  });
  expect(stray, "a script sourcing a library of another project that it does not name").toEqual([]);
});

test("the shell coverage gate merges exactly the coverage:shell projects, each recording its own test run", () => {
  const tagged = Object.entries(nodes)
    .filter(([, node]) => (node.data.tags ?? []).includes("coverage:shell"))
    .map(([name]) => name)
    .sort();
  expect(tagged).toContain("repo-tooling");
  const gate = targetsOf("shell-coverage").coverage;
  const words = commandOf(gate).split(/\s+/);
  expect(words.slice(0, 3)).toEqual(["bash", "tools/coverage/shcov.sh", "report"]);
  expect(words.slice(3).sort(), "the projects shell-coverage:coverage merges").toEqual(tagged);
  expect(gate.dependsOn).toEqual([{ projects: ["tag:coverage:shell"], target: "test" }]);
  for (const name of tagged) {
    const t = targetsOf(name).test;
    expect(commandOf(t).startsWith(`bash tools/coverage/shcov.sh run ${name} -- `), `${name}:test`).toBe(true);
    expect(t.outputs, `${name}:test outputs`).toEqual([`{workspaceRoot}/target/shcov/${name}.json`]);
  }
});

test("the workflows are linted by actionlint, ci-workflows' lint-workflows", () => {
  expect(commandOf(targetsOf("ci-workflows")["lint-workflows"])).toBe("bash scripts/lint-workflows.sh");
});

test("the check recipe runs every gate target, the workflow lint and both coverage gates included, at either tier", () => {
  const justfile = readFileSync(join(REPO, "justfile"), "utf8");
  const recipe = justfile.slice(justfile.indexOf("\ncheck *flags:") + 1);
  const body = recipe.slice(0, recipe.indexOf("\n\n"));
  const targets = "-t format lint lint-workflows build test doc coverage";
  expect(body).toContain(`bash scripts/nx run-many --all ${targets};`);
  expect(body).toContain(`bash scripts/nx affected --base="$tier" ${targets};`);
});

// scripts/nx is the one place the Nx environment is set, plugin loading without
// the 10-second worker window among it; a graph computed from inside another Nx
// task that bypassed it would start plugin workers that a cold Windows runner
// lets time out. So every Nx this repo starts from a task's code or command
// line goes through scripts/nx — and the known nested callers do.
test("every Nx started from inside a task goes through scripts/nx", () => {
  const spawn = /\b(?:spawnSync|spawn|execFileSync|execFile|execSync|exec|run)\(/;
  const direct = /["'`](?:[^"'`\s]*\/)?(?:nx|bunx|npx)["'`]|\b(?:bunx|npx|bun x)\s+nx\b/;
  const viaWrapper = /["'`]scripts\/nx["'`]/;
  const files = run("git", ["ls-files", "--cached", "--others", "--exclude-standard", "--", "tools/*.mjs", "tools/**/*.mjs"])
    .stdout.split("\n")
    .filter(Boolean);
  const bypass = [];
  const wrapped = new Set();
  for (const f of files) {
    readFileSync(join(REPO, f), "utf8")
      .split("\n")
      .forEach((line, i) => {
        if (!spawn.test(line)) return;
        if (direct.test(line.replace(viaWrapper, '""'))) bypass.push(`${f}:${i + 1}: ${line.trim()}`);
        if (viaWrapper.test(line)) wrapped.add(f);
      });
  }
  for (const [name, node] of Object.entries(nodes)) {
    for (const [target, def] of Object.entries(node.data.targets ?? {})) {
      const commands = [def.options?.command, ...(def.options?.commands ?? [])].filter((c) => typeof c === "string");
      for (const c of commands) {
        if (/(?:^|[;&|(]\s*)(?:\S*\/)?(?:nx|bunx nx|npx nx)\s/.test(c)) bypass.push(`${name}:${target}: ${c}`);
      }
    }
  }
  expect(bypass, "Nx started without scripts/nx").toEqual([]);
  for (const f of ["tools/check-project-boundaries.mjs", "tools/tests/affected.test.mjs", "tools/tests/gate.test.mjs"]) {
    expect([...wrapped], "the known nested Nx callers").toContain(f);
  }
});
