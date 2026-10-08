// The boundary rule against a real (tiny) Cargo workspace: cargo metadata is the
// edge source, so the test builds one rather than feeding canned JSON.
// llmlint: ignore-file[shell_test_tiers_stay_split] these tests are offline and hermetic: they run the boundary checker and Nx's own graph resolution (the gate's orchestrator, installed from bun.lock) and cargo metadata --offline over scratch or the real workspace; nothing reaches a network, and the files they read are this project's inputs, so a separate project would be selected by exactly the same edits
import { afterEach, expect, setDefaultTimeout, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { chmodSync, cpSync, mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const REPO = resolve(import.meta.dir, "../..");

// The tests spawn Nx and cargo metadata, whose cold start on a Windows runner
// comes within reach of bun's 5s default.
setDefaultTimeout(120_000);

/** Run a command; return { code, stdout, stderr }. Never throws on non-zero. */
function run(cmd, args, opts = {}) {
  const r = spawnSync(cmd, args, { encoding: "utf8", ...opts });
  if (r.error) throw r.error;
  return { code: r.status, stdout: r.stdout ?? "", stderr: r.stderr ?? "" };
}

/** A scratch directory removed by the returned cleanup. */
function scratch() {
  const dir = mkdtempSync(join(tmpdir(), "llmlint-tools-"));
  return { dir, cleanup: () => rmSync(dir, { recursive: true, force: true }) };
}

/** A git repo in `dir` (the checker lists project.json files through git). */
function gitRepo(dir) {
  for (const args of [["init", "-q", "-b", "main"], ["config", "user.email", "tests@example.invalid"], ["config", "user.name", "tests"]]) {
    const r = run("git", args, { cwd: dir });
    if (r.code !== 0) throw new Error(`git ${args.join(" ")}: ${r.stderr}`);
  }
}

let cleanups = [];
afterEach(() => {
  for (const c of cleanups) c();
  cleanups = [];
});

function workspace({ appDevDep = "", e2eTags = ["type:e2e"], e2eDep = "", e2eImplicit = ["app"] } = {}) {
  const { dir, cleanup } = scratch();
  cleanups.push(cleanup);
  const write = (path, text) => {
    mkdirSync(join(dir, path, ".."), { recursive: true });
    writeFileSync(join(dir, path), text);
  };
  write(
    "Cargo.toml",
    `[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n\n[dev-dependencies]\n${appDevDep}\n\n[workspace]\nmembers = ["e2e"]\n`,
  );
  write("src/lib.rs", "");
  write("e2e/Cargo.toml", `[package]\nname = "app-e2e"\nversion = "0.1.0"\nedition = "2021"\n\n[dev-dependencies]\n${e2eDep}\n`);
  write("e2e/src/lib.rs", "");
  write("project.json", JSON.stringify({ name: "app", tags: ["type:app"] }));
  write("e2e/project.json", JSON.stringify({ name: "app-e2e", tags: e2eTags, implicitDependencies: e2eImplicit }));
  mkdirSync(join(dir, "tools"), { recursive: true });
  cpSync(join(REPO, "tools/project-boundaries.json"), join(dir, "tools/project-boundaries.json"));
  gitRepo(dir);
  return dir;
}

const check = (dir, env = process.env) => run("bun", [join(REPO, "tools/check-project-boundaries.mjs"), "--root", dir], { env });

test("allowed edges pass quietly", () => {
  const r = check(workspace());
  expect(r.stderr).toBe("");
  expect(r.code).toBe(0);
});

test("the app crate depending on its e2e crate fails, naming both and the edge", () => {
  const r = check(workspace({ appDevDep: 'app-e2e = { path = "e2e" }' }));
  expect(r.code).toBe(1);
  expect(r.stderr).toContain("app (type:app) may not depend on app-e2e (type:e2e)");
  expect(r.stderr).toContain("Cargo dev dependency app-e2e");
});

test("a root reached through a symlink is read the way cargo reports it", () => {
  // macOS's tmpdir is /var/folders/..., which cargo reports as /private/var/...;
  // a symlinked root reproduces that on every platform (a junction on Windows).
  const linked = (dir) => {
    const { dir: holder, cleanup } = scratch();
    cleanups.push(cleanup);
    const link = join(holder, "linked");
    symlinkSync(dir, link, "junction");
    return link;
  };
  const ok = check(linked(workspace()));
  expect(ok.stderr).toBe("");
  expect(ok.code).toBe(0);
  const bad = check(linked(workspace({ appDevDep: 'app-e2e = { path = "e2e" }' })));
  expect(bad.code).toBe(1);
  expect(bad.stderr).toContain("app (type:app) may not depend on app-e2e (type:e2e)");
});

test("a Cargo edge that is not also an Nx edge fails, naming the fix", () => {
  // app-e2e builds against app through Cargo but does not declare it to Nx, so a
  // change to app would not select app-e2e under nx affected.
  const r = check(workspace({ e2eDep: 'app = { path = ".." }', e2eImplicit: [] }));
  expect(r.code).toBe(1);
  expect(r.stderr).toContain('app-e2e has a Cargo dev dependency app on app that is not in its implicitDependencies; add "app" there');
  const mirrored = check(workspace({ e2eDep: 'app = { path = ".." }' }));
  expect(mirrored.stderr).toBe("");
  expect(mirrored.code).toBe(0);
});

test("a project without exactly one type tag fails", () => {
  const r = check(workspace({ e2eTags: [] }));
  expect(r.code).toBe(1);
  expect(r.stderr).toContain("app-e2e must carry exactly one type:* tag");
});

test("the real repository passes", () => {
  const r = check(REPO);
  expect(r.stderr).toBe("");
  expect(r.code).toBe(0);
});

test("implicit dependency globs expand the way Nx reads them", () => {
  const dir = workspace();
  // The e2e project claiming every other project (`*`) reaches app: allowed.
  writeFileSync(join(dir, "e2e/project.json"), JSON.stringify({ name: "app-e2e", tags: ["type:e2e"], implicitDependencies: ["*"] }));
  expect(check(dir).code).toBe(0);
  // The app claiming `*` reaches the e2e project: refused, naming the edge.
  writeFileSync(join(dir, "project.json"), JSON.stringify({ name: "app", tags: ["type:app"], implicitDependencies: ["*", "!nothing"] }));
  const r = check(dir);
  expect(r.code).toBe(1);
  expect(r.stderr).toContain("app (type:app) may not depend on app-e2e (type:e2e) — found implicitDependencies");
});

test("malformed project definitions and policies are refused", () => {
  const dir = workspace();
  writeFileSync(join(dir, "e2e/project.json"), JSON.stringify({ name: "app", tags: ["type:e2e"] }));
  expect(check(dir).stderr).toContain("project name app is declared twice");
  writeFileSync(join(dir, "e2e/project.json"), JSON.stringify({ name: "app-e2e", tags: "type:e2e" }));
  expect(check(dir).stderr).toContain("tags and implicitDependencies must be arrays of strings");
  writeFileSync(join(dir, "tools/project-boundaries.json"), JSON.stringify({ depConstraints: [{ sourceTag: "app" }] }));
  expect(check(dir).stderr).toContain("malformed constraint");
});

test("a project.json holding JSON null, or a bad --root, is refused with a message", () => {
  const dir = workspace();
  writeFileSync(join(dir, "e2e/project.json"), "null");
  expect(check(dir).stderr).toContain("e2e/project.json must hold a JSON object");
  const r = run("bun", [join(REPO, "tools/check-project-boundaries.mjs"), "--root", join(dir, "missing")]);
  expect(r.code).toBe(2);
  expect(r.stderr).toContain("usage:");
});

test("tag: implicit dependencies resolve to the tagged projects", () => {
  const dir = workspace();
  writeFileSync(join(dir, "e2e/project.json"), JSON.stringify({ name: "app-e2e", tags: ["type:e2e", "suite"] }));
  writeFileSync(join(dir, "project.json"), JSON.stringify({ name: "app", tags: ["type:app"], implicitDependencies: ["tag:suite"] }));
  const r = check(dir);
  expect(r.code).toBe(1);
  expect(r.stderr).toContain("app (type:app) may not depend on app-e2e (type:e2e) — found implicitDependencies");
});

/** Make `dir` look Nx-enabled, with `scripts/nx graph` answering `edges` (implicit). */
function stubNxGraph(dir, edges) {
  mkdirSync(join(dir, "node_modules/.bin"), { recursive: true });
  writeFileSync(join(dir, "node_modules/.bin/nx"), "");
  const deps = { app: [], "app-e2e": [] };
  for (const [from, to] of edges) deps[from].push({ source: from, target: to, type: "implicit" });
  mkdirSync(join(dir, "scripts"), { recursive: true });
  writeFileSync(
    join(dir, "scripts/nx"),
    `#!/usr/bin/env bash\nfor a in "$@"; do case "$a" in --file=*) printf '%s' '${JSON.stringify({ graph: { dependencies: deps } })}' > "\${a#--file=}" ;; esac; done\n`,
  );
}

test("the checker's implicit edges must match Nx's resolved graph, both ways", () => {
  const agree = workspace();
  stubNxGraph(agree, [["app-e2e", "app"]]);
  expect(check(agree).code).toBe(0);

  const nxOnly = workspace();
  stubNxGraph(nxOnly, [["app-e2e", "app"], ["app", "app-e2e"]]);
  const r1 = check(nxOnly);
  expect(r1.code).toBe(1);
  expect(r1.stderr).toContain("Nx resolves app -> app-e2e, which this checker did not");

  const checkerOnly = workspace();
  stubNxGraph(checkerOnly, []);
  const r2 = check(checkerOnly);
  expect(r2.code).toBe(1);
  expect(r2.stderr).toContain("this checker resolves app-e2e -> app, which Nx does not");
});

/** Env whose `cargo metadata` answers `edit(real metadata)`: only the far-end tool is replaced. */
function cargoAnswering(dir, edit) {
  const metadata = JSON.parse(run("cargo", ["metadata", "--format-version", "1", "--no-deps", "--offline"], { cwd: dir }).stdout);
  const bin = join(dir, "fake-bin");
  mkdirSync(bin);
  writeFileSync(join(bin, "metadata.json"), JSON.stringify(edit(metadata)));
  writeFileSync(join(bin, "cargo"), `#!/bin/sh\ncat '${join(bin, "metadata.json")}'\n`);
  chmodSync(join(bin, "cargo"), 0o755);
  return { ...process.env, PATH: `${bin}:${process.env.PATH}` };
}

test.skipIf(process.platform === "win32")("cargo metadata whose packages fall short of its workspace members is refused", () => {
  const dir = workspace({ appDevDep: 'app-e2e = { path = "e2e" }' });
  // Dropping the app package would hide its forbidden edge; the checker must refuse, not pass.
  const short = check(dir, cargoAnswering(dir, (m) => ({ ...m, packages: m.packages.filter((p) => p.name !== "app") })));
  expect(short.code).toBe(1);
  expect(short.stderr).toContain("packages do not match its workspace_members");

  const none = workspace();
  const r = check(none, cargoAnswering(none, (m) => ({ ...m, workspace_members: [] })));
  expect(r.code).toBe(1);
  expect(r.stderr).toContain("returned no workspace_members list");
});
