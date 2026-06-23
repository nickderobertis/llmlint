//! End-to-end tests: drive the **real `llmlint` binary** the way a user does,
//! against the deterministic `llmlint-mock-oneharness` fixture (the genuinely
//! external boundary) via `--oneharness-bin`. No network, no real LLM. Every
//! user-facing journey — happy path and failure/recovery — lands here as the
//! source of truth for what's covered (see `tests/AGENTS.md`).

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::cargo::cargo_bin;
use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use tempfile::TempDir;

fn mock_path() -> PathBuf {
    cargo_bin("llmlint-mock-oneharness")
}

/// A throwaway project directory with helpers to write configs/fixtures and
/// build llmlint invocations pointed at the mock.
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
    fn write(&self, rel: &str, contents: &str) -> &Self {
        let p = self.path().join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, contents).unwrap();
        self
    }
    fn write_verdicts(&self, json: &str) -> PathBuf {
        let p = self.path().join("verdicts.json");
        fs::write(&p, json).unwrap();
        p
    }
    /// A bare llmlint command (cwd = project), no oneharness wiring — for
    /// subcommands like `init`/`config` that take no `--oneharness-bin`.
    fn bare(&self) -> Command {
        let mut c = Command::cargo_bin("llmlint").unwrap();
        c.current_dir(self.path());
        c
    }
    /// A default-`lint` command wired to the mock harness.
    fn lint(&self) -> Command {
        let mut c = self.bare();
        c.arg("--oneharness-bin").arg(mock_path());
        c
    }
}

const RULE: &str = "TRUE when ok; FALSE otherwise.";

// ---- happy path ----------------------------------------------------------

#[test]
fn all_rules_pass_exits_zero() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: a_rule, description: \"{RULE}\" }}\n  \
             - {{ name: b_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"a_rule": true, "b_rule": true}"#);

    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .success()
        .stdout(predicate::str::contains("PASS a_rule"))
        .stdout(predicate::str::contains("PASS b_rule"))
        .stdout(predicate::str::contains("2 rules: 2 passed"));
}

#[test]
fn violation_fails_with_file_and_line() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: no_inline_sql, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/db.rs", "// code\n");
    let verdicts = p.write_verdicts(
        r#"{"no_inline_sql": {"holds": false, "violations": [
            {"file": "src/db.rs", "line": 12, "message": "inline SQL"}]}}"#,
    );

    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .code(1)
        .stdout(predicate::str::contains("FAIL no_inline_sql"))
        .stdout(predicate::str::contains("src/db.rs:12: inline SQL"));
}

// ---- multi-judge majority vote -------------------------------------------

#[test]
fn majority_vote_flips_a_single_dissent_to_pass() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: voted_rule, description: \"{RULE}\", judges: 3 }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    // Three sequential judges: fail, pass, pass -> majority pass.
    let verdicts = p.write_verdicts(r#"{"voted_rule": [false, true, true]}"#);
    let state = p.path().join("state");

    p.lint()
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_STATE", &state)
        .assert()
        .success()
        .stdout(predicate::str::contains("PASS voted_rule"));
}

#[test]
fn majority_vote_fails_when_most_judges_dissent() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: voted_rule, description: \"{RULE}\", judges: 3 }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(
        r#"{"voted_rule": [{"holds": false, "violations": [{"message": "bad"}]}, true, false]}"#,
    );
    let state = p.path().join("state");

    p.lint()
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_STATE", &state)
        .assert()
        .code(1)
        .stdout(predicate::str::contains(
            "FAIL voted_rule (1/3 judges held)",
        ));
}

// ---- includes / plugin system --------------------------------------------

#[test]
fn includes_merge_rules_from_another_file() {
    let p = Project::new();
    p.write(
        "team.yml",
        &format!("rules:\n  - {{ name: team_rule, description: \"{RULE}\" }}\n"),
    );
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\ninclude:\n  - ./team.yml\nrules:\n  \
             - {{ name: root_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"root_rule": true, "team_rule": true}"#);

    let out = p
        .lint()
        .arg("--format")
        .arg("json")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let names: Vec<&str> = v["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"root_rule"));
    assert!(names.contains(&"team_rule"));
}

#[test]
fn config_lint_plugin_catches_a_bad_rule() {
    let p = Project::new();
    // The bundled plugin lints config files; it always runs against llmlint.yml.
    p.write(
        "llmlint.yml",
        "version: 1\ninclude:\n  - llmlint:config-lint\n",
    );
    let verdicts = p.write_verdicts(
        r#"{"name_is_descriptive_not_placeholder":
              {"holds": false, "violations": [{"file": "llmlint.yml", "message": "rule named 'foo'"}]},
            "description_states_clear_true_and_false": true,
            "name_matches_description": true}"#,
    );

    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .code(1)
        .stdout(predicate::str::contains(
            "FAIL name_is_descriptive_not_placeholder",
        ))
        .stdout(predicate::str::contains("rule named 'foo'"));
}

// ---- file selection -------------------------------------------------------

#[test]
fn include_exclude_globs_select_the_right_files() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**/*.rs\"]\n  exclude: [\"**/gen.rs\"]\nrules:\n  \
             - {{ name: scoped_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/a.rs", "// a\n");
    p.write("src/gen.rs", "// generated\n");
    let verdicts = p.write_verdicts(r#"{"scoped_rule": true}"#);
    let dump = p.path().join("system.txt");

    p.lint()
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();

    let system = fs::read_to_string(&dump).unwrap();
    assert!(system.contains("src/a.rs"), "system:\n{system}");
    assert!(
        !system.contains("gen.rs"),
        "excluded file leaked:\n{system}"
    );
}

#[test]
fn explicit_cli_files_override_config_globs() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: cli_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/a.rs", "// a\n");
    p.write("README.md", "# readme\n");
    let verdicts = p.write_verdicts(r#"{"cli_rule": true}"#);
    let dump = p.path().join("system.txt");

    p.lint()
        .arg("README.md")
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();

    let system = fs::read_to_string(&dump).unwrap();
    assert!(system.contains("README.md"), "system:\n{system}");
    assert!(
        !system.contains("src/a.rs"),
        "config glob should be overridden"
    );
}

// ---- filters --------------------------------------------------------------

#[test]
fn rule_filter_limits_which_rules_run() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: keep_rule, description: \"{RULE}\" }}\n  \
             - {{ name: drop_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"keep_rule": true, "drop_rule": true}"#);

    let out = p
        .lint()
        .arg("--rule")
        .arg("keep_rule")
        .arg("--format")
        .arg("json")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let names: Vec<&str> = v["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["keep_rule"]);
}

#[test]
fn rules_with_no_matching_files_are_skipped() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"does-not-exist/**\"]\nrules:\n  \
             - {{ name: lonely_rule, description: \"{RULE}\" }}\n"
        ),
    );
    let verdicts = p.write_verdicts(r#"{"lonely_rule": true}"#);

    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .success()
        .stdout(predicate::str::contains("SKIP lonely_rule"));
}

// ---- init -----------------------------------------------------------------

#[test]
fn init_scaffolds_a_config_then_refuses_to_clobber() {
    let p = Project::new();
    p.bare().arg("init").assert().success();
    let cfg = fs::read_to_string(p.path().join("llmlint.yml")).unwrap();
    assert!(cfg.contains("llmlint:config-lint"));

    // Second init without --force fails (exit 2).
    p.bare().arg("init").assert().code(2);
    // --force overwrites.
    p.bare().arg("init").arg("--force").assert().success();
}

#[test]
fn init_with_template_embeds_the_prompt_template() {
    let p = Project::new();
    p.bare()
        .arg("init")
        .arg("--with-template")
        .assert()
        .success();
    let cfg = fs::read_to_string(p.path().join("llmlint.yml")).unwrap();
    assert!(cfg.contains("prompt_template: |"));
    assert!(cfg.contains("{% for r in rules %}"));
}

#[test]
fn init_then_self_lint_is_clean() {
    let p = Project::new();
    p.bare().arg("init").assert().success();
    // The example rule targets src/** (empty here -> skipped); the config-lint
    // plugin targets the config file. Mock holds everything -> exit 0.
    let verdicts = p.write_verdicts(
        r#"{"description_states_clear_true_and_false": true,
            "name_is_descriptive_not_placeholder": true,
            "name_matches_description": true,
            "public_items_are_documented": true}"#,
    );
    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .success();
}

// ---- config / doctor ------------------------------------------------------

#[test]
fn config_command_prints_merged_config_and_sources() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\ninclude:\n  - llmlint:config-lint\nrules:\n  \
             - {{ name: my_rule, description: \"{RULE}\" }}\n"
        ),
    );
    let out = p.bare().arg("config").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["config_files"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s == "llmlint:config-lint"));
    let names: Vec<&str> = v["config"]["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"my_rule"));
    assert!(names.contains(&"name_matches_description"));
}

#[test]
fn doctor_reports_the_harness_version() {
    let p = Project::new();
    p.bare()
        .arg("doctor")
        .env("LLMLINT_ONEHARNESS_BIN", mock_path())
        .assert()
        .success()
        .stdout(predicate::str::contains("oneharness"));
}

#[test]
fn doctor_fails_clearly_when_oneharness_is_missing() {
    let p = Project::new();
    p.bare()
        .arg("doctor")
        .env("LLMLINT_ONEHARNESS_BIN", "/nonexistent/oneharness-xyz")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("oneharness not found"));
}

// ---- failure / recovery ---------------------------------------------------

#[test]
fn missing_config_is_a_usage_error() {
    let p = Project::new();
    p.lint()
        .assert()
        .code(2)
        .stderr(predicate::str::contains("no llmlint config found"));
}

#[test]
fn malformed_config_is_a_usage_error() {
    let p = Project::new();
    p.write("llmlint.yml", "rules:\n  - name: r\n    bogus_field: 1\n");
    p.lint()
        .assert()
        .code(2)
        .stderr(predicate::str::contains("config"));
}

#[test]
fn duplicate_rule_names_are_rejected() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: dup, description: \"{RULE}\" }}\n  \
             - {{ name: dup, description: \"{RULE}\" }}\n"
        ),
    );
    p.lint()
        .assert()
        .code(2)
        .stderr(predicate::str::contains("duplicate rule name"));
}

#[test]
fn schema_validation_failure_is_surfaced() {
    let p = lint_project();
    p.lint()
        .env("LLMLINT_MOCK_FAIL_SCHEMA", "1")
        .assert()
        .code(2)
        .stdout(predicate::str::contains("failed schema validation"));
}

#[test]
fn missing_structured_output_is_surfaced() {
    let p = lint_project();
    p.lint()
        .env("LLMLINT_MOCK_NO_STRUCTURED", "1")
        .assert()
        .code(2)
        .stdout(predicate::str::contains("no structured output"));
}

#[test]
fn unparseable_oneharness_output_is_surfaced() {
    let p = lint_project();
    p.lint()
        .env("LLMLINT_MOCK_GARBAGE", "1")
        .assert()
        .code(2)
        .stdout(predicate::str::contains(
            "could not parse oneharness output",
        ));
}

/// A minimal one-rule project used by the failure-path tests.
fn lint_project() -> Project {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: some_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    p
}

// ---- additional coverage: filters, passthrough, more failure shapes -------

#[test]
fn empty_rule_selection_exits_zero() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: only_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    p.lint()
        .arg("--rule")
        .arg("nonexistent")
        .assert()
        .success()
        .stdout(predicate::str::contains("0 rules"));
}

#[test]
fn agent_filter_selects_only_that_agent() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nagents:\n  special:\n    \
             harness: claude-code\nrules:\n  \
             - {{ name: default_rule, description: \"{RULE}\" }}\n  \
             - {{ name: special_rule, description: \"{RULE}\", agent: special }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"default_rule": true, "special_rule": true}"#);
    let out = p
        .lint()
        .arg("--agent")
        .arg("special")
        .arg("--format")
        .arg("json")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let names: Vec<&str> = v["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["special_rule"]);
}

#[test]
fn oneharness_passthrough_args_are_accepted() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\noneharness:\n  model: gpt-5\n  \
             schema_max_retries: 1\n  config: [\"./oh.toml\"]\nagents:\n  a:\n    model: opus\n\
             rules:\n  - {{ name: passthrough_rule, description: \"{RULE}\", agent: a }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    p.write("oh.toml", "model = \"gpt-5\"\n");
    let verdicts = p.write_verdicts(r#"{"passthrough_rule": true}"#);
    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .success();
}

#[test]
fn multiple_oneharness_configs_warns() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\noneharness:\n  config: [\"./a.toml\"]\n\
             rules:\n  - {{ name: warn_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"warn_rule": true}"#);
    p.lint()
        .arg("--oneharness-config")
        .arg("./b.toml")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .success()
        .stderr(predicate::str::contains("single file"));
}

#[test]
fn oneharness_bin_from_env_is_used() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: env_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"env_rule": true}"#);
    // No --oneharness-bin; resolve it from the environment instead.
    p.bare()
        .env("LLMLINT_ONEHARNESS_BIN", mock_path())
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .success()
        .stdout(predicate::str::contains("PASS env_rule"));
}

#[test]
fn empty_results_from_oneharness_is_an_error() {
    let p = lint_project();
    p.lint()
        .env("LLMLINT_MOCK_EMPTY_RESULTS", "1")
        .assert()
        .code(2)
        .stdout(predicate::str::contains("no results"));
}

#[test]
fn bad_verdict_shape_is_an_error() {
    let p = lint_project();
    p.lint()
        .env("LLMLINT_MOCK_BAD_SHAPE", "1")
        .assert()
        .code(2)
        .stdout(predicate::str::contains("invalid verdict shape"));
}

#[test]
fn config_command_rejects_invalid_config() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nrules:\n  - {{ name: dup, description: \"{RULE}\" }}\n  \
             - {{ name: dup, description: \"{RULE}\" }}\n"
        ),
    );
    p.bare()
        .arg("config")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("duplicate rule name"));
}

#[test]
fn init_writes_to_a_custom_output_path() {
    let p = Project::new();
    p.bare()
        .arg("init")
        .arg("--output")
        .arg("config/nested/custom.yml")
        .assert()
        .success();
    assert!(p.path().join("config/nested/custom.yml").is_file());
}

#[test]
fn init_global_uses_xdg_config_home() {
    let p = Project::new();
    let xdg = p.path().join("xdg");
    p.bare()
        .arg("init")
        .arg("--global")
        .env("XDG_CONFIG_HOME", &xdg)
        .assert()
        .success();
    assert!(xdg.join("llmlint/llmlint.yml").is_file());
}

#[test]
fn init_global_falls_back_to_home_config() {
    let p = Project::new();
    let home = p.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    p.bare()
        .arg("init")
        .arg("--global")
        .env_remove("XDG_CONFIG_HOME")
        .env("HOME", &home)
        .assert()
        .success();
    assert!(home.join(".config/llmlint/llmlint.yml").is_file());
}
