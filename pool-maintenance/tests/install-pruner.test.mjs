// pool-maintenance/install-pruner.sh, the step `just bootstrap` runs, driven
// the way bootstrap drives it. A test cannot let it install a real toolchain or
// crate over the network, so rustup, cargo and the pruner they install are
// stand-ins on a PATH of the base tools: each records the argv it ran with, and
// the stand-in `cargo install` lays down a pruner reporting the version it was
// asked for. Windows is skipped: the stand-ins are shell scripts.
import { afterEach, beforeEach, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, join, resolve } from "node:path";

const REPO = resolve(import.meta.dir, "../..");
const SCRIPT = join(REPO, "pool-maintenance/install-pruner.sh");
const unix = test.skipIf(process.platform === "win32");
let dir, bin, cargoBin, log;

function standIn(path, body) {
  writeFileSync(path, `#!/bin/sh\n${body}`);
  chmodSync(path, 0o755);
}

// A pruner at `path` reporting `version`.
function pruner(path, version) {
  standIn(path, `echo 'cargo-prune-superseded ${version}'\n`);
}

function calls() {
  return existsSync(log) ? readFileSync(log, "utf8").split("\n").filter(Boolean) : [];
}

function install(args, { cargoExit = 0, path } = {}) {
  const env = { ...process.env, PATH: path ?? [bin, cargoBin, "/usr/bin", "/bin"].join(delimiter), CALLS: log, CARGO_BIN: cargoBin, CARGO_EXIT: String(cargoExit) };
  const r = spawnSync("bash", [SCRIPT, ...args], { cwd: dir, encoding: "utf8", env });
  if (r.error) throw r.error;
  return { code: r.status, out: `${r.stdout}${r.stderr}` };
}

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "llmlint-pruner-"));
  bin = join(dir, "bin");
  cargoBin = join(dir, "cargo-bin");
  log = join(dir, "calls.log");
  mkdirSync(bin);
  mkdirSync(cargoBin);
  standIn(join(bin, "rustup"), 'echo "rustup $*" >>"$CALLS"\n');
  // `cargo +<rust> install --locked cargo-prune-superseded --version <v>`: the
  // pruner lands in cargo's bin directory, as the real install does.
  standIn(
    join(bin, "cargo"),
    [
      'echo "cargo $*" >>"$CALLS"',
      '[ "$CARGO_EXIT" = 0 ] || { echo "error: failed to download" >&2; exit "$CARGO_EXIT"; }',
      'printf "#!/bin/sh\\necho \'cargo-prune-superseded %s\'\\n" "$6" >"$CARGO_BIN/cargo-prune-superseded"',
      'chmod +x "$CARGO_BIN/cargo-prune-superseded"',
      "",
    ].join("\n"),
  );
});

afterEach(() => rmSync(dir, { recursive: true, force: true }));

unix("with no pruner on PATH it installs the pinned release with its own toolchain, and a second run installs nothing", () => {
  const first = install(["0.1.1", "1.97.1"]);
  expect(first.code, first.out).toBe(0);
  expect(calls()).toEqual([
    "rustup toolchain install --profile minimal 1.97.1",
    "cargo +1.97.1 install --locked cargo-prune-superseded --version 0.1.1",
  ]);
  const again = install(["0.1.1", "1.97.1"]);
  expect(again.code, again.out).toBe(0);
  expect(calls().length).toBe(2);
});

unix("a pruner already at the pin is left alone", () => {
  pruner(join(cargoBin, "cargo-prune-superseded"), "0.1.1");
  const r = install(["0.1.1", "1.97.1"]);
  expect(r.code, r.out).toBe(0);
  expect(r.out).toBe("");
  expect(calls()).toEqual([]);
});

unix("a pruner off the pin is replaced by the pinned release", () => {
  pruner(join(cargoBin, "cargo-prune-superseded"), "0.0.9");
  const r = install(["0.1.1", "1.97.1"]);
  expect(r.code, r.out).toBe(0);
  expect(calls()).toContain("cargo +1.97.1 install --locked cargo-prune-superseded --version 0.1.1");
  expect(spawnSync(join(cargoBin, "cargo-prune-superseded"), ["--version"], { encoding: "utf8" }).stdout.trim()).toBe(
    "cargo-prune-superseded 0.1.1",
  );
});

unix("a failed install fails bootstrap, naming the fix", () => {
  const r = install(["0.1.1", "1.97.1"], { cargoExit: 101 });
  expect(r.code, r.out).toBe(1);
  expect(r.out).toContain("cargo install of cargo-prune-superseded 0.1.1 failed (above); check the network, then re-run just bootstrap.");
});

unix("an install shadowed by another pruner earlier on PATH fails, naming the fix", () => {
  const earlier = join(dir, "earlier");
  mkdirSync(earlier);
  pruner(join(earlier, "cargo-prune-superseded"), "0.0.9");
  const r = install(["0.1.1", "1.97.1"], { path: [earlier, bin, cargoBin, "/usr/bin", "/bin"].join(delimiter) });
  expect(r.code, r.out).toBe(1);
  expect(r.out).toContain(`installed 0.1.1, but PATH resolves ${join(earlier, "cargo-prune-superseded")} ('cargo-prune-superseded 0.0.9')`);
  expect(r.out).toContain("put cargo's bin directory (~/.cargo/bin) first on PATH.");
});

unix("malformed pins are refused before anything is installed", () => {
  for (const args of [[], ["0.1"], ["0.1.1", "stable"], ["latest", "1.97.1"]]) {
    const r = install(args);
    expect(r.code, `${args}: ${r.out}`).toBe(2);
    expect(r.out).toContain("usage: install-pruner.sh <version> <rust-toolchain>");
  }
  expect(calls()).toEqual([]);
});

unix("just bootstrap installs the justfile's pins", () => {
  const just = readFileSync(join(REPO, "justfile"), "utf8");
  const pin = (name) => just.match(new RegExp(`^${name} := "([^"]+)"`, "m"))[1];
  const r = spawnSync("just", ["--dry-run", "bootstrap"], { cwd: REPO, encoding: "utf8" });
  expect(r.status, r.stderr).toBe(0);
  expect(r.stderr.split("\n")).toContain(
    `bash pool-maintenance/install-pruner.sh ${pin("prune-superseded-version")} ${pin("prune-superseded-rust")}`,
  );
});
