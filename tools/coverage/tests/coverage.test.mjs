// The coverage driver against a real (tiny) Cargo workspace laid out like this
// repository's: an `llmlint` package (lib + bin) whose unit tests cover one
// function, an `llmlint-e2e` member whose test covers the other, and the mock
// fixture member coverage.sh builds for the e2e run. Real cargo-llvm-cov and
// nextest run; nothing is stubbed.
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
  write("src/main.rs", 'fn main() {\n    println!("llmlint {}", llmlint::unit_covered(0));\n}\n');
  write("tests/mock-oneharness/Cargo.toml", pkg("llmlint-mock-oneharness"));
  write("tests/mock-oneharness/src/main.rs", 'fn main() {\n    println!("mock");\n}\n');
  write(
    "tests/e2e/Cargo.toml",
    `${pkg("llmlint-e2e")}\n[[test]]\nname = "e2e"\npath = "main.rs"\n\n[dev-dependencies]\nllmlint = { path = "../.." }\n`,
  );
  const calls = Array.from({ length: 10 }, (_, i) => `    assert_eq!(llmlint::e2e_covered_${i}(${i + 1}), 1);\n    assert_eq!(llmlint::e2e_covered_${i}(0), ${i});\n`).join("");
  write("tests/e2e/main.rs", `#[test]\nfn e2e() {\n${calls}}\n`);
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
}, 600_000);

test("the e2e crate's profiles combine with the unit run's and pass the floor", () => {
  expect(coverage(["clear"]).code).toBe(0);
  expect(coverage(["test", "llmlint"]).code).toBe(0);
  const e2e = coverage(["test", "llmlint-e2e"]);
  expect(e2e.code, e2e.out).toBe(0);
  const report = coverage(["report"]);
  expect(report.code, report.out).toBe(0);
  // Only the package's own sources are measured: the members under tests/ are not.
  expect(report.out).toMatch(/^lib\.rs\s/m);
  expect(report.out).not.toContain("e2e/main.rs");
  expect(report.out).toMatch(/coverage: 100\.00% lines covered \(floor 95%\)/);
}, 600_000);

test("a crate that is not a member, an unknown step, or a bad switch is a usage error", () => {
  const notMember = coverage(["test", "llmlint-nope"]);
  expect(notMember.code).toBe(2);
  expect(notMember.out).toContain("'llmlint-nope' is not a member of this Cargo workspace");
  expect(coverage(["tset"]).code).toBe(2);
  const bad = coverage(["report"], { LLMLINT_COVERAGE: "maybe" });
  expect(bad.code).toBe(2);
  expect(bad.out).toContain("LLMLINT_COVERAGE must be 'on'");
}, 600_000);
