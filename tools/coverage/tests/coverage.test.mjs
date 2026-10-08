// The coverage driver against a real (tiny) Cargo workspace laid out like this
// repository's: an `llmlint` package (lib + bin) whose unit tests cover one
// function, an `llmlint-e2e` member whose test covers the others — one only by
// spawning the `llmlint` binary beside its own test executable, as the real
// journeys do, so those lines count only if the child's profiles reach the
// report — and the mock fixture member coverage.sh builds for the e2e run. Real
// cargo-llvm-cov and nextest run; nothing is stubbed.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const REPO = resolve(import.meta.dir, "../../..");
let dir;

function write(path, text) {
  mkdirSync(join(dir, path, ".."), { recursive: true });
  writeFileSync(join(dir, path), text);
}

function coverage(args, env = {}) {
  const r = spawnSync("bash", [join(dir, "tools/coverage/coverage.sh"), ...args], {
    cwd: dir,
    encoding: "utf8",
    env: { ...process.env, LLMLINT_COVERAGE: "on", CARGO_TARGET_DIR: join(dir, "target"), ...env },
  });
  if (r.error) throw r.error;
  return { code: r.status, out: `${r.stdout}${r.stderr}` };
}

beforeAll(() => {
  dir = mkdtempSync(join(tmpdir(), "llmlint-coverage-"));
  const pkg = (name, extra = "") => `[package]\nname = "${name}"\nversion = "0.1.0"\nedition = "2021"\npublish = false\n${extra}`;
  write(
    "Cargo.toml",
    // `autotests = false`, as in the real manifest: the e2e suite is its member's, not the package's.
    `${pkg("llmlint", "autotests = false\n")}\n[workspace]\nmembers = ["tests/e2e", "tests/mock-oneharness"]\n`,
  );
  write(
    "src/lib.rs",
    [
      "pub fn unit_covered(x: u64) -> u64 {",
      ...Array.from({ length: 6 }, (_, i) => `    let x = x.wrapping_add(${i});`),
      "    x",
      "}",
      ...Array.from({ length: 10 }, (_, i) => [
        `pub fn e2e_covered_${i}(x: u64) -> u64 {`,
        `    if x > ${i} {`,
        `        x - ${i}`,
        "    } else {",
        `        x + ${i}`,
        "    }",
        "}",
      ]).flat(),
      "pub fn subprocess_covered(x: u64) -> u64 {",
      ...Array.from({ length: 30 }, (_, i) => `    let x = x.wrapping_mul(${i + 2});`),
      "    x",
      "}",
      "#[cfg(test)]",
      "mod tests {",
      "    #[test]",
      "    fn unit() {",
      "        assert_eq!(super::unit_covered(0), 15);",
      "    }",
      "}",
      "",
    ].join("\n"),
  );
  // `--version` (coverage.sh's build run) takes the else branch; only the e2e
  // test's spawned child takes the other.
  write(
    "src/main.rs",
    [
      "fn main() {",
      '    if std::env::args().nth(1).as_deref() == Some("--sub") {',
      '        println!("{}", llmlint::subprocess_covered(1));',
      "    } else {",
      '        println!("llmlint {}", llmlint::unit_covered(0));',
      "    }",
      "}",
      "",
    ].join("\n"),
  );
  write("tests/mock-oneharness/Cargo.toml", pkg("llmlint-mock-oneharness"));
  write("tests/mock-oneharness/src/main.rs", 'fn main() {\n    println!("mock");\n}\n');
  write(
    "tests/e2e/Cargo.toml",
    `${pkg("llmlint-e2e")}\n[[test]]\nname = "e2e"\npath = "main.rs"\n\n[dev-dependencies]\nllmlint = { path = "../.." }\n`,
  );
  const calls = Array.from({ length: 10 }, (_, i) => `    assert_eq!(llmlint::e2e_covered_${i}(${i + 1}), 1);\n    assert_eq!(llmlint::e2e_covered_${i}(0), ${i});\n`).join("");
  const spawn = [
    "#[test]",
    "fn spawned_binary() {",
    "    let exe = std::env::current_exe().unwrap();",
    "    let dir = exe.parent().unwrap().parent().unwrap();",
    '    let bin = dir.join(format!("llmlint{}", std::env::consts::EXE_SUFFIX));',
    '    let out = std::process::Command::new(&bin).arg("--sub").output().unwrap();',
    '    assert!(out.status.success(), "{out:?}");',
    "}",
    "",
  ].join("\n");
  write("tests/e2e/main.rs", `#[test]\nfn e2e() {\n${calls}}\n\n${spawn}`);
  mkdirSync(join(dir, "tools/coverage"), { recursive: true });
  copyFileSync(join(REPO, "tools/coverage/coverage.sh"), join(dir, "tools/coverage/coverage.sh"));
  const lock = spawnSync("cargo", ["generate-lockfile", "--offline"], { cwd: dir, encoding: "utf8" });
  if (lock.status !== 0) throw new Error(`cargo generate-lockfile: ${lock.stderr}`);
});

afterAll(() => rmSync(dir, { recursive: true, force: true }));

test("the unit run alone is below the floor and the report fails, naming it", () => {
  expect(coverage(["clear"]).code).toBe(0);
  const unit = coverage(["test", "llmlint"]);
  expect(unit.code, unit.out).toBe(0);
  const report = coverage(["report"]);
  expect(report.code, report.out).toBe(1);
  expect(report.out).toContain("below 95% line coverage over every crate's run");
  expect(report.out).toMatch(/^lib\.rs\s/m);
}, 600_000);

test("the e2e crate's profiles, its spawned binary's included, combine with the unit run's and pass the floor", () => {
  expect(coverage(["clear"]).code).toBe(0);
  expect(coverage(["test", "llmlint"]).code).toBe(0);
  const e2e = coverage(["test", "llmlint-e2e"]);
  expect(e2e.code, e2e.out).toBe(0);
  const report = coverage(["report"]);
  expect(report.code, report.out).toBe(0);
  // Success is one line: the per-file table is printed only on failure.
  expect(report.out.trim()).toBe("coverage: 100.00% lines covered (floor 95%)");
}, 600_000);

test("a crate that is not a member, an unknown step, or a bad switch is a usage error", () => {
  const notMember = coverage(["test", "llmlint-nope"]);
  expect(notMember.code).toBe(2);
  expect(notMember.out).toContain("'llmlint-nope' is not a member of this Cargo workspace");
  // A member's name on one line of the argument does not make the argument one.
  const multiline = coverage(["test", "llmlint\n../x"]);
  expect(multiline.code).toBe(2);
  expect(multiline.out).toContain("is not a valid crate name");
  expect(coverage(["tset"]).code).toBe(2);
  const bad = coverage(["report"], { LLMLINT_COVERAGE: "maybe" });
  expect(bad.code).toBe(2);
  expect(bad.out).toContain("LLMLINT_COVERAGE must be 'on'");
}, 600_000);
