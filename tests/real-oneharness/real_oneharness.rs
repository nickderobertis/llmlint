//! The real-oneharness tier: proof that the `--config` layers llmlint forwards
//! mean what llmlint intends once the **released** oneharness reads them.
//!
//! The hermetic e2e suite can only double oneharness, so it proves the argv
//! (which files, in which order) but not that a later file's settings win. Here
//! the real `llmlint` binary runs against the mock oneharness — which records
//! the argv it was handed — and that recorded `--config` list is then given,
//! verbatim and from the same working directory, to the real `oneharness config
//! --format json`, oneharness's own resolver of the layered configuration. Each
//! test asserts the resolved `mode` is the one the highest layer set, attributed
//! to that file. No model is called and nothing is billed.
//!
//! The tests are `#[ignore]`-d because they need the released binary; run them
//! with `just test-oneharness`, which installs `oneharness-cli` at the justfile's
//! `oneharness-cli-version` pin and passes its path as
//! `LLMLINT_REAL_ONEHARNESS`. The pin is the multi-file floor
//! ([`LAYERED_CONFIG_MIN_VERSION`]) — held there by
//! [`the_tier_pins_the_multi_file_floor`], which runs everywhere — so the tier
//! proves the oldest oneharness llmlint lets layer several files actually does.

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::cargo::cargo_bin;
use assert_cmd::Command;
use llmlint::io::oneharness::LAYERED_CONFIG_MIN_VERSION;
use serde_json::Value;
use tempfile::TempDir;

const RULE: &str = "Code has no TODO comments.";

/// Environment prefixes that steer llmlint or oneharness. Both binaries read
/// their own, so every command here starts without any inherited one — an
/// exported `ONEHARNESS_MODE` would otherwise outrank every file under test.
const STEERING_PREFIXES: [&str; 2] = ["LLMLINT_", "ONEHARNESS_"];

fn clear_steering_env(cmd: &mut Command) {
    for (name, _) in std::env::vars_os() {
        let key = name.to_string_lossy();
        if STEERING_PREFIXES.iter().any(|p| key.starts_with(p)) {
            cmd.env_remove(&name);
        }
    }
}

fn floor() -> String {
    let (major, minor, patch) = LAYERED_CONFIG_MIN_VERSION;
    format!("{major}.{minor}.{patch}")
}

/// The released oneharness `just test-oneharness` installed. A tier asked to
/// run without it is a hard failure, never a skip.
fn real_oneharness() -> PathBuf {
    let bin = std::env::var_os("LLMLINT_REAL_ONEHARNESS").unwrap_or_else(|| {
        panic!("LLMLINT_REAL_ONEHARNESS is unset; run this tier via `just test-oneharness`")
    });
    let bin = PathBuf::from(bin);
    let out = std::process::Command::new(&bin)
        .arg("--version")
        .output()
        .unwrap_or_else(|e| panic!("running {} --version: {e}", bin.display()));
    let version = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        version.trim(),
        format!("oneharness {}", floor()),
        "the tier must run the multi-file floor release"
    );
    bin
}

struct Project {
    dir: TempDir,
}

impl Project {
    fn new() -> Self {
        Project {
            dir: TempDir::new().unwrap(),
        }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn write(&self, rel: &str, contents: &str) {
        let p = self.path().join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, contents).unwrap();
    }

    /// A oneharness config file at `rel` that sets only `mode`.
    fn mode_file(&self, rel: &str, mode: &str) {
        self.write(rel, &format!("mode = \"{mode}\"\n"));
    }

    /// Run `llmlint lint` from `cwd` (relative to the project) against the mock
    /// oneharness, then return the `--config` values it forwarded, in order.
    /// `extra` adds flags; `env` adds environment variables.
    fn forwarded_configs(&self, cwd: &str, extra: &[&str], env: &[(&str, &str)]) -> Vec<String> {
        let dump = self.path().join("args.txt");
        let verdicts = self.path().join("verdicts.json");
        fs::write(&verdicts, r#"{"oh_rule": true}"#).unwrap();
        let mut cmd = Command::cargo_bin("llmlint").unwrap();
        clear_steering_env(&mut cmd);
        cmd.current_dir(self.path().join(cwd))
            .env("LLMLINT_HISTORY_DIR", self.path().join(".llmlint-history"))
            .arg("--oneharness-bin")
            .arg(cargo_bin("llmlint-mock-oneharness"))
            .args(extra)
            .env("LLMLINT_MOCK_VERSION", floor())
            .env("LLMLINT_MOCK_VERDICTS", &verdicts)
            .env("LLMLINT_MOCK_DUMP_ARGS", &dump);
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.assert().success();
        let args = fs::read_to_string(&dump).expect("the mock ran and recorded its argv");
        let mut lines = args.lines();
        let mut out = Vec::new();
        while lines.by_ref().any(|l| l == "--config") {
            out.push(lines.next().expect("--config takes a value").to_string());
        }
        out
    }

    /// Resolve `configs` with the real `oneharness config --format json` from
    /// `cwd`, exactly as the `oneharness run` llmlint spawns there would load
    /// them, and return the report.
    fn resolve(&self, cwd: &str, configs: &[String]) -> Value {
        let mut cmd = Command::new(real_oneharness());
        clear_steering_env(&mut cmd);
        cmd.current_dir(self.path().join(cwd))
            .args(["config", "--format", "json"]);
        for c in configs {
            cmd.arg("--config").arg(c);
        }
        let out = cmd.assert().success().get_output().stdout.clone();
        serde_json::from_slice(&out).expect("oneharness config --format json is JSON")
    }

    /// Forward from `cwd`, resolve with the real oneharness, and assert the
    /// forwarded layers are exactly `expected` and that the winning `mode` is
    /// `mode`, attributed to `source`.
    fn assert_layers(
        &self,
        cwd: &str,
        extra: &[&str],
        env: &[(&str, &str)],
        expected: &[&str],
        mode: &str,
        source: &str,
    ) {
        let configs = self.forwarded_configs(cwd, extra, env);
        assert_eq!(configs, expected, "the --config layers llmlint forwarded");
        let report = self.resolve(cwd, &configs);
        let files: Vec<&str> = report["config_files"]
            .as_array()
            .expect("config_files is a list")
            .iter()
            .map(|f| f.as_str().expect("config file path"))
            .collect();
        assert_eq!(
            files, expected,
            "oneharness loaded exactly the forwarded files"
        );
        assert_eq!(report["mode"]["value"], mode, "{report:#}");
        assert_eq!(report["mode"]["source"], source, "{report:#}");
    }
}

/// An llmlint config at `rel` whose `oneharness.config` is `configs` (a YAML
/// flow list), over one rule the mock passes.
fn llmlint_config(p: &Project, rel: &str, configs: &str, extra: &str) {
    p.write(
        rel,
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\n{extra}oneharness:\n  config: {configs}\n\
             rules:\n  - {{ name: oh_rule, description: \"{RULE}\" }}\n"
        ),
    );
}

#[test]
fn the_tier_pins_the_multi_file_floor() {
    // `just test-oneharness` installs `oneharness-cli-version`; it must be the
    // floor llmlint enforces, or the tier proves a different release than the
    // one llmlint lets layer several files.
    let justfile = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../justfile"));
    let expected = format!("oneharness-cli-version := \"{}\"", floor());
    assert!(
        justfile.lines().any(|l| l == expected),
        "the justfile must pin `{expected}` to match LAYERED_CONFIG_MIN_VERSION"
    );
}

#[test]
#[ignore = "needs the released oneharness; run via `just test-oneharness`"]
fn the_later_configured_file_wins() {
    let p = Project::new();
    p.write("src/lib.rs", "// code\n");
    p.mode_file("a.toml", "read-only");
    p.mode_file("b.toml", "plan");
    llmlint_config(&p, "llmlint.yml", r#"["a.toml", "b.toml"]"#, "");
    p.assert_layers(".", &[], &[], &["a.toml", "b.toml"], "plan", "b.toml");
}

#[test]
#[ignore = "needs the released oneharness; run via `just test-oneharness`"]
fn the_repository_config_wins_over_its_plugin() {
    let p = Project::new();
    p.write("src/lib.rs", "// code\n");
    p.mode_file("p.toml", "plan");
    p.mode_file("r.toml", "edit");
    p.write(
        "team.yml",
        "oneharness:\n  config: [\"p.toml\"]\nrules: []\n",
    );
    llmlint_config(
        &p,
        "llmlint.yml",
        r#"["r.toml"]"#,
        "plugins:\n  - ./team.yml\n",
    );
    p.assert_layers(".", &[], &[], &["p.toml", "r.toml"], "edit", "r.toml");
}

#[test]
#[ignore = "needs the released oneharness; run via `just test-oneharness`"]
fn the_nearest_nested_config_wins() {
    // The ancestor lists `shared.toml` after its own `root.toml`; the nearer
    // config lists it too, so it is forwarded once, at the nearer config's
    // position — beneath that config's `leaf.toml`, above the ancestor's file.
    let p = Project::new();
    p.write("sub/src/lib.rs", "// code\n");
    p.mode_file("sub/root.toml", "read-only");
    p.mode_file("sub/shared.toml", "default");
    p.mode_file("sub/leaf.toml", "edit");
    p.write(
        "llmlint.yml",
        "version: 1\noneharness:\n  config: [\"root.toml\", \"shared.toml\"]\n",
    );
    llmlint_config(&p, "sub/llmlint.yml", r#"["shared.toml", "leaf.toml"]"#, "");
    p.assert_layers(
        "sub",
        &[],
        &[],
        &["root.toml", "shared.toml", "leaf.toml"],
        "edit",
        "leaf.toml",
    );
}

#[test]
#[ignore = "needs the released oneharness; run via `just test-oneharness`"]
fn the_env_paths_win_over_the_config_files() {
    let p = Project::new();
    p.write("src/lib.rs", "// code\n");
    p.mode_file("a.toml", "read-only");
    p.mode_file("e1.toml", "plan");
    p.mode_file("e2.toml", "auto");
    llmlint_config(&p, "llmlint.yml", r#"["a.toml"]"#, "");
    let env_list = std::env::join_paths(["e1.toml", "e2.toml"]).unwrap();
    p.assert_layers(
        ".",
        &[],
        &[("LLMLINT_ONEHARNESS_CONFIG", env_list.to_str().unwrap())],
        &["a.toml", "e1.toml", "e2.toml"],
        "auto",
        "e2.toml",
    );
}

#[test]
#[ignore = "needs the released oneharness; run via `just test-oneharness`"]
fn the_flag_wins_over_env_and_config() {
    let p = Project::new();
    p.write("src/lib.rs", "// code\n");
    p.mode_file("a.toml", "read-only");
    p.mode_file("e.toml", "plan");
    p.mode_file("c.toml", "edit");
    llmlint_config(&p, "llmlint.yml", r#"["a.toml"]"#, "");
    p.assert_layers(
        ".",
        &["--oneharness-config", "c.toml"],
        &[("LLMLINT_ONEHARNESS_CONFIG", "e.toml")],
        &["a.toml", "e.toml", "c.toml"],
        "edit",
        "c.toml",
    );
}

/// `install-oneharness.sh` turns the pin into a directory it deletes and
/// rebuilds, so a pin that is not a plain release version — a path, a range, a
/// word — is refused before anything is created or fetched. Offline: the script
/// stops at the pin check, ahead of any PyPI call.
#[cfg(unix)]
#[test]
fn the_installer_refuses_a_pin_that_is_not_a_release_version() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("install-oneharness.sh");
    for pin in ["../../elsewhere", "0.14.0/x", ">=0.14", "latest", "0.14.0 "] {
        let root = TempDir::new().unwrap();
        let dir = root.path().join("tests/real-oneharness");
        fs::create_dir_all(&dir).unwrap();
        fs::copy(&script, dir.join("install-oneharness.sh")).unwrap();
        fs::write(
            root.path().join("justfile"),
            format!("oneharness-cli-version := \"{pin}\"\n"),
        )
        .unwrap();
        let out = std::process::Command::new("bash")
            .arg(dir.join("install-oneharness.sh"))
            .output()
            .unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{pin}: {out:?}");
        assert!(err.contains("not a release version"), "{pin}: {err}");
        assert!(!root.path().join(".dev").exists(), "{pin}: nothing created");
    }
}
