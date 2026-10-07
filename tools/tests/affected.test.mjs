// What the affected tier selects for an edit, asked of Nx itself (`nx show
// projects --affected --files=…`) over the real project graph: an edit selects
// the project owning the file and every project that depends on it, and nothing
// else. A file landing in the wrong project, or an edge missing from the graph,
// would let a change skip the suites that exercise it while the gate stayed green.
// llmlint: ignore-file[shell_test_tiers_stay_split] these tests are offline and hermetic: they ask Nx's own graph resolution (the gate's orchestrator, installed from bun.lock) about the real workspace and run nothing it selects; the files they read are this project's inputs, so a separate project would be selected by exactly the same edits
import { beforeAll, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const REPO = resolve(import.meta.dir, "../..");

function nx(args) {
  const r = spawnSync("bash", ["scripts/nx", ...args], { encoding: "utf8", cwd: REPO });
  if (r.error) throw r.error;
  if (r.status !== 0) throw new Error(`nx ${args.join(" ")} failed: ${r.stderr}`);
  return r.stdout;
}

/** The projects the affected tier selects when only `file` changed. */
const affected = (file) => nx(["show", "projects", "--affected", `--files=${file}`, "--json"]).trim();
const selected = (file) => JSON.parse(affected(file)).sort();

// Every project that depends on `project`, directly or through another,
// read from the graph Nx resolves: who consumes a project.
let dependents;
beforeAll(() => {
  const dir = mkdtempSync(join(tmpdir(), "llmlint-affected-"));
  let deps;
  try {
    const file = join(dir, "graph.json");
    nx(["graph", `--file=${file}`]);
    deps = JSON.parse(readFileSync(file, "utf8")).graph.dependencies;
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
  dependents = (project) => {
    const seen = new Set([project]);
    for (let grew = true; grew; ) {
      grew = false;
      for (const [source, edges] of Object.entries(deps)) {
        if (!seen.has(source) && edges.some((e) => seen.has(e.target))) {
          seen.add(source);
          grew = true;
        }
      }
    }
    return [...seen].sort();
  };
});

test("an edit to the mock fixture selects it and the suites driving it, not the crate", () => {
  const got = selected("tests/mock-oneharness/src/main.rs");
  expect(got).toEqual(dependents("llmlint-mock-oneharness"));
  expect(got).toContain("llmlint-e2e");
  expect(got).not.toContain("llmlint");
  expect(got).not.toContain("bench");
});

test("an edit to the root crate selects it and every project depending on it", () => {
  const got = selected("src/main.rs");
  expect(got).toEqual(dependents("llmlint"));
  for (const p of ["llmlint-mock-oneharness", "llmlint-e2e", "bench", "repo-tooling", "coverage"]) expect(got).toContain(p);
  expect(got).not.toContain("coverage-driver");
});

test("only an edit to the coverage driver selects its self-tests, and it re-runs the gate", () => {
  const got = selected("tools/coverage/coverage.sh");
  expect(got).toEqual(dependents("coverage-driver"));
  expect(got).toContain("coverage");
  for (const p of ["llmlint", "llmlint-e2e"]) expect(got).not.toContain(p);
  for (const file of ["src/main.rs", "tests/e2e/main.rs", "tools/coverage/gate/project.json"]) {
    expect(selected(file)).not.toContain("coverage-driver");
  }
});

test("an edit to a shared script or workflow selects exactly its consumers", () => {
  const script = selected("scripts/nx-base.sh");
  expect(script).toEqual(dependents("repo-tooling"));
  const workflow = selected(".github/workflows/ci.yml");
  expect(workflow).toEqual(dependents("ci-workflows"));
  for (const got of [script, workflow]) {
    for (const p of ["llmlint", "llmlint-mock-oneharness", "llmlint-e2e", "bench"]) expect(got).not.toContain(p);
  }
});
