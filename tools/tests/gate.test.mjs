// What the gate tiers reach, read from the project graph Nx itself resolves
// (`nx graph`), not from any one project.json: every shell script is under some
// project's shellcheck, the workflows are under actionlint, and `just check`
// runs the targets those checks hang off. Dropping one of them would silently
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

test("every shell script is shellchecked by some project's lint-sh", () => {
  const covered = new Set();
  for (const [name, node] of Object.entries(nodes)) {
    const targets = node.data.targets ?? {};
    if (!targets["lint-sh"]) continue;
    const words = commandOf(targets["lint-sh"]).split(/\s+/);
    expect(words[0], `${name}:lint-sh`).toBe("shellcheck");
    const globs = words.slice(1).filter((w) => !w.startsWith("-"));
    const expanded = run("bash", ["-c", `shopt -s nullglob; for f in ${globs.join(" ")}; do printf '%s\\n' "$f"; done`]);
    for (const f of expanded.stdout.split("\n").filter(Boolean)) covered.add(f);
  }
  const scripts = run("git", ["ls-files", "--cached", "--others", "--exclude-standard", "--", "*.sh", ".githooks/*", "scripts/nx"])
    .stdout.split("\n")
    // A hook is any file in .githooks/ but the project's own definition and docs.
    .filter((f) => f && !(f.startsWith(".githooks/") && /\.(md|json)$/.test(f)));
  expect(scripts).toContain(".githooks/pre-push");
  expect(scripts.length).toBeGreaterThan(10);
  const missed = scripts.filter((f) => !covered.has(f));
  expect(missed, "shell scripts no project's lint-sh checks").toEqual([]);
});

test("the workflows are linted by actionlint, ci-workflows' lint-workflows", () => {
  expect(commandOf(targetsOf("ci-workflows")["lint-workflows"])).toBe("bash scripts/lint-workflows.sh");
});

test("the check recipe runs every gate target, the shell and workflow lint included, at either tier", () => {
  const justfile = readFileSync(join(REPO, "justfile"), "utf8");
  const recipe = justfile.slice(justfile.indexOf("\ncheck *flags:") + 1);
  const body = recipe.slice(0, recipe.indexOf("\n\n"));
  const targets = "-t format lint lint-sh lint-workflows build test doc coverage";
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
