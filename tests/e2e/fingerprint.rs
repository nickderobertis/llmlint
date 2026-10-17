//! `llmlint fingerprint`: the built binary run against real config files on
//! disk, with only oneharness replaced by the stand-in harness (whose `config`
//! subcommand resolves files, `extends` chains and `ONEHARNESS_*` overrides the
//! way `oneharness config` does).

use std::fs;

use super::*;

/// A project with a local rule, a local plugin, a discovered `oneharness.toml`
/// that `extends` a parent, and a named oneharness config that also `extends` it.
fn judge_project() -> Project {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nplugins:\n  - ./team.yml\nrules:\n  \
             - {{ name: local_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write(
        "team.yml",
        &format!("rules:\n  - {{ name: team_rule, description: \"{RULE}\" }}\n"),
    );
    p.write("src/lib.rs", "// code\n");
    p.write(
        "oneharness.toml",
        "extends = \"harness/base.toml\"\nrun_mode = \"fallback\"\n\n[harness.codex]\nmodel = \"codex-model\"\n",
    );
    p.write("harness/base.toml", "model = \"base-model\"\n");
    p.write(
        "harness/named.toml",
        "extends = \"base.toml\"\n\n[harness.claude-code]\nmodel = \"named-model\"\n",
    );
    p.write("template.md", "Judge {{ rules }} over {{ files }}.\n");
    p
}

/// `llmlint fingerprint` in `p`, wired to the stand-in harness.
fn fingerprint(p: &Project) -> Command {
    let mut c = p.bare();
    c.arg("fingerprint")
        .arg("--oneharness-bin")
        .arg(mock_path());
    c
}

/// Run `cmd` and return its digest, holding it to the output contract: exit 0
/// and exactly one line of 64 lowercase hex characters on stdout.
fn digest(cmd: &mut Command) -> String {
    let out = cmd.output().unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let line = stdout
        .strip_suffix('\n')
        .unwrap_or_else(|| panic!("no trailing newline: {stdout:?}"));
    assert_eq!(line.len(), 64, "{stdout:?}");
    assert!(
        line.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "{stdout:?}"
    );
    line.to_string()
}

/// Run `cmd`, expecting a failure: non-zero, nothing on stdout, and a message on
/// stderr. Returns the message.
fn failure(cmd: &mut Command) -> String {
    let out = cmd.output().unwrap();
    assert_ne!(out.status.code(), Some(0));
    assert!(
        out.stdout.is_empty(),
        "stdout: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(!stderr.trim().is_empty());
    stderr
}

/// Copy the files of `from` (not its history) into a fresh project.
fn copy_project(from: &Project) -> Project {
    let to = Project::new();
    copy_dir(from.path(), to.path());
    to
}

fn copy_dir(from: &Path, to: &Path) {
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if name == ".llmlint-history" {
            continue;
        }
        let dest = to.join(&name);
        if entry.file_type().unwrap().is_dir() {
            fs::create_dir_all(&dest).unwrap();
            copy_dir(&entry.path(), &dest);
        } else {
            fs::copy(entry.path(), dest).unwrap();
        }
    }
}

#[test]
fn two_copies_at_different_paths_and_two_harness_paths_print_one_digest() {
    let a = judge_project();
    let b = copy_project(&a);
    assert_ne!(a.path(), b.path());
    let in_a = digest(&mut fingerprint(&a));
    assert_eq!(digest(&mut fingerprint(&a)), in_a, "a rerun must agree");
    assert_eq!(
        digest(&mut fingerprint(&b)),
        in_a,
        "another checkout must agree"
    );

    // A harness setting naming a file inside each checkout by its absolute path
    // (what a wrapper exporting `$PWD/...` produces), with `--cwd` given
    // absolutely too: still one digest, since paths in the checkout are hashed
    // relative to it.
    let with_paths = |p: &Project| {
        let mut c = fingerprint(p);
        c.arg("--cwd").arg(p.path()).env(
            "ONEHARNESS_SCHEMA_FILE",
            p.path().join("harness").join("schema.json"),
        );
        digest(&mut c)
    };
    let pathed = with_paths(&a);
    assert_ne!(pathed, in_a, "the override is part of the judge");
    assert_eq!(with_paths(&b), pathed, "its checkout path must not be");

    // The same stand-in at another path, by flag and by env: the binary's path
    // contributes nothing.
    let bin_dir = TempDir::new().unwrap();
    let other = bin_dir
        .path()
        .join(format!("oneharness-copy{}", std::env::consts::EXE_SUFFIX));
    fs::copy(mock_path(), &other).unwrap();
    let mut by_flag = a.bare();
    by_flag
        .args(["fingerprint", "--oneharness-bin"])
        .arg(&other);
    assert_eq!(digest(&mut by_flag), in_a);
    let mut by_env = a.bare();
    by_env
        .arg("fingerprint")
        .env("LLMLINT_ONEHARNESS_BIN", &other);
    assert_eq!(digest(&mut by_env), in_a);
}

#[test]
fn what_decides_a_verdict_changes_the_digest() {
    let p = judge_project();
    let base = digest(&mut fingerprint(&p));
    let mut seen = vec![("baseline", base.clone())];
    let mut differs = |what: &'static str, d: String| {
        for (other, prior) in &seen {
            assert_ne!(&d, prior, "{what} printed the same digest as {other}");
        }
        seen.push((what, d));
    };

    // A local rule.
    let cfg = fs::read_to_string(p.path().join("llmlint.yml")).unwrap();
    p.write(
        "llmlint.yml",
        &cfg.replace(RULE, "true when tidy; false otherwise."),
    );
    differs("a changed local rule", digest(&mut fingerprint(&p)));
    p.write("llmlint.yml", &cfg);
    assert_eq!(
        digest(&mut fingerprint(&p)),
        base,
        "restoring it restores the digest"
    );

    // The prompt template.
    differs(
        "a prompt template",
        digest(fingerprint(&p).arg("--prompt-template").arg("template.md")),
    );
    p.write(
        "template.md",
        "Judge {{ rules }} carefully over {{ files }}.\n",
    );
    differs(
        "a changed prompt template",
        digest(fingerprint(&p).arg("--prompt-template").arg("template.md")),
    );

    // The model, on the command line.
    differs("--model", digest(fingerprint(&p).args(["--model", "m-1"])));
    differs(
        "another --model",
        digest(fingerprint(&p).args(["--model", "m-2"])),
    );

    // The model in the harness configuration: in oneharness's discovered file,
    // in its `extends` parent, and in a file named directly.
    p.write(
        "oneharness.toml",
        "extends = \"harness/base.toml\"\nrun_mode = \"fallback\"\n\n[harness.codex]\nmodel = \"codex-model-2\"\n",
    );
    differs("the discovered harness model", digest(&mut fingerprint(&p)));
    p.write("harness/base.toml", "model = \"base-model-2\"\n");
    differs("the extends parent's model", digest(&mut fingerprint(&p)));
    let named = || {
        let mut c = fingerprint(&p);
        c.args(["--oneharness-config", "harness/named.toml"]);
        c
    };
    differs("a named harness config", digest(&mut named()));
    p.write(
        "harness/named.toml",
        "extends = \"base.toml\"\n\n[harness.claude-code]\nmodel = \"named-model-2\"\n",
    );
    differs("the named file's model", digest(&mut named()));
    p.write("harness/base.toml", "model = \"base-model-3\"\n");
    differs("the named file's extends parent", digest(&mut named()));

    // An `ONEHARNESS_*` override oneharness applies is part of the judge too.
    differs(
        "ONEHARNESS_MODEL",
        digest(fingerprint(&p).env("ONEHARNESS_MODEL", "env-model")),
    );
}

#[test]
fn a_plugin_resolving_to_a_newer_version_changes_the_digest() {
    // The same rule at both versions, so only the version actually fetched for
    // the `@1` range differs.
    let (p, origin, cache) = project_with_pinned_plugin("shared_rule", "1.2");
    let run = || {
        let mut c = fingerprint(&p);
        c.env("LLMLINT_CACHE_DIR", &cache)
            .env("LLMLINT_PLUGIN_TTL", "0");
        digest(&mut c)
    };
    let first = run();
    assert_eq!(run(), first);
    publish(&origin, "shared_rule", "1.4");
    assert_ne!(
        run(),
        first,
        "a newer version within the pin must change it"
    );
}

#[test]
fn recording_only_settings_leave_the_digest_alone() {
    let p = judge_project();
    let base = digest(&mut fingerprint(&p));
    let elsewhere = TempDir::new().unwrap();
    for (var, value) in [
        ("ONEHARNESS_HISTORY", "true"),
        ("ONEHARNESS_HISTORY_DIR", "/var/tmp/oneharness-history"),
        ("ONEHARNESS_HISTORY_POINTER_FILE", "/var/tmp/sessions.jsonl"),
        (
            "ONEHARNESS_HISTORY_LABELS",
            "onepipeline.attempt=2,onepipeline.run_id=x",
        ),
        ("LLMLINT_HISTORY_ENABLED", "false"),
        ("LLMLINT_HISTORY_MAX_RUNS", "3"),
        ("LLMLINT_NO_HISTORY", "1"),
    ] {
        assert_eq!(
            digest(fingerprint(&p).env(var, value)),
            base,
            "{var} only records a run"
        );
    }
    assert_eq!(
        digest(fingerprint(&p).env("LLMLINT_HISTORY_DIR", elsewhere.path())),
        base,
        "LLMLINT_HISTORY_DIR only records a run"
    );
    let cfg = fs::read_to_string(p.path().join("llmlint.yml")).unwrap();
    p.write(
        "llmlint.yml",
        &format!("history:\n  enabled: false\n  max_runs: 7\n  dir: /var/tmp/h\n{cfg}"),
    );
    assert_eq!(
        digest(&mut fingerprint(&p)),
        base,
        "the history block only records"
    );
    // The control: the harness model beside them still counts.
    p.write("harness/base.toml", "model = \"base-model-2\"\n");
    assert_ne!(digest(&mut fingerprint(&p)), base);
}

#[test]
fn fingerprint_makes_no_judge_call() {
    let p = judge_project();
    let spawns = p.path().join("spawns");
    digest(fingerprint(&p).env("LLMLINT_MOCK_SPAWNLOG", &spawns));
    let mut kinds: Vec<String> = fs::read_dir(&spawns)
        .unwrap()
        .map(|e| {
            let args = fs::read_to_string(e.unwrap().path()).unwrap();
            args.lines().next().unwrap_or_default().to_string()
        })
        .collect();
    kinds.sort();
    // Only the version query and the resolved-config query; never `run`.
    assert_eq!(kinds, vec!["--version", "config"]);
}

#[test]
fn an_invalid_config_fails_with_nothing_on_stdout() {
    let p = judge_project();
    p.write(
        "llmlint.yml",
        &format!("rules:\n  - {{ name: local_rule, judges: 2, description: \"{RULE}\" }}\n"),
    );
    let msg = failure(&mut fingerprint(&p));
    assert!(msg.contains("judges: 2 (must be odd"), "{msg}");

    p.write("llmlint.yml", "rules: [unterminated\n");
    let msg = failure(&mut fingerprint(&p));
    assert!(msg.contains("llmlint.yml"), "{msg}");
}

#[test]
fn an_unresolvable_plugin_fails_with_nothing_on_stdout() {
    let p = judge_project();
    let missing = p.path().join("nowhere.yml");
    p.write(
        "llmlint.yml",
        &format!(
            "plugins:\n  - \"{}@1\"\nrules:\n  - {{ name: local_rule, description: \"{RULE}\" }}\n",
            file_url(&missing)
        ),
    );
    let msg = failure(fingerprint(&p).env("LLMLINT_CACHE_DIR", p.path().join("cache")));
    assert!(msg.contains("nowhere.yml"), "{msg}");
}

#[test]
fn an_unreadable_harness_configuration_fails_with_nothing_on_stdout() {
    let p = judge_project();
    // Named, but absent.
    let msg = failure(fingerprint(&p).args(["--oneharness-config", "harness/absent.toml"]));
    assert!(msg.contains("harness/absent.toml"), "{msg}");
    assert!(msg.contains("--oneharness-config"), "{msg}");
    // Discovered, but its `extends` parent is absent: oneharness's own refusal.
    p.write("oneharness.toml", "extends = \"harness/gone.toml\"\n");
    let msg = failure(&mut fingerprint(&p));
    assert!(msg.contains("harness configuration"), "{msg}");
    assert!(msg.contains("gone.toml"), "{msg}");
    // Invalid TOML in a discovered file.
    p.write("oneharness.toml", "model = [unterminated\n");
    let msg = failure(&mut fingerprint(&p));
    assert!(msg.contains("oneharness.toml"), "{msg}");
}

#[test]
fn a_oneharness_that_cannot_report_its_version_fails_with_nothing_on_stdout() {
    let p = judge_project();
    let mut c = p.bare();
    c.args(["fingerprint", "--oneharness-bin"])
        .arg(p.path().join("no-such-oneharness"));
    let msg = failure(&mut c);
    assert!(msg.contains("no-such-oneharness"), "{msg}");
}
