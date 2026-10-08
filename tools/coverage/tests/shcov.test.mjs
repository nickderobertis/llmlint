// The shell coverage driver (tools/coverage/shcov.sh) against a scratch
// repository, under the real bashcov the root Gemfile.lock pins: `run` around a
// project's test command records what its scripts executed, however deep in the
// process tree and through a scratch copy of a script, and `report` merges the
// records and fails below the floor. Nothing is stubbed; the "test commands" are
// small bash runners that drive the scratch scripts the way the journeys do.
import { afterEach, beforeEach, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const REPO = resolve(import.meta.dir, "../../..");
const FLOOR = Number(readFileSync(join(REPO, "tools/coverage/shcov.sh"), "utf8").match(/^readonly MIN_LINES=(\d+)$/m)[1]);
let dir;

function write(path, text, mode) {
  mkdirSync(join(dir, path, ".."), { recursive: true });
  writeFileSync(join(dir, path), text);
  if (mode) chmodSync(join(dir, path), mode);
}

function shcov(args, env = {}) {
  const r = spawnSync("bash", [join(dir, "tools/coverage/shcov.sh"), ...args], {
    cwd: dir,
    encoding: "utf8",
    env: { ...process.env, LLMLINT_COVERAGE: "on", ...env },
  });
  if (r.error) throw r.error;
  return { code: r.status, out: `${r.stdout}${r.stderr}` };
}

const record = (project) => JSON.parse(readFileSync(join(dir, `target/shcov/${project}.json`), "utf8"))[project].coverage;

// A script with one line per branch, so which lines ran says which arguments did.
const SCRIPT = [
  "#!/usr/bin/env bash",
  "set -euo pipefail",
  'case "${1:-}" in',
  "  a) echo took-a ;;",
  "  b) echo took-b ;;",
  "  *) echo took-other ;;",
  "esac",
  "",
].join("\n");

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "llmlint-shcov-"));
  for (const f of ["tools/coverage/shcov.sh", "tools/coverage/shcov.rb", "tools/shell/shell.sh", "Gemfile", "Gemfile.lock"]) {
    mkdirSync(join(dir, f, ".."), { recursive: true });
    copyFileSync(join(REPO, f), join(dir, f));
  }
  // The gems are the checkout's (`just bootstrap` installs them there); Bundler
  // reads the path from this file before the environment.
  write(".bundle/config", `---\nBUNDLE_PATH: "${join(REPO, ".dev/bundle")}"\nBUNDLE_FROZEN: "true"\n`);
  // The driver is not one of the scratch repository's scripts under test.
  write(".gitignore", "/tools/\n/target/\n");
  write("scripts/branchy.sh", SCRIPT, 0o755);
  const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !k.startsWith("GIT_")));
  spawnSync("git", ["-C", dir, "init", "-q"], { env });
});

afterEach(() => rmSync(dir, { recursive: true, force: true }));

test("a run records the lines its scripts executed, through a grandchild and a scratch copy, and not a stand-in's", () => {
  // `a` runs the script itself from a child bash; `b` runs a byte-identical copy
  // from a scratch root that is deleted before the run ends; the stand-in shares
  // the script's path in another scratch root but not its bytes, so it must not
  // count (it would mark line 9, past the script's end).
  write(
    "run-tests.sh",
    [
      "#!/usr/bin/env bash",
      "set -euo pipefail",
      'bash -c "bash scripts/branchy.sh a"',
      'copy="$(mktemp -d)"',
      'mkdir -p "$copy/scripts"',
      'cp scripts/branchy.sh "$copy/scripts/branchy.sh"',
      '(cd "$copy" && bash scripts/branchy.sh b)',
      'rm -rf "$copy"',
      'stand_in="$(mktemp -d)"',
      'mkdir -p "$stand_in/scripts"',
      "printf '#!/usr/bin/env bash\\n\\n\\n\\n\\n\\n\\n\\necho stand-in\\n' >\"$stand_in/scripts/branchy.sh\"",
      'bash "$stand_in/scripts/branchy.sh" c',
      'rm -rf "$stand_in"',
      "",
    ].join("\n"),
    0o755,
  );
  const r = shcov(["run", "demo", "--", "bash", "run-tests.sh"]);
  expect(r.code, r.out).toBe(0);
  expect(r.out).toContain("took-a");
  expect(r.out).toContain("took-b");
  const lines = record("demo")["scripts/branchy.sh"].lines;
  expect(lines[3], "line 4, the a branch").toBeGreaterThan(0);
  expect(lines[4], "line 5, the b branch (run as a deleted copy)").toBeGreaterThan(0);
  expect(lines[5] ?? 0, "line 6, never taken").toBe(0);
  expect(lines.length, "nothing past the script's last line (the stand-in's)").toBeLessThanOrEqual(8);
});

test("a child started with a cleared environment is measured when the journey hands it SHCOV_BASH_ENV", () => {
  write("run-tests.sh", '#!/usr/bin/env bash\nenv -i PATH=/usr/bin:/bin BASH_ENV="$SHCOV_BASH_ENV" bash scripts/branchy.sh a\n', 0o755);
  expect(shcov(["run", "demo", "--", "bash", "run-tests.sh"]).code).toBe(0);
  expect(record("demo")["scripts/branchy.sh"].lines[3]).toBeGreaterThan(0);
});

test("the BASH_ENV a run replaces still runs first, through a nested run, but not in a cleared environment", () => {
  // The parent BASH_ENV marks each bash it ran in. A nested run (the
  // coverage-driver's own suite runs shcov under shcov) must keep it too, and
  // end — a snippet that re-read its parent from the environment would source
  // itself forever.
  const mark = join(dir, "parent-ran");
  write("parent-env.sh", `printf '%s\\n' "$0" >>${JSON.stringify(mark)}\n`);
  write(
    "run-tests.sh",
    [
      "#!/usr/bin/env bash",
      "bash scripts/branchy.sh a",
      'bash tools/coverage/shcov.sh run inner -- bash scripts/branchy.sh b',
      'env -i PATH=/usr/bin:/bin BASH_ENV="$SHCOV_BASH_ENV" bash scripts/branchy.sh c',
      "",
    ].join("\n"),
    0o755,
  );
  const r = shcov(["run", "outer", "--", "bash", "run-tests.sh"], { BASH_ENV: join(dir, "parent-env.sh") });
  expect(r.code, r.out).toBe(0);
  const ran = readFileSync(mark, "utf8").split("\n").filter(Boolean);
  expect(ran).toContain("run-tests.sh");
  expect(ran.filter((s) => s === "scripts/branchy.sh").length, "a and the nested b ran it; the cleared c did not").toBe(2);
  expect(record("outer")["scripts/branchy.sh"].lines[3]).toBeGreaterThan(0);
  expect(record("inner")["scripts/branchy.sh"].lines[4]).toBeGreaterThan(0);
  expect(record("outer")["scripts/branchy.sh"].lines[5], "the cleared child is still measured").toBeGreaterThan(0);
});

test("the command's own exit status is the run's, and its record is still written", () => {
  write("run-tests.sh", "#!/usr/bin/env bash\nbash scripts/branchy.sh a\nexit 7\n", 0o755);
  const r = shcov(["run", "demo", "--", "bash", "run-tests.sh"]);
  expect(r.code).toBe(7);
  expect(record("demo")["scripts/branchy.sh"].lines[3]).toBeGreaterThan(0);
});

test("without sha256sum a run hashes with shasum, still counting a copy, and with neither it refuses before the command runs", () => {
  // This host's PATH with both SHA-256 tools left out; shasum is then played by a
  // stand-in that hashes with the real sha256sum by absolute path.
  const bin = join(dir, "sandbox");
  mkdirSync(bin);
  for (const d of (process.env.PATH ?? "").split(":").filter(Boolean)) {
    let names = [];
    try {
      names = readdirSync(d);
    } catch {
      continue;
    }
    for (const n of names) {
      if (n === "sha256sum" || n === "shasum" || existsSync(join(bin, n))) continue;
      symlinkSync(join(d, n), join(bin, n));
    }
  }
  const real = spawnSync("bash", ["-c", "type -P sha256sum"], { encoding: "utf8" }).stdout.trim();
  write("hash/shasum", `#!/bin/sh\n[ "$1" = -a ] && shift 2\nexec ${real} "$@"\n`, 0o755);
  const ran = join(dir, "command-ran");
  write(
    "run-tests.sh",
    [
      "#!/usr/bin/env bash",
      "set -euo pipefail",
      `touch ${JSON.stringify(ran)}`,
      'copy="$(mktemp -d)"',
      'mkdir -p "$copy/scripts"',
      'cp scripts/branchy.sh "$copy/scripts/branchy.sh"',
      '(cd "$copy" && bash scripts/branchy.sh b)',
      'rm -rf "$copy"',
      "",
    ].join("\n"),
    0o755,
  );

  symlinkSync(join(dir, "hash/shasum"), join(bin, "shasum"));
  let r = shcov(["run", "demo", "--", "bash", "run-tests.sh"], { PATH: bin });
  expect(r.code, r.out).toBe(0);
  expect(record("demo")["scripts/branchy.sh"].lines[4], "line 5, run as a copy identified by its shasum digest").toBeGreaterThan(0);

  rmSync(join(bin, "shasum"));
  rmSync(ran);
  r = shcov(["run", "demo", "--", "bash", "run-tests.sh"], { PATH: bin });
  expect(r.code).toBe(1);
  expect(r.out).toContain("no sha256sum or shasum on PATH; install coreutils and re-run.");
  expect(existsSync(ran), "the measured command never started").toBe(false);
});

test("a trace that turns unreadable keeps the lines read before it, and says so", () => {
  // bashcov's parser needs LINENO in every trace line; a script that unsets it
  // garbles the rest of its own trace, never another process's or the lines
  // already read.
  write(
    "scripts/unsets.sh",
    ["#!/usr/bin/env bash", "echo before", "unset LINENO", "echo after", ""].join("\n"),
    0o755,
  );
  write("run-tests.sh", "#!/usr/bin/env bash\nbash scripts/unsets.sh\nbash scripts/branchy.sh a\n", 0o755);
  const r = shcov(["run", "demo", "--", "bash", "run-tests.sh"]);
  expect(r.code, r.out).toBe(0);
  expect(r.out).toContain("keeping the lines read before it");
  const rec = record("demo");
  expect(rec["scripts/unsets.sh"].lines[1], "line 2, before the unset").toBeGreaterThan(0);
  expect(rec["scripts/branchy.sh"].lines[3], "another process's trace is unaffected").toBeGreaterThan(0);
});

test("the report passes merged records at or above the floor and fails below it, listing the least-covered script", () => {
  // Two projects each cover one branch; only merged do they cover the script.
  write("run-a.sh", "#!/usr/bin/env bash\nbash scripts/branchy.sh a\n", 0o755);
  write("run-rest.sh", "#!/usr/bin/env bash\nbash scripts/branchy.sh b\nbash scripts/branchy.sh c\n", 0o755);
  expect(shcov(["run", "one", "--", "bash", "run-a.sh"]).code).toBe(0);
  expect(shcov(["run", "two", "--", "bash", "run-rest.sh"]).code).toBe(0);
  let r = shcov(["report", "one", "two"]);
  expect(r.code, r.out).toBe(0);
  expect(r.out).toMatch(/shcov: 100\.00% of shell lines covered \(\d+\/\d+, floor \d+%\)/);
  expect(FLOOR, "the floor this journey holds the gate to").toBeGreaterThan(5);

  // A script nobody runs, big enough to sink the total under any floor above 5%.
  write("scripts/untested.sh", `#!/usr/bin/env bash\n${Array.from({ length: 200 }, (_, i) => `echo ${i}`).join("\n")}\n`);
  r = shcov(["report", "one", "two"]);
  expect(r.code, r.out).toBe(1);
  expect(r.out).toMatch(/0\.00%\s+0\/200\s+scripts\/untested\.sh/);
  expect(r.out).toContain(`floor ${FLOOR}%`);
  expect(r.out).toContain("below the floor");
  const merged = JSON.parse(readFileSync(join(dir, "target/shcov/merged.json"), "utf8")).merged.coverage;
  expect(merged["scripts/untested.sh"].lines.filter((n) => n === 0).length).toBe(200);
});

test("hits past a script's last line are not counted, so a damaged record cannot inflate the number", () => {
  // branchy.sh has 7 lines: a record claiming hits on lines 8-500 credits none.
  write("target/shcov/padded.json", JSON.stringify({ padded: { coverage: { "scripts/branchy.sh": { lines: Array(500).fill(1).fill(0, 0, 7) } } } }));
  const r = shcov(["report", "padded"]);
  expect(r.code, r.out).toBe(1);
  expect(r.out).toMatch(/0\.00% of shell lines covered \(0\/\d+,/);
});

test("a record that is missing or not a record fails the report rather than passing over it", () => {
  let r = shcov(["report", "never-ran"]);
  expect(r.code).toBe(1);
  expect(r.out).toContain("no resultset at target/shcov/never-ran.json");
  write("target/shcov/broken.json", "{ not json");
  r = shcov(["report", "broken"]);
  expect(r.code).toBe(1);
  expect(r.out).toContain("is not a resultset");
  // Valid JSON of the wrong shape is refused too, rather than merged.
  for (const bad of [
    {},
    { p: { coverage: { "scripts/branchy.sh": { lines: [null, -1] } } } },
    { p: { coverage: { "scripts/branchy.sh": { lines: "1,2" } } } },
    { p: { coverage: [] } },
    [],
  ]) {
    write("target/shcov/shape.json", JSON.stringify(bad));
    r = shcov(["report", "shape"]);
    expect(r.code, JSON.stringify(bad)).toBe(1);
    expect(r.out, JSON.stringify(bad)).toContain("is not a resultset of the shape shcov.rb run writes");
  }
});

test("LLMLINT_COVERAGE=off runs the command unmeasured and stands install and report down", () => {
  write("run-tests.sh", "#!/usr/bin/env bash\nbash scripts/branchy.sh a\nexit 3\n", 0o755);
  const off = { LLMLINT_COVERAGE: "off" };
  const r = shcov(["run", "demo", "--", "bash", "run-tests.sh"], off);
  expect(r.code).toBe(3);
  expect(r.out).toContain("took-a");
  expect(existsSync(join(dir, "target/shcov/demo.json"))).toBe(false);
  for (const args of [["report", "demo"], ["install"]]) {
    const s = shcov(args, off);
    expect(s.code, args[0]).toBe(0);
    expect(s.out, args[0]).toContain("LLMLINT_COVERAGE=off");
  }
});

test("install provisions the locked gems, and a failed bundle install is named with the fix", () => {
  let r = shcov(["install"]);
  expect(r.code, r.out).toBe(0);
  // A Gemfile the lock does not match is refused (the config is frozen), offline.
  writeFileSync(join(dir, "Gemfile"), `${readFileSync(join(dir, "Gemfile"), "utf8")}gem "llmlint-shcov-absent", "1.0.0"\n`);
  r = shcov(["install"]);
  expect(r.code).toBe(1);
  expect(r.out).toContain("bundle install failed (above); fix it and re-run: just bootstrap");
});

test("the floor shcov.sh enforces is the one AGENTS.md records", () => {
  // AGENTS.md states the approved shell floor twice (the deviations list and
  // "Shell"); the driver's MIN_LINES is the enforced copy. They move together.
  const agents = readFileSync(join(REPO, "AGENTS.md"), "utf8");
  expect(Number(agents.match(/\*\*(\d+)% lines for shell\*\*/)[1])).toBe(FLOOR);
  expect(Number(agents.match(/^- \*\*Floor: (\d+)% lines\*\*/m)[1])).toBe(FLOOR);
  const nested = readFileSync(join(REPO, "tools/coverage/AGENTS.md"), "utf8");
  expect(Number(nested.match(/The measurement behind the (\d+)% floor/)[1])).toBe(FLOOR);
});

test("without Ruby or Bundler each step that needs them is refused, naming the fix", () => {
  // A PATH of only what shcov.sh itself runs, plus (second case) a stand-in ruby.
  const bin = join(dir, "sandbox");
  mkdirSync(bin);
  for (const t of ["bash", "dirname", "grep"]) {
    symlinkSync(spawnSync("bash", ["-c", `type -P ${t}`], { encoding: "utf8" }).stdout.trim(), join(bin, t));
  }
  for (const [missing, extra] of [["ruby", []], ["bundle", ["ruby"]]]) {
    for (const t of extra) write(`sandbox/${t}`, "#!/bin/sh\nexit 0\n", 0o755);
    for (const args of [["install"], ["run", "demo", "--", "true"], ["report", "demo"]]) {
      const r = shcov(args, { PATH: bin });
      expect(r.code, `${missing} ${args[0]}`).toBe(1);
      expect(r.out, `${missing} ${args[0]}`).toContain(`shcov: ${missing} not found on PATH`);
      expect(r.out, `${missing} ${args[0]}`).toContain("then run: just bootstrap");
    }
  }
});

test("a malformed invocation is a usage error, not a pass", () => {
  for (const args of [[], ["typo"], ["run", "demo"], ["run", "demo", "bash", "x"], ["run", "Bad_Name", "--", "true"], ["report"], ["report", "../x"], ["install", "x"]]) {
    const r = shcov(args);
    expect(r.code, JSON.stringify(args)).toBe(2);
    expect(r.out, JSON.stringify(args)).toContain("usage: tools/coverage/shcov.sh");
  }
  const r = shcov(["run", "demo", "--", "true"], { LLMLINT_COVERAGE: "maybe" });
  expect(r.code).toBe(2);
  expect(r.out).toContain("must be 'on' (the default) or 'off'");
});
