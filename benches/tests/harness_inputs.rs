//! The CLI harness scripts' input checks (`bench.sh`, `bench-instructions.sh`):
//! each environment override reaches a tool flag or a filesystem write, so a bad
//! value must be refused by name before anything is built or measured. Each
//! journey drives the real script the way `just bench-cli` does, with a PATH
//! holding only the tools the scripts need before their own tool check — so a
//! value they admit stops, offline, at "hyperfine/valgrind not found", and
//! nothing is ever compiled or timed. This is the `bench` project's `test`
//! target; the measurements themselves stay its informational `bench*` targets.

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::{Command, Output};

#[cfg(unix)]
use tempfile::TempDir;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate lives one level below the repository root")
        .to_path_buf()
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
/// Run `benches/<script>` with `env` set and a PATH holding only `dirname`
/// (the one tool either script runs before its hyperfine/valgrind check), and
/// every inherited `BENCH_*` override cleared.
fn run(script: &str, env: &[(&str, &str)]) -> (Output, TempDir) {
    let tools = TempDir::new().unwrap();
    symlink(which("dirname"), tools.path().join("dirname")).unwrap();
    let mut cmd = Command::new(which("bash"));
    cmd.arg(repo_root().join("benches").join(script))
        .env("PATH", tools.path());
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("BENCH_") {
            cmd.env_remove(&name);
        }
    }
    // Keep the default output directory out of the repository's target/.
    cmd.env("BENCH_OUT", tools.path().join("out"));
    for (name, value) in env {
        cmd.env(name, value);
    }
    (cmd.output().unwrap(), tools)
}

#[cfg(unix)]
fn assert_refused(out: &Output, var: &str, case: &str) {
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{case}: {out:?}");
    assert!(stderr.contains(&format!("{var} must")), "{case}: {stderr}");
    assert!(
        !stderr.contains("not found on PATH"),
        "{case}: reached the tool check: {stderr}"
    );
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("building"),
        "{case}: built anyway"
    );
}

#[cfg(unix)]
fn assert_admitted(out: &Output, tool: &str, case: &str) {
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{case}: {out:?}");
    assert!(
        stderr.contains(&format!("{tool} not found on PATH")),
        "{case}: {stderr}"
    );
}

#[cfg(unix)]
#[test]
fn bench_refuses_a_warmup_that_is_not_a_small_whole_number() {
    for bad in ["abc", "-1", "1e3", "010", "10000", "3 --runs 1", ""] {
        let (out, _tools) = run("bench.sh", &[("BENCH_WARMUP", bad)]);
        // An empty value falls back to the default, like an unset one.
        if bad.is_empty() {
            assert_admitted(&out, "hyperfine", "empty BENCH_WARMUP");
            continue;
        }
        assert_refused(&out, "BENCH_WARMUP", bad);
    }
    for good in ["0", "10", "9999"] {
        let (out, _tools) = run("bench.sh", &[("BENCH_WARMUP", good)]);
        assert_admitted(&out, "hyperfine", good);
    }
}

#[cfg(unix)]
#[test]
fn bench_refuses_a_keep_flag_other_than_zero_or_one() {
    for bad in ["yes", "2", "true"] {
        let (out, _tools) = run("bench.sh", &[("BENCH_KEEP", bad)]);
        assert_refused(&out, "BENCH_KEEP", bad);
    }
    for good in ["0", "1"] {
        let (out, _tools) = run("bench.sh", &[("BENCH_KEEP", good)]);
        assert_admitted(&out, "hyperfine", good);
    }
}

#[cfg(unix)]
#[test]
fn both_harnesses_refuse_an_output_dir_that_is_a_file_or_an_option() {
    for (script, tool) in [
        ("bench.sh", "hyperfine"),
        ("bench-instructions.sh", "valgrind"),
    ] {
        let scratch = TempDir::new().unwrap();
        let file = scratch.path().join("results");
        fs::write(&file, "an existing file\n").unwrap();
        let file = file.display().to_string();
        for bad in [file.as_str(), "--help"] {
            let (out, _tools) = run(script, &[("BENCH_OUT", bad)]);
            assert_refused(&out, "BENCH_OUT", &format!("{script} {bad}"));
        }
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "an existing file\n",
            "{script}: the file was touched"
        );
        let dir = scratch.path().join("absent/dir").display().to_string();
        let (out, _tools) = run(script, &[("BENCH_OUT", dir.as_str())]);
        assert_admitted(&out, tool, &format!("{script} {dir}"));
    }
}

/// Every override a harness documents under "Environment overrides" is one it
/// refuses by name, so a new knob cannot land without its check (portable: this
/// reads the scripts, so it also holds on the Windows `cross` job, where the
/// journeys above do not run).
#[test]
fn every_documented_override_has_a_named_refusal() {
    for script in ["bench.sh", "bench-instructions.sh"] {
        let text = fs::read_to_string(repo_root().join("benches").join(script)).unwrap();
        let documented: Vec<&str> = text
            .lines()
            .skip_while(|l| !l.starts_with("# Environment overrides:"))
            .skip(1)
            .take_while(|l| l.starts_with("#   "))
            .filter_map(|l| l.trim_start_matches('#').split_whitespace().next())
            .collect();
        assert!(!documented.is_empty(), "{script}: no documented overrides");
        for var in documented {
            assert!(
                text.contains(&format!("fail \"{var} must")),
                "{script}: {var} is documented but never refused by name"
            );
        }
    }
}

/// `just bench` and `just profile` forward their arguments through Nx, which
/// rebuilds them into a shell command line, so each recipe refuses anything but
/// a plain word before Nx (or a build) is reached: a `$(…)` or `;` in an
/// argument is never run.
#[cfg(unix)]
#[test]
fn the_bench_and_profile_recipes_refuse_shell_syntax_before_reaching_nx() {
    let scratch = TempDir::new().unwrap();
    let marker = scratch.path().join("ran");
    let marker = marker.display().to_string();
    for (recipe, says) in [
        ("bench", "is not a plain baseline name or Criterion option"),
        (
            "profile",
            "is not a plain mode, llmlint argument or bench filter",
        ),
    ] {
        for bad in [
            format!("$(touch {marker})"),
            format!("`touch {marker}`"),
            format!("x;touch {marker}"),
            "two words".to_owned(),
        ] {
            let out = Command::new("just")
                .arg(recipe)
                .arg(&bad)
                .current_dir(repo_root())
                .output()
                .expect("`just` is a required dev tool (see scripts/setup-lib.sh)");
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert_eq!(out.status.code(), Some(2), "{recipe} {bad}: {stderr}");
            assert!(stderr.contains(says), "{recipe} {bad}: {stderr}");
            assert!(
                !stderr.contains("NX"),
                "{recipe} {bad}: reached Nx: {stderr}"
            );
        }
    }
    assert!(!Path::new(&marker).exists(), "an argument was executed");
}
