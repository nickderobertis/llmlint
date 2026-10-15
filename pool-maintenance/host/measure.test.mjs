// pool-maintenance/measure.sh the way `onebudgetspec check` runs it, over a
// one-crate scratch workspace carrying the real script, justfile (its
// pool-maintenance recipe and pins) and .cargo/config.toml: real cargo builds,
// real mtimes, and the pinned cargo-prune-superseded `just bootstrap` installs.
// It drives those real host tools, so it is this project's, apart from
// pool-maintenance's offline journeys. Windows is skipped: onevcs pool
// maintenance runs on Unix hosts, and the script's find/touch are Unix tools.
import { afterEach, beforeEach, expect, setDefaultTimeout, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const REPO = resolve(import.meta.dir, "../..");
const unix = test.skipIf(process.platform === "win32");
// Each journey compiles the scratch crate two or three times.
setDefaultTimeout(300_000);
let dir;

function write(path, text) {
  mkdirSync(join(dir, path, ".."), { recursive: true });
  writeFileSync(join(dir, path), text);
}

// The caller's environment, minus anything that would send a build elsewhere.
function env(extra = {}) {
  const base = Object.fromEntries(
    Object.entries(process.env).filter(([k]) => !["CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR", "ONEBUDGETSPEC_RESULT"].includes(k)),
  );
  return { ...base, ...extra };
}

function cargo(...args) {
  const r = spawnSync("cargo", args, { cwd: dir, encoding: "utf8", env: env() });
  if (r.status !== 0) throw new Error(`cargo ${args.join(" ")}: ${r.stderr}`);
}

function measure(extra = {}) {
  const r = spawnSync("bash", [join(dir, "pool-maintenance/measure.sh")], {
    cwd: join(dir, "pool-maintenance"),
    encoding: "utf8",
    env: env(extra),
  });
  if (r.error) throw r.error;
  return { code: r.status, out: `${r.stdout}${r.stderr}` };
}

// The build entries the pruner judges in the scratch crate's debug profile.
function entries() {
  const profile = join(dir, "target/debug");
  return ["deps", ".fingerprint", "build", "incremental"]
    .filter((kind) => existsSync(join(profile, kind)))
    .flatMap((kind) => readdirSync(join(profile, kind)).map((name) => `${kind}/${name}`))
    .sort();
}

// Every file and directory under the target dir, modified on 1 January 1999.
function backdateTarget() {
  const r = spawnSync("bash", ["-c", "find target -exec touch -h -t 199901010000 {} +"], { cwd: dir, encoding: "utf8" });
  if (r.status !== 0) throw new Error(`backdating target: ${r.stderr}`);
}

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "llmlint-pool-"));
  mkdirSync(join(dir, "pool-maintenance"));
  mkdirSync(join(dir, ".cargo"));
  copyFileSync(join(REPO, "pool-maintenance/measure.sh"), join(dir, "pool-maintenance/measure.sh"));
  copyFileSync(join(REPO, "justfile"), join(dir, "justfile"));
  copyFileSync(join(REPO, ".cargo/config.toml"), join(dir, ".cargo/config.toml"));
  write("Cargo.toml", '[package]\nname = "demo"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\n');
  write("src/lib.rs", "pub fn answer() -> u32 {\n    42\n}\n\n#[test]\nfn unit() {\n    assert_eq!(answer(), 42);\n}\n");
  write("tests/it.rs", "#[test]\nfn integration() {\n    assert_eq!(demo::answer(), 42);\n}\n");
  cargo("generate-lockfile", "--offline");
});

afterEach(() => rmSync(dir, { recursive: true, force: true }));

unix("a backlog is cleared untimed, then the timed pass removes only the alternative build and keeps the ordinary one", () => {
  // A backlog: an older configuration's builds, past the window. Nothing has
  // superseded them yet: the ordinary build measure.sh makes does that.
  cargo("test", "--no-run", "--locked", "--config", 'profile.dev.package."demo".opt-level = 1');
  backdateTarget();
  const backlog = entries();
  expect(backlog.length).toBeGreaterThan(0);

  const result = join(dir, "result.json");
  writeFileSync(result, "");
  const r = measure({ ONEBUDGETSPEC_RESULT: result });
  expect(r.code, r.out).toBe(0);
  // The per-target line the declared recipe prints, for both passes.
  expect(r.out.match(/cargo-prune-superseded: target: removed \d+ superseded build\(s\)/g)?.length, r.out).toBe(2);

  const report = JSON.parse(readFileSync(result, "utf8"));
  expect(typeof report.value).toBe("number");
  expect(report.value).toBeGreaterThanOrEqual(0);
  // The figure is the timed pass's own report; the backlog pass is evidence only.
  expect(report.detail).toMatch(/^entries walked: \d+; profile directories: 1; builds removed: [1-9]\d*; bytes freed: \d+;/);
  expect(report.detail).toMatch(/; untimed backlog pass \(evidence, not the figure\): [\d.e-]+ s, builds removed: [1-9]\d*;/);
  expect(report.detail).toMatch(/; entries removed: [1-9]\d*, all of the alternative build's [1-9]\d*; ordinary entries retained: all [1-9]\d*$/);

  // The backlog and the alternative build are gone; the timed pass removed only
  // the alternative build's entries.
  const lists = join(dir, "target/pool-maintenance-measure");
  const removed = readFileSync(join(lists, "removed.txt"), "utf8").split("\n").filter(Boolean);
  const alternative = readFileSync(join(lists, "alternative.txt"), "utf8").split("\n").filter(Boolean);
  expect(removed.length).toBeGreaterThan(0);
  expect(removed.filter((e) => !alternative.includes(e))).toEqual([]);
  const left = entries();
  expect(left.filter((e) => backlog.includes(e))).toEqual([]);
  expect(left.filter((e) => alternative.includes(`target/debug/${e}`))).toEqual([]);
  // The ordinary build is whole: building it again compiles nothing.
  const again = spawnSync("cargo", ["test", "--no-run", "--locked"], { cwd: dir, encoding: "utf8", env: env() });
  expect(again.status, again.stderr).toBe(0);
  expect(again.stderr).not.toContain("Compiling");
});
