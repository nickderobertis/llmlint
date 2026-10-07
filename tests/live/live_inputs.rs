//! The live tier's offline input checks: `live-lib.sh` writes `LL_TIMEOUT`,
//! `LL_MODEL` and the harness id into each scratch project's `llmlint.yml` and
//! `oneharness.toml`, runs `LLMLINT_BIN` and hands `LLMLINT_ONEHARNESS_BIN` to
//! llmlint, so each is refused by name before any paid call — and the library
//! sets its own strict mode whoever sources it. Each journey sources the real
//! library the way `live-claude.sh` does and calls its real functions, with a
//! PATH of only the base tools it uses (plus stand-in `jq`/`oneharness`
//! executables where a journey must get past those presence checks). No harness
//! is ever run: this is the `live` project's `test` target; the paid journeys
//! are its `live` target.

use std::fs;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::{symlink, PermissionsExt};
#[cfg(unix)]
use std::process::{Command, Output};

#[cfg(unix)]
use tempfile::TempDir;

fn lib_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("live-lib.sh")
}

#[cfg(unix)]
/// The first `name` on the test's own PATH.
fn which(name: &str) -> PathBuf {
    std::env::var_os("PATH")
        .and_then(|path| {
            std::env::split_paths(&path)
                .map(|dir| dir.join(name))
                .find(|p| p.is_file())
        })
        .unwrap_or_else(|| panic!("{name} is not on PATH"))
}

#[cfg(unix)]
/// A clean environment for sourcing the library: a PATH of only the base tools
/// it runs, a scratch TMPDIR (so a refused constructor can be seen to create
/// nothing), and none of the caller's variables.
struct Sandbox {
    dir: TempDir,
}

#[cfg(unix)]
impl Sandbox {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("bin")).unwrap();
        fs::create_dir_all(dir.path().join("tmp")).unwrap();
        for tool in ["dirname", "mktemp", "mkdir", "rm", "cat"] {
            symlink(which(tool), dir.path().join("bin").join(tool)).unwrap();
        }
        Sandbox { dir }
    }

    /// An executable stand-in named `name` on the sandbox PATH.
    fn stub(&self, name: &str) -> PathBuf {
        let path = self.dir.path().join("bin").join(name);
        fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn tmp(&self) -> PathBuf {
        self.dir.path().join("tmp")
    }

    /// Source the real library, then run `script` (the library's functions are
    /// in scope) under `env`.
    fn run(&self, script: &str, env: &[(&str, &str)]) -> Output {
        let mut cmd = Command::new(which("bash"));
        cmd.env_clear()
            .env("PATH", self.dir.path().join("bin"))
            .env("HOME", self.dir.path())
            .env("TMPDIR", self.tmp())
            .arg("-c")
            .arg(format!("source \"$1\"\n{script}"))
            .arg("live-inputs")
            .arg(lib_path());
        for (name, value) in env {
            cmd.env(name, value);
        }
        cmd.output().unwrap()
    }

    fn scratch_projects(&self) -> usize {
        fs::read_dir(self.tmp()).unwrap().count()
    }
}

#[cfg(unix)]
fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[cfg(unix)]
/// The run failed with the library's `FAIL:` line naming `needle`.
fn assert_refused(out: &Output, needle: &str, case: &str) {
    let err = stderr(out);
    assert_eq!(out.status.code(), Some(1), "{case}: {out:?}");
    assert!(
        err.contains("FAIL: ") && err.contains(needle),
        "{case}: {err}"
    );
}

#[cfg(unix)]
#[test]
fn a_bad_timeout_or_model_is_refused_before_any_journey_runs() {
    let sb = Sandbox::new();
    for (var, bad) in [
        ("LL_TIMEOUT", "abc"),
        ("LL_TIMEOUT", "0"),
        ("LL_TIMEOUT", "-5"),
        ("LL_TIMEOUT", "1e3"),
        ("LL_TIMEOUT", "86401"),
        ("LL_TIMEOUT", "30\n  model: injected"),
        ("LL_MODEL", "haiku\"\n  timeout: 1"),
        ("LL_MODEL", "two words"),
        ("LL_MODEL", "{flow: map}"),
    ] {
        let out = sb.run("live_run_journeys claude-code", &[(var, bad)]);
        assert_refused(&out, &format!("{var} must"), &format!("{var}={bad:?}"));
        // Refused before the tool checks, so no stand-in was needed to get here.
        assert!(
            !stderr(&out).contains("jq"),
            "{var}={bad:?}: {}",
            stderr(&out)
        );
    }
    // An admitted pair gets past validation to the next step, the jq check.
    let out = sb.run(
        "live_run_journeys claude-code",
        &[
            ("LL_TIMEOUT", "86400"),
            ("LL_MODEL", "claude-haiku-4.5@2026/x:y_z"),
        ],
    );
    assert_refused(&out, "required tool not found on PATH: jq", "admitted");
}

#[cfg(unix)]
#[test]
fn both_project_constructors_refuse_bad_settings_and_harness_ids_without_scaffolding() {
    let sb = Sandbox::new();
    for constructor in ["make_project", "make_fallback_project"] {
        for (script, env, needle) in [
            (
                "claude-code",
                vec![("LL_TIMEOUT", "soon")],
                "LL_TIMEOUT must",
            ),
            ("claude-code", vec![("LL_MODEL", "x\"y")], "LL_MODEL must"),
            ("'claude-code\"]'", vec![], "harness id must"),
            ("'Claude Code'", vec![], "harness id must"),
            ("''", vec![], "harness id must"),
        ] {
            let out = sb.run(&format!("{constructor} {script}"), &env);
            assert_refused(&out, needle, &format!("{constructor} {script} {env:?}"));
            assert_eq!(
                sb.scratch_projects(),
                0,
                "{constructor} {script}: scaffolded a project before refusing"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn admitted_settings_are_written_quoted_into_the_scratch_configs() {
    let sb = Sandbox::new();
    let env = [("LL_TIMEOUT", "45"), ("LL_MODEL", "1.5")];
    let out = sb.run("make_project claude-code", &env);
    assert!(out.status.success(), "{out:?}");
    let proj = PathBuf::from(String::from_utf8(out.stdout).unwrap());
    let yml = fs::read_to_string(proj.join("llmlint.yml")).unwrap();
    assert!(yml.contains("  timeout: 45\n"), "{yml}");
    // Quoted, so YAML reads a model id that looks like a number as a string.
    assert!(yml.contains("  model: \"1.5\"\n"), "{yml}");
    assert!(yml.contains("    harness: claude-code\n"), "{yml}");

    let out = sb.run("make_fallback_project claude-code", &env);
    assert!(out.status.success(), "{out:?}");
    let proj = PathBuf::from(String::from_utf8(out.stdout).unwrap());
    let toml = fs::read_to_string(proj.join("oneharness.toml")).unwrap();
    assert!(
        toml.contains("harnesses = [\"codex\", \"claude-code\"]"),
        "{toml}"
    );
}

#[cfg(unix)]
#[test]
fn a_binary_override_that_is_not_an_executable_is_refused_by_name() {
    let sb = Sandbox::new();
    sb.stub("jq");
    sb.stub("oneharness");
    let missing = sb.dir.path().join("absent/llmlint");
    let missing = missing.to_str().unwrap();
    let directory = sb.tmp();
    let directory = directory.to_str().unwrap();
    for bad in [missing, directory] {
        // A oneharness on PATH does not paper over a bad override llmlint reads.
        let out = sb.run(
            "live_run_journeys claude-code",
            &[("LLMLINT_ONEHARNESS_BIN", bad)],
        );
        assert_refused(&out, "LLMLINT_ONEHARNESS_BIN must", bad);
        let out = sb.run("live_run_journeys claude-code", &[("LLMLINT_BIN", bad)]);
        assert_refused(&out, "LLMLINT_BIN must", bad);
        assert!(
            !stderr(&out).contains("== llmlint live e2e"),
            "{bad}: a journey started: {}",
            stderr(&out)
        );
    }
    // Admitted overrides resolve to themselves.
    let llmlint = sb.stub("llmlint-under-test");
    let oneharness = sb.dir.path().join("bin/oneharness");
    let out = sb.run(
        "require_oneharness && ll_bin",
        &[
            ("LLMLINT_BIN", llmlint.to_str().unwrap()),
            ("LLMLINT_ONEHARNESS_BIN", oneharness.to_str().unwrap()),
        ],
    );
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        llmlint.to_str().unwrap()
    );
}

#[cfg(unix)]
#[test]
fn the_library_sets_its_own_strict_mode() {
    // Sourced from a shell with every strict option off, the library turns them
    // on itself: errexit, nounset and pipefail.
    let sb = Sandbox::new();
    let mut cmd = Command::new(which("bash"));
    let out = cmd
        .env_clear()
        .env("PATH", sb.dir.path().join("bin"))
        .env("HOME", sb.dir.path())
        .arg("-c")
        .arg(
            "set +e +u +o pipefail\nsource \"$1\"\n\
             [[ $- == *e* && $- == *u* ]] && shopt -qo pipefail && echo strict",
        )
        .arg("live-inputs")
        .arg(lib_path())
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "strict\n", "{out:?}");
    // And it holds: an unset variable aborts the sourcing shell.
    let out = sb.run("echo \"$NOT_SET\"\necho reached", &[]);
    assert_ne!(out.status.code(), Some(0), "{out:?}");
    assert!(
        stderr(&out).contains("NOT_SET: unbound variable"),
        "{out:?}"
    );
    assert!(!String::from_utf8_lossy(&out.stdout).contains("reached"));
}

/// Portable (it reads the library, so it also holds on the Windows `cross` job,
/// where the journeys above do not run): strict mode is set before any command
/// the library runs, and every variable it writes into a scratch config is one
/// it validates by name.
#[test]
fn strict_mode_comes_first_and_every_written_setting_is_validated() {
    let text = fs::read_to_string(lib_path()).unwrap();
    let first_command = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .unwrap();
    assert_eq!(first_command, "set -euo pipefail");
    for var in [
        "LL_TIMEOUT",
        "LL_MODEL",
        "LLMLINT_BIN",
        "LLMLINT_ONEHARNESS_BIN",
    ] {
        assert!(
            text.contains(&format!("fail \"{var} must")),
            "{var} is never refused by name"
        );
    }
}
