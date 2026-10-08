// tools/shell/shell.sh the way every shell project's format and lint targets run
// it: in a scratch repository carrying the real script, justfile pins and
// .editorconfig style, against the real pinned shfmt and shellcheck that
// `just bootstrap` installs into ~/.local/bin. Only a tool's version (for the
// off-pin journey) is ever stood in.
import { afterEach, beforeEach, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { chmodSync, copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const REPO = resolve(import.meta.dir, "../../..");
let dir;

function write(path, text, mode) {
  mkdirSync(join(dir, path, ".."), { recursive: true });
  writeFileSync(join(dir, path), text);
  if (mode) chmodSync(join(dir, path), mode);
}

function git(...args) {
  const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !k.startsWith("GIT_")));
  const r = spawnSync("git", ["-C", dir, ...args], { encoding: "utf8", env });
  if (r.status !== 0) throw new Error(`git ${args.join(" ")}: ${r.stderr}`);
}

function shell(args, env = {}) {
  const r = spawnSync("bash", [join(dir, "tools/shell/shell.sh"), ...args], {
    cwd: dir,
    encoding: "utf8",
    env: { ...process.env, LLMLINT_SHELL_TOOLS: "on", ...env },
  });
  if (r.error) throw r.error;
  return { code: r.status, out: `${r.stdout}${r.stderr}`, stdout: r.stdout };
}

const pin = (tool) => readFileSync(join(REPO, "justfile"), "utf8").match(new RegExp(`^${tool}-version := "([^"]+)"`, "m"))[1];

const FORMATTED = '#!/usr/bin/env bash\nset -euo pipefail\nif [ "${1:-}" = a ]; then\n  echo a\nfi\n';
const UNFORMATTED = '#!/usr/bin/env bash\nset -euo pipefail\nif [ "${1:-}" = a ]; then\n      echo a\nfi\n';

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "llmlint-shell-"));
  mkdirSync(join(dir, "tools/shell"), { recursive: true });
  copyFileSync(join(REPO, "tools/shell/shell.sh"), join(dir, "tools/shell/shell.sh"));
  copyFileSync(join(REPO, "justfile"), join(dir, "justfile"));
  copyFileSync(join(REPO, ".editorconfig"), join(dir, ".editorconfig"));
  git("init", "-q");
});

afterEach(() => rmSync(dir, { recursive: true, force: true }));

test("format passes a script in the recorded style and fails one that is not, naming the fix", () => {
  write("scripts/good.sh", FORMATTED);
  write("scripts/bad.sh", UNFORMATTED);
  expect(shell(["format", "scripts/good.sh"]).code).toBe(0);
  const r = shell(["format", "scripts/good.sh", "scripts/bad.sh"]);
  expect(r.code).toBe(1);
  expect(r.out).toContain("scripts/bad.sh");
  expect(r.out).toContain("-      echo a");
  expect(r.out).toContain("+  echo a");
  expect(r.out).toContain("just format");
});

test("format --write rewrites a script to the style, after which the check passes", () => {
  write("scripts/bad.sh", UNFORMATTED);
  expect(shell(["format", "--write", "scripts/bad.sh"]).code).toBe(0);
  expect(readFileSync(join(dir, "scripts/bad.sh"), "utf8")).toBe(FORMATTED);
  expect(shell(["format", "scripts/bad.sh"]).code).toBe(0);
});

test("the style is the one .editorconfig records, for a script with no extension too", () => {
  // `.githooks/pre-push`-style: no .sh, so only its own section styles it.
  write(".githooks/pre-push", UNFORMATTED);
  expect(shell(["format", ".githooks/pre-push"]).code).toBe(1);
  write(".githooks/pre-push", FORMATTED);
  expect(shell(["format", ".githooks/pre-push"]).code).toBe(0);
});

test("lint fails on a shellcheck finding with its code and passes a clean script", () => {
  write("scripts/clean.sh", FORMATTED);
  write("scripts/finding.sh", '#!/usr/bin/env bash\nset -euo pipefail\nf=$1\nrm $f\n');
  expect(shell(["lint", "scripts/clean.sh"]).code).toBe(0);
  const r = shell(["lint", "scripts/clean.sh", "scripts/finding.sh"]);
  expect(r.code).toBe(1);
  expect(r.out).toContain("scripts/finding.sh line 4");
  expect(r.out).toContain("SC2086");
  expect(r.out).toContain("disable it at that site with its reason");
});

test("files lists every shell script by extension or shebang, and nothing ignored or of another language", () => {
  write("a.sh", "echo a\n");
  write("lib/b.bash", "echo b\n");
  write("hooks/pre-push", "#!/usr/bin/env bash\necho hook\n");
  write("bin/posix", "#!/bin/sh\necho posix\n");
  write("bin/tool.py", "#!/usr/bin/env python3\nprint(1)\n");
  write("notes.txt", "a note that mentions #!/bin/sh after its first character\n");
  write("bin/shout", "#!/usr/bin/env shout\n");
  write("ignored/c.sh", "echo c\n");
  write(".gitignore", "/ignored/\n");
  const r = shell(["files"]);
  expect(r.code).toBe(0);
  expect(r.stdout.split("\n").filter(Boolean)).toEqual(["a.sh", "bin/posix", "hooks/pre-push", "lib/b.bash", "tools/shell/shell.sh"]);
});

test("files outside a git work tree is a clear failure, not an empty list", () => {
  rmSync(join(dir, ".git"), { recursive: true, force: true });
  const r = shell(["files"], { GIT_CEILING_DIRECTORIES: dir });
  expect(r.code).toBe(1);
  expect(r.out).toContain("listing the tree's files with git failed");
});

test("versions names each pin and the version found, which keys the lint cache", () => {
  const r = shell(["versions"]);
  expect(r.code).toBe(0);
  expect(r.stdout).toContain(`shfmt pin=${pin("shfmt")} have=${pin("shfmt")}`);
  expect(r.stdout).toContain(`shellcheck pin=${pin("shellcheck")} have=${pin("shellcheck")}`);
  expect(r.stdout).toContain("LLMLINT_SHELL_TOOLS=on");
});

test("LLMLINT_SHELL_TOOLS=off stands format and lint down with a notice; any other value is a usage error", () => {
  write("scripts/bad.sh", UNFORMATTED);
  for (const step of ["format", "lint"]) {
    const r = shell([step, "scripts/bad.sh"], { LLMLINT_SHELL_TOOLS: "off" });
    expect(r.code, step).toBe(0);
    expect(r.out, step).toContain("LLMLINT_SHELL_TOOLS=off");
  }
  const r = shell(["format", "scripts/bad.sh"], { LLMLINT_SHELL_TOOLS: "no" });
  expect(r.code).toBe(2);
  expect(r.out).toContain("must be 'on' (the default) or 'off'");
});

test("a tool off its pin is refused before it formats anything, naming the pin and the fix", () => {
  write("scripts/good.sh", FORMATTED);
  const home = join(dir, "home");
  write("home/.local/bin/shfmt", "#!/bin/sh\necho v0.0.1\n", 0o755);
  const r = shell(["format", "scripts/good.sh"], { HOME: home });
  expect(r.code).toBe(1);
  expect(r.out).toContain(`is shfmt 0.0.1, but the justfile pins ${pin("shfmt")}`);
  expect(r.out).toContain("just shell-tools");
});

test("a missing tool or a missing pin is refused with the line that fixes it", () => {
  write("scripts/good.sh", FORMATTED);
  const bare = { HOME: join(dir, "home"), PATH: "/usr/bin:/bin" };
  const missing = spawnSync("bash", ["-c", "command -v shellcheck"], { env: bare });
  if (missing.status !== 0) {
    const r = shell(["lint", "scripts/good.sh"], bare);
    expect(r.code).toBe(1);
    expect(r.out).toContain("shellcheck not found on PATH");
  }
  writeFileSync(join(dir, "justfile"), "default:\n    @true\n");
  const r = shell(["lint", "scripts/good.sh"]);
  expect(r.code).toBe(1);
  expect(r.out).toContain('restore the line: shellcheck-version := "<version>"');
});

test("a malformed invocation is a usage error, not a pass", () => {
  for (const args of [[], ["typo"], ["format"], ["format", "--write"], ["lint"], ["files", "x"], ["versions", "x"]]) {
    const r = shell(args);
    expect(r.code, JSON.stringify(args)).toBe(2);
    expect(r.out, JSON.stringify(args)).toContain("usage: tools/shell/shell.sh");
  }
});
