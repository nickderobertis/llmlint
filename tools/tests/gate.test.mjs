// What the gate tiers reach, read from the project graph Nx itself resolves
// (`nx graph`), not from any one project.json: every shell script is under some
// project's shellcheck, the workflows are under actionlint, and `just check`
// runs the targets those checks hang off. Dropping one of them would silently
// un-gate a whole class of files while every remaining target stayed green.
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
    .filter(Boolean);
  expect(scripts.length).toBeGreaterThan(10);
  const missed = scripts.filter((f) => !covered.has(f));
  expect(missed, "shell scripts no project's lint-sh checks").toEqual([]);
});

test("the workflows are linted by actionlint, ci-workflows' lint-workflows", () => {
  expect(commandOf(targetsOf("ci-workflows")["lint-workflows"])).toBe("bash scripts/lint-workflows.sh");
});

test("the check recipe runs every gate target, the shell and workflow lint included, at either tier", () => {
  const justfile = readFileSync(join(REPO, "justfile"), "utf8");
  const recipe = justfile.slice(justfile.indexOf("\ncheck *flags:"));
  const body = recipe.split("\n").slice(1).filter((l) => l.startsWith("    "))[0];
  expect(body).toContain('scripts/nx-tier.sh "$@" -- -t format lint lint-sh lint-workflows build test doc coverage');
});
