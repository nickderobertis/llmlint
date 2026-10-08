// tools/shell/shell.sh the way every shell project's format and lint targets run
// it: in a scratch repository carrying the real script, justfile pins and
// .editorconfig style, against the real pinned shfmt and shellcheck that
// `just bootstrap` installs into ~/.local/bin. Only a tool's version (for the
// off-pin journey) is ever stood in.
// llmlint: ignore-file[shell_test_tiers_stay_split] the host tools this suite runs are the pinned shfmt and shellcheck every gate run already requires, executed offline over scratch files in milliseconds; it exercises only this project's shell.sh, so a separate host-tool project would be selected by exactly the same edits and save no run
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
  if (mode !== undefined) chmodSync(join(dir, path), mode);
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

test("a file that is missing, a glob that matched nothing, or one shfmt cannot parse is named as that, not as drift", () => {
  for (const step of ["format", "lint"]) {
    const r = shell([step, "scripts/*.sh"]);
    expect(r.code, step).toBe(1);
    expect(r.out, step).toContain("cannot read scripts/*.sh; check the path");
  }
  write("scripts/broken.sh", "#!/usr/bin/env bash\nif then\n");
  const r = shell(["format", "scripts/broken.sh"]);
  expect(r.code).toBe(1);
  expect(r.out).toContain("scripts/broken.sh:2:");
  expect(r.out).toContain("shfmt could not parse a file");
  expect(r.out).not.toContain("not formatted to the .editorconfig style");
});

test("format --write that cannot rewrite a file fails naming the fix", () => {
  write("scripts/bad.sh", UNFORMATTED);
  write("scripts/broken.sh", "#!/usr/bin/env bash\nif then\n");
  const r = shell(["format", "--write", "scripts/bad.sh", "scripts/broken.sh"]);
  expect(r.code).toBe(1);
  expect(r.out).toContain("shfmt could not rewrite the files");
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

test("SHELLCHECK_OPTS from the environment cannot weaken the lint", () => {
  write("scripts/finding.sh", '#!/usr/bin/env bash\nset -euo pipefail\nf=$1\nrm $f\n');
  const r = shell(["lint", "scripts/finding.sh"], { SHELLCHECK_OPTS: "-e SC2086" });
  expect(r.code).toBe(1);
  expect(r.out).toContain("SC2086");
});

test("shellcheck failing to check at all is named as that, not as findings", () => {
  // shellcheck's own error exits (2+: a file it could not process, bad options)
  // are rare with the real binary, so a stand-in at the pinned version plays one.
  write("scripts/clean.sh", FORMATTED);
  const home = join(dir, "home");
  write("home/.local/bin/shellcheck", `#!/bin/sh\ncase "$1" in --version) printf 'ShellCheck\\nversion: ${pin("shellcheck")}\\n' ;; *) echo "shellcheck: cannot open" >&2; exit 2 ;; esac\n`, 0o755);
  const r = shell(["lint", "scripts/clean.sh"], { HOME: home });
  expect(r.code).toBe(1);
  expect(r.out).toContain("shellcheck could not check the files (exit 2, reason above)");
  expect(r.out).not.toContain("fix each finding");
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
  expect(r.out).toContain("discovering the tree's shell scripts failed");
});

test("files refuses an unreadable candidate rather than leave it out of what is checked", () => {
  write("hooks/locked", "#!/usr/bin/env bash\necho hidden\n", 0o000);
  const r = shell(["files"]);
  expect(r.code).toBe(1);
  expect(r.out).toContain("cannot read hooks/locked to see whether it is a shell script");
  expect(r.out).toContain("chmod u+r hooks/locked");
});

test("files fails on a read error rather than mistake it for end of file, and reads a binary file quietly", () => {
  // A binary candidate with NULs and no newline is read, not warned about, and is no script.
  writeFileSync(join(dir, "blob.bin"), Buffer.from([0x23, 0x21, 0x00, 0x2f, 0x62, 0x69, 0x6e, 0x00, 0xff]));
  write("hooks/run", "#!/usr/bin/env bash\necho run\n");
  let r = shell(["files"]);
  expect(r.code, r.out).toBe(0);
  expect(r.stdout.split("\n").filter(Boolean)).toEqual(["hooks/run", "tools/shell/shell.sh"]);
  expect(r.out).not.toContain("warning");
  // A disk error cannot be induced on a scratch file, so a stand-in head plays one
  // for this file and passes every other read to the real head.
  const real = spawnSync("bash", ["-c", "type -P head"], { encoding: "utf8" }).stdout.trim();
  write("stubs/head", `#!/bin/sh\nfor a; do [ "$a" = hooks/run ] && { echo "head: error reading 'hooks/run': Input/output error" >&2; exit 1; }; done\nexec ${real} "$@"\n`, 0o755);
  r = shell(["files"], { PATH: `${join(dir, "stubs")}:${process.env.PATH}` });
  expect(r.code).toBe(1);
  expect(r.out).toContain("Input/output error");
  expect(r.out).toContain("shell: reading hooks/run failed (reason above), so whether it is a shell script is unknown");
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

test("without HOME the step is refused by name, not with an unbound-variable error", () => {
  write("scripts/good.sh", FORMATTED);
  // `undefined` drops the variable from the child's environment.
  for (const HOME of [undefined, ""]) {
    const r = shell(["format", "scripts/good.sh"], { HOME });
    expect(r.code, JSON.stringify(HOME)).toBe(1);
    expect(r.out).toContain("shell: HOME is unset or empty; set it to your home directory");
    expect(r.out).not.toContain("unbound variable");
  }
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

test("an operand shaped like an option is refused, so it cannot reach the tool as a flag", () => {
  write("scripts/bad.sh", UNFORMATTED);
  for (const args of [["format", "--version"], ["format", "scripts/bad.sh", "-ln=posix"], ["format", "--write", "-i0"], ["lint", "--version"], ["lint", "-e", "SC2086", "scripts/bad.sh"]]) {
    const r = shell(args);
    expect(r.code, JSON.stringify(args)).toBe(2);
    expect(r.out, JSON.stringify(args)).toContain("is not a file to check (it starts with '-')");
  }
  // The same name as a path is checked, and fails on its findings.
  write("-x.sh", UNFORMATTED);
  expect(shell(["format", "./-x.sh"]).code).toBe(1);
});

test("a malformed invocation is a usage error, not a pass", () => {
  for (const args of [[], ["typo"], ["format"], ["format", "--write"], ["lint"], ["files", "x"], ["versions", "x"]]) {
    const r = shell(args);
    expect(r.code, JSON.stringify(args)).toBe(2);
    expect(r.out, JSON.stringify(args)).toContain("usage: tools/shell/shell.sh");
  }
});
