// pool-maintenance/measure.sh's refusals, which it makes before anything
// builds: run outside onebudgetspec, or with a pruner off the justfile's pin.
// The script runs from a scratch copy of the repository's measure.sh and
// justfile; only the off-pin pruner is a stand-in. The real build-and-prune
// journey drives real cargo and the real pruner, so it is the
// pool-maintenance-host project's (pool-maintenance/host/). Windows is
// skipped: onevcs pool maintenance runs on Unix hosts.
import { afterEach, beforeEach, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, join, resolve } from "node:path";

const REPO = resolve(import.meta.dir, "../..");
const unix = test.skipIf(process.platform === "win32");
let dir;

function measure(extra = {}) {
  const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => k !== "ONEBUDGETSPEC_RESULT"));
  const r = spawnSync("bash", [join(dir, "pool-maintenance/measure.sh")], {
    cwd: join(dir, "pool-maintenance"),
    encoding: "utf8",
    env: { ...env, ...extra },
  });
  if (r.error) throw r.error;
  return { code: r.status, out: `${r.stdout}${r.stderr}` };
}

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "llmlint-pool-"));
  mkdirSync(join(dir, "pool-maintenance"));
  copyFileSync(join(REPO, "pool-maintenance/measure.sh"), join(dir, "pool-maintenance/measure.sh"));
  copyFileSync(join(REPO, "justfile"), join(dir, "justfile"));
});

afterEach(() => rmSync(dir, { recursive: true, force: true }));

unix("run outside onebudgetspec it refuses before anything builds", () => {
  const r = measure();
  expect(r.code, r.out).toBe(2);
  expect(r.out).toContain("ONEBUDGETSPEC_RESULT is not set");
  expect(r.out).toContain("just budgets-host");
  expect(existsSync(join(dir, "target"))).toBe(false);
});

unix("a pruner off the justfile's pin is refused, naming the fix, before anything builds", () => {
  const bin = join(dir, "stand-in");
  mkdirSync(bin);
  writeFileSync(join(bin, "cargo-prune-superseded"), "#!/bin/sh\necho 'cargo-prune-superseded 0.0.1'\n");
  chmodSync(join(bin, "cargo-prune-superseded"), 0o755);
  const want = readFileSync(join(REPO, "justfile"), "utf8").match(/^prune-superseded-version := "([^"]+)"/m)[1];
  const r = measure({ ONEBUDGETSPEC_RESULT: join(dir, "result.json"), PATH: `${bin}${delimiter}${process.env.PATH}` });
  expect(r.code, r.out).toBe(1);
  expect(r.out).toContain(`is 'cargo-prune-superseded 0.0.1', but the justfile pins ${want}; run just bootstrap.`);
  expect(existsSync(join(dir, "target"))).toBe(false);
});
