// onevcs.toml, the pool maintenance this repository declares, run the way
// onevcs runs it: its `[pool.maintain] command` argv, unchanged, in a worktree
// holding this repository's justfile. This is the drift gate between that
// declaration and the recipe it names: renaming or removing the recipe, or
// changing its window or target dirs, fails here. The pruner is a stand-in
// recording the argv it ran with, so the journey prunes nothing and builds
// nothing; its real behaviour is pool-maintenance-host's journey. Windows is
// skipped: onevcs pool maintenance runs on Unix hosts.
import { afterEach, beforeEach, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { chmodSync, copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, join, resolve } from "node:path";

const REPO = resolve(import.meta.dir, "../..");
const unix = test.skipIf(process.platform === "win32");
let dir, bin;

function declaration() {
  return Bun.TOML.parse(readFileSync(join(REPO, "onevcs.toml"), "utf8"));
}

// The declared command, run in the scratch worktree, with `pruner` as the
// stand-in cargo-prune-superseded's body.
function maintain(pruner) {
  writeFileSync(join(bin, "cargo-prune-superseded"), `#!/bin/sh\n${pruner}`);
  chmodSync(join(bin, "cargo-prune-superseded"), 0o755);
  const [cmd, ...args] = declaration().pool.maintain.command;
  const r = spawnSync(cmd, args, {
    cwd: dir,
    encoding: "utf8",
    env: { ...process.env, PATH: `${bin}${delimiter}${process.env.PATH}`, CALLS: join(dir, "calls.log") },
  });
  if (r.error) throw r.error;
  return { code: r.status, out: `${r.stdout}${r.stderr}` };
}

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "llmlint-declared-"));
  bin = join(dir, "stand-in");
  mkdirSync(bin);
  copyFileSync(join(REPO, "justfile"), join(dir, "justfile"));
});

afterEach(() => rmSync(dir, { recursive: true, force: true }));

test("onevcs.toml declares schema 1 maintenance with a 30-minute timeout and nothing else", () => {
  const d = declaration();
  expect(Object.keys(d).sort()).toEqual(["pool", "schema_version"]);
  expect(d.schema_version).toBe(1);
  expect(d.pool).toEqual({ maintain: { command: ["just", "pool-maintenance"], timeout: "30m" } });
});

unix("the declared command prunes every target dir, keeping 2 days, and reports success", () => {
  const r = maintain('echo "$*" >>"$CALLS"\necho "cargo-prune-superseded: target: removed 0 superseded build(s)"\n');
  expect(r.code, r.out).toBe(0);
  expect(r.out).toContain("cargo-prune-superseded: target: removed 0 superseded build(s)");
  const just = readFileSync(join(REPO, "justfile"), "utf8");
  const targets = just.match(/^pool-maintenance-targets := "([^"]+)"/m)[1];
  expect(readFileSync(join(dir, "calls.log"), "utf8")).toBe(`--keep-within 2d ${targets}\n`);
});

unix("a failed pass fails the declared command, so onevcs sees it", () => {
  const r = maintain('echo "cargo-prune-superseded: target: cannot read" >&2\nexit 3\n');
  expect(r.code, r.out).not.toBe(0);
  expect(r.out).toContain("cargo-prune-superseded: target: cannot read");
});
