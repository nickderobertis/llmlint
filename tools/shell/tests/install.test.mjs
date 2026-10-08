// tools/shell/install-shell-tools.sh, the way `just bootstrap` runs it, with real
// curl/tar/install/sha256sum: only what a test cannot own is stood in — the host
// (`uname`), the two release servers (a local release tree over file://, with the
// digest pin file to match), and so the tools themselves (stand-ins that report
// the pinned version, as the real ones do).
import { afterEach, beforeEach, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const REPO = resolve(import.meta.dir, "../../..");
const pin = (tool) => readFileSync(join(REPO, "justfile"), "utf8").match(new RegExp(`^${tool}-version := "([^"]+)"`, "m"))[1];
const SHFMT = pin("shfmt");
const SHELLCHECK = pin("shellcheck");
let dir;

function write(path, text, mode) {
  mkdirSync(join(dir, path, ".."), { recursive: true });
  writeFileSync(join(dir, path), text);
  if (mode) chmodSync(join(dir, path), mode);
}

const sha = (path) => createHash("sha256").update(readFileSync(path)).digest("hex");

// A release tree for every asset the script can choose, and a pin file naming
// each one's digest.
function releases() {
  const sums = [];
  for (const [os, arch] of [["linux", "amd64"], ["linux", "arm64"], ["darwin", "amd64"], ["darwin", "arm64"]]) {
    const asset = `shfmt_v${SHFMT}_${os}_${arch}`;
    write(`rel/sh/v${SHFMT}/${asset}`, `#!/bin/sh\necho v${SHFMT}\necho ${os}/${arch} >&2\n`);
    sums.push(`${sha(join(dir, `rel/sh/v${SHFMT}/${asset}`))}  ${asset}`);
  }
  for (const [os, arch] of [["linux", "x86_64"], ["linux", "aarch64"], ["darwin", "x86_64"], ["darwin", "aarch64"]]) {
    const asset = `shellcheck-v${SHELLCHECK}.${os}.${arch}.tar.gz`;
    write(`pkg/shellcheck-v${SHELLCHECK}/shellcheck`, `#!/bin/sh\necho "version: ${SHELLCHECK}"\necho ${os}/${arch} >&2\n`, 0o755);
    mkdirSync(join(dir, `rel/sc/v${SHELLCHECK}`), { recursive: true });
    const tar = spawnSync("tar", ["-czf", join(dir, `rel/sc/v${SHELLCHECK}/${asset}`), "-C", join(dir, "pkg"), `shellcheck-v${SHELLCHECK}`]);
    if (tar.status !== 0) throw new Error(`tar: ${tar.stderr}`);
    sums.push(`${sha(join(dir, `rel/sc/v${SHELLCHECK}/${asset}`))}  ${asset}`);
  }
  write("sums", `# stand-in pins\n${sums.join("\n")}\n`);
}

function install(env = {}, { os = "Linux", arch = "x86_64" } = {}) {
  write("stubs/uname", `#!/bin/sh\ncase "$1" in -s) echo ${os} ;; *) echo ${arch} ;; esac\n`, 0o755);
  const r = spawnSync("bash", [join(dir, "tools/shell/install-shell-tools.sh")], {
    cwd: dir,
    encoding: "utf8",
    env: {
      ...process.env,
      PATH: `${join(dir, "stubs")}:${process.env.PATH}`,
      HOME: join(dir, "home"),
      LLMLINT_SHELL_TOOLS: "on",
      SHFMT_BASE_URL: `file://${join(dir, "rel/sh")}`,
      SHELLCHECK_BASE_URL: `file://${join(dir, "rel/sc")}`,
      SHELL_TOOLS_INSTALL_DIR: join(dir, "bin"),
      SHELL_TOOLS_SHA256_FILE: join(dir, "sums"),
      ...env,
    },
  });
  if (r.error) throw r.error;
  return { code: r.status, out: `${r.stdout}${r.stderr}` };
}

// Which stand-in build an installed tool is (each prints its os/arch).
function build(tool) {
  const r = spawnSync(join(dir, "bin", tool), ["--version"], { encoding: "utf8" });
  return { version: r.stdout.trim(), build: r.stderr.trim() };
}

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "llmlint-shell-tools-"));
  mkdirSync(join(dir, "tools/shell"), { recursive: true });
  copyFileSync(join(REPO, "tools/shell/install-shell-tools.sh"), join(dir, "tools/shell/install-shell-tools.sh"));
  copyFileSync(join(REPO, "justfile"), join(dir, "justfile"));
  releases();
});

afterEach(() => rmSync(dir, { recursive: true, force: true }));

test("installs the pinned shfmt and shellcheck built for each supported host", () => {
  for (const [os, arch, goBuild, scBuild] of [
    ["Linux", "x86_64", "linux/amd64", "linux/x86_64"],
    ["Linux", "aarch64", "linux/arm64", "linux/aarch64"],
    ["Darwin", "arm64", "darwin/arm64", "darwin/aarch64"],
    ["Darwin", "amd64", "darwin/amd64", "darwin/x86_64"],
  ]) {
    rmSync(join(dir, "bin"), { recursive: true, force: true });
    const r = install({}, { os, arch });
    expect(r.code, `${os}/${arch}: ${r.out}`).toBe(0);
    // One summary line, naming what each tool needed.
    expect(r.out.trim().split("\n")).toEqual([
      `install-shell-tools: shfmt ${SHFMT} (installed for ${goBuild}), shellcheck ${SHELLCHECK} (installed for ${scBuild}) in ${join(dir, "bin")}`,
    ]);
    expect(build("shfmt")).toEqual({ version: `v${SHFMT}`, build: goBuild });
    expect(build("shellcheck")).toEqual({ version: `version: ${SHELLCHECK}`, build: scBuild });
  }
});

test("a second run keeps the pinned tools in place without fetching anything", () => {
  expect(install().code).toBe(0);
  rmSync(join(dir, "rel"), { recursive: true, force: true });
  const r = install();
  expect(r.code, r.out).toBe(0);
  expect(r.out).toContain(`shfmt ${SHFMT} (already there), shellcheck ${SHELLCHECK} (already there)`);
});

test("a tool off its pin in the install dir is replaced by the pinned one", () => {
  write("bin/shfmt", "#!/bin/sh\necho v0.0.1\n", 0o755);
  const r = install();
  expect(r.code, r.out).toBe(0);
  expect(build("shfmt").version).toBe(`v${SHFMT}`);
});

test("a download that does not match its pinned digest is refused and nothing is installed", () => {
  write(`rel/sh/v${SHFMT}/shfmt_v${SHFMT}_linux_amd64`, "#!/bin/sh\necho tampered\n");
  const r = install();
  expect(r.code).toBe(1);
  expect(r.out).toContain(`sha256 mismatch for shfmt_v${SHFMT}_linux_amd64 — NOT installing`);
  expect(existsSync(join(dir, "bin/shfmt"))).toBe(false);
});

test("an asset with no pinned digest, or a missing pin file, is refused", () => {
  writeFileSync(join(dir, "sums"), readFileSync(join(dir, "sums"), "utf8").replace(/^.*shellcheck.*linux\.x86_64.*$/m, ""));
  let r = install();
  expect(r.code).toBe(1);
  expect(r.out).toContain(`no pinned sha256 for shellcheck-v${SHELLCHECK}.linux.x86_64.tar.gz`);
  r = install({ SHELL_TOOLS_SHA256_FILE: join(dir, "absent") });
  expect(r.code).toBe(1);
  expect(r.out).toContain("no readable digest pin file");
});

test("an archive without the expected layout is refused after its digest matches", () => {
  const asset = `shellcheck-v${SHELLCHECK}.linux.x86_64.tar.gz`;
  write("other/README", "not shellcheck\n");
  spawnSync("tar", ["-czf", join(dir, `rel/sc/v${SHELLCHECK}/${asset}`), "-C", join(dir, "other"), "README"]);
  writeFileSync(
    join(dir, "sums"),
    readFileSync(join(dir, "sums"), "utf8").replace(new RegExp(`^\\w+  ${asset.replace(/\./g, "\\.")}$`, "m"), `${sha(join(dir, `rel/sc/v${SHELLCHECK}/${asset}`))}  ${asset}`),
  );
  const r = install();
  expect(r.code).toBe(1);
  expect(r.out).toContain(`holds no shellcheck-v${SHELLCHECK}/shellcheck`);
});

test("an archive that matches its digest but will not unpack is refused, and its tool is not installed", () => {
  const asset = `shellcheck-v${SHELLCHECK}.linux.x86_64.tar.gz`;
  write(`rel/sc/v${SHELLCHECK}/${asset}`, "not a gzip archive\n");
  writeFileSync(
    join(dir, "sums"),
    readFileSync(join(dir, "sums"), "utf8").replace(new RegExp(`^\\w+  ${asset.replace(/\./g, "\\.")}$`, "m"), `${sha(join(dir, `rel/sc/v${SHELLCHECK}/${asset}`))}  ${asset}`),
  );
  const r = install();
  expect(r.code).toBe(1);
  expect(r.out).toContain(`${asset} matched its pinned digest but did not unpack`);
  expect(existsSync(join(dir, "bin/shellcheck"))).toBe(false);
});

test("a hash tool that fails refuses the download rather than trust it", () => {
  write("stubs/sha256sum", "#!/bin/sh\nexit 1\n", 0o755);
  const r = install();
  expect(r.code).toBe(1);
  expect(r.out).toContain(`sha256sum could not hash the downloaded shfmt_v${SHFMT}_linux_amd64 — NOT installing`);
  expect(existsSync(join(dir, "bin/shfmt"))).toBe(false);
});

test("an install dir that cannot be written, or no scratch space, is a failure naming the fix", () => {
  mkdirSync(join(dir, "locked"));
  chmodSync(join(dir, "locked"), 0o555);
  let r = install({ SHELL_TOOLS_INSTALL_DIR: join(dir, "locked/bin") });
  chmodSync(join(dir, "locked"), 0o755);
  expect(r.code).toBe(1);
  expect(r.out).toContain(`could not install shfmt into ${join(dir, "locked/bin")}`);
  expect(r.out).toContain("point SHELL_TOOLS_INSTALL_DIR at a writable directory");
  r = install({ TMPDIR: join(dir, "no-such-tmp") });
  expect(r.code).toBe(1);
  expect(r.out).toContain("could not create a temporary directory");
});

// A PATH of only the named host tools (and the uname stand-in), so which hash
// tool the installer finds is up to the journey.
function sandboxPath(tools) {
  const bin = join(dir, "sandbox");
  mkdirSync(bin, { recursive: true });
  for (const t of tools) {
    const real = spawnSync("bash", ["-c", `type -P ${t}`], { encoding: "utf8" }).stdout.trim();
    if (!real) throw new Error(`${t} is not on this host's PATH`);
    symlinkSync(real, join(bin, t));
  }
  return `${join(dir, "stubs")}:${bin}`;
}
const BASE = ["bash", "curl", "tar", "gzip", "install", "awk", "grep", "head", "cut", "sed", "mkdir", "rm", "mktemp", "dirname", "cat"];

test("without sha256sum the installer verifies with shasum, and with neither it refuses to install", () => {
  // shasum is a host tool the installer only falls back to; a stand-in that
  // hashes with the real sha256sum by absolute path plays it where it is absent.
  const real = spawnSync("bash", ["-c", "type -P sha256sum"], { encoding: "utf8" }).stdout.trim();
  write("hash/shasum", `#!/bin/sh\n[ "$1" = -a ] && shift 2\nexec ${real} "$@"\n`, 0o755);
  const path = sandboxPath(BASE);
  symlinkSync(join(dir, "hash/shasum"), join(dir, "sandbox/shasum"));
  let r = install({ PATH: path });
  expect(r.code, r.out).toBe(0);
  expect(build("shfmt").version).toBe(`v${SHFMT}`);

  rmSync(join(dir, "bin"), { recursive: true, force: true });
  rmSync(join(dir, "sandbox/shasum"));
  r = install({ PATH: path });
  expect(r.code).toBe(1);
  expect(r.out).toContain("no SHA-256 tool (sha256sum or shasum) on PATH");
  expect(existsSync(join(dir, "bin"))).toBe(false);
});

test("an unsupported host is refused by name", () => {
  let r = install({}, { os: "SunOS" });
  expect(r.code).toBe(1);
  expect(r.out).toContain("no pinned shfmt/shellcheck build for this OS: SunOS");
  r = install({}, { arch: "riscv64" });
  expect(r.code).toBe(1);
  expect(r.out).toContain("no pinned shfmt/shellcheck build for this architecture: riscv64");
});

test("a malformed override or a missing pin is refused before anything is fetched", () => {
  for (const [env, msg] of [
    [{ SHFMT_BASE_URL: "http://example.com" }, "must be https://<host>[/path] or file:///<path>"],
    [{ SHELLCHECK_BASE_URL: "" }, "got: <empty>"],
    [{ SHELL_TOOLS_INSTALL_DIR: "" }, "SHELL_TOOLS_INSTALL_DIR is empty"],
    [{ SHELL_TOOLS_INSTALL_DIR: "-m777" }, "must not start with '-'"],
    [{ LLMLINT_SHELL_TOOLS: "maybe" }, "must be 'on' (the default) or 'off'"],
  ]) {
    const r = install(env);
    expect(r.code, JSON.stringify(env)).not.toBe(0);
    expect(r.out, JSON.stringify(env)).toContain(msg);
  }
  expect(existsSync(join(dir, "bin")), "nothing installed by a refused run").toBe(false);
  writeFileSync(join(dir, "justfile"), `shfmt-version := "${SHFMT}"\n`);
  let r = install();
  expect(r.code).toBe(1);
  expect(r.out).toContain('restore the line: shellcheck-version := "<version>"');
  // A pin lands in a URL and an archive path, so only a release number passes.
  writeFileSync(join(dir, "justfile"), `shfmt-version := "../../x"\nshellcheck-version := "${SHELLCHECK}"\n`);
  r = install();
  expect(r.code).toBe(1);
  expect(r.out).toContain("the shfmt-version pin");
  expect(r.out).toContain("is not a release version (got: ../../x)");
});

test("a host whose uname fails is refused with the fix, not misdetected", () => {
  write("stubs/uname", "#!/bin/sh\nexit 3\n", 0o755);
  const r = spawnSync("bash", [join(dir, "tools/shell/install-shell-tools.sh")], {
    cwd: dir,
    encoding: "utf8",
    env: { ...process.env, PATH: `${join(dir, "stubs")}:${process.env.PATH}`, LLMLINT_SHELL_TOOLS: "on", SHELL_TOOLS_INSTALL_DIR: join(dir, "bin"), SHELL_TOOLS_SHA256_FILE: join(dir, "sums") },
  });
  expect(r.status).toBe(1);
  expect(`${r.stdout}${r.stderr}`).toContain("uname failed, so the host's OS and architecture are unknown");
});

test("the committed digest file pins exactly the assets the installer can choose for the current pins", () => {
  // The drift gate for tools/shell/shell-tools.sha256: the matrix is read from the
  // installer's own case arms and asset names, and the versions from the
  // justfile, so a moved pin or a new platform cannot leave the file stale.
  const script = readFileSync(join(REPO, "tools/shell/install-shell-tools.sh"), "utf8");
  const oses = [...script.matchAll(/^\s+\w+\) os=(\w+) ;;$/gm)].map((m) => m[1]);
  const arches = [...script.matchAll(/^\s+[^)]+\) go_arch=(\w+) sc_arch=(\w+) ;;$/gm)].map((m) => ({ go_arch: m[1], sc_arch: m[2] }));
  const templates = [...script.matchAll(/^\s+asset="([^"]+)"$/gm)].map((m) => m[1]);
  expect(oses).toEqual(["linux", "darwin"]);
  expect(arches.length).toBe(2);
  expect(templates.length).toBe(2);
  const vars = { shfmt_version: SHFMT, shellcheck_version: SHELLCHECK };
  const expected = new Set();
  for (const os of oses) {
    for (const arch of arches) {
      for (const t of templates) {
        expected.add(t.replace(/\$\{(\w+)\}/g, (_, v) => ({ ...vars, os, ...arch })[v]));
      }
    }
  }
  const pinned = readFileSync(join(REPO, "tools/shell/shell-tools.sha256"), "utf8")
    .split("\n")
    .filter((l) => l && !l.startsWith("#"));
  for (const line of pinned) expect(line, "a digest line is <64 hex>  <asset>").toMatch(/^[0-9a-f]{64} {2}\S+$/);
  const names = pinned.map((l) => l.split(/\s+/)[1]);
  expect(names.length, "each asset pinned once").toBe(new Set(names).size);
  expect(new Set(names)).toEqual(expected);
});

test("an unreachable release server is a clear failure, not a silent skip", () => {
  const r = install({ SHFMT_BASE_URL: `file://${join(dir, "nowhere")}` });
  expect(r.code).toBe(1);
  expect(r.out).toContain("could not download");
});

test("LLMLINT_SHELL_TOOLS=off installs nothing, with a notice", () => {
  const r = install({ LLMLINT_SHELL_TOOLS: "off" });
  expect(r.code).toBe(0);
  expect(r.out).toContain("LLMLINT_SHELL_TOOLS=off");
  expect(existsSync(join(dir, "bin"))).toBe(false);
});
