//! `llmlint rules`: the rules that would judge a set of paths, read before any of
//! them is written. Each journey drives the real binary over a fixture config of
//! its own; the parity journeys run the real `lint` beside it, with the paid
//! model doubled at the harness boundary (the mock oneharness, recording every
//! prompt it received), and require the two to name the same rules.

// llmlint: ignore-file[e2e_not_mocked] the stand-in oneharness is this suite's double for the oneharness subprocess, the one genuinely external boundary (AGENTS.md); it is used only to record the prompts the real `lint` sends so `rules` can be held to them, and every journey drives the real freshly built llmlint binary over real files and git repos

use std::collections::BTreeSet;

use llmlint::domain::rule_listing::{self, RulesDocument};

use super::*;

const REVIEWER: &str = "You review the code as a careful maintainer.";
const ARCHITECT: &str = "You review the code as its architect.";
const IDLE: &str = "You review nothing these paths touch.";

/// The fixture: two agents with distinct framings (and a third no matched rule
/// uses), a rule with no file scoping, a rule whose own `files.include` names one
/// directory, a rule whose scoping another glob excludes, a switched-off rule, a
/// rule from a local plugin, and a session `files.exclude`.
fn fixture() -> Project {
    let p = Project::new();
    p.write(
        "team.yml",
        "rules:\n  - { name: plugin_rule, agent: architect, \
         description: \"Plugin invariant holds.\" }\n",
    );
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\n\
             files:\n  exclude: [\"generated/**\"]\n\
             plugins:\n  - ./team.yml\n\
             agents:\n  \
               reviewer: {{ prompt_template: \"{REVIEWER}\" }}\n  \
               architect: {{ prompt_template: \"{ARCHITECT}\" }}\n  \
               idle: {{ prompt_template: \"{IDLE}\" }}\n\
             rules:\n  \
               - {{ name: unscoped, agent: reviewer, description: \"Unscoped invariant holds.\" }}\n  \
               - {{ name: api_only, agent: reviewer, description: \"API invariant holds.\", \
                    files: {{ include: [\"api/**\"] }} }}\n  \
               - {{ name: not_generated_code, agent: architect, \
                    description: \"Handwritten code invariant holds.\", \
                    files: {{ include: [\"**/*.rs\"], exclude: [\"**/*_gen.rs\"] }} }}\n  \
               - {{ name: switched_off, agent: reviewer, description: \"Never judged.\", \
                    relevance: false }}\n  \
               - {{ name: idle_rule, agent: idle, description: \"Idle invariant holds.\", \
                    files: {{ include: [\"never/**\"] }} }}\n"
        ),
    );
    p
}

/// Run `rules` with `args` in `dir`, asserting success, and parse its JSON into
/// the one Rust declaration of that document (which refuses unknown fields).
fn rules_json(p: &Project, dir: &Path, args: &[&str]) -> (RulesDocument, Value) {
    let out = p
        .bare()
        .current_dir(dir)
        .arg("rules")
        .args(args)
        .args(["--format", "json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "rules {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    let doc: RulesDocument = serde_json::from_value(value.clone()).unwrap();
    (doc, value)
}

fn rule_names(doc: &RulesDocument) -> Vec<&str> {
    doc.rules.iter().map(|r| r.name.0.as_str()).collect()
}

/// The rule names every judge prompt the mock harness recorded under `log`
/// carried: each `### <name>` heading of the prompt's "Rules to evaluate"
/// section.
fn prompted_rules(log: &Path) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for entry in fs::read_dir(log).unwrap() {
        let record: Value =
            serde_json::from_slice(&fs::read(entry.unwrap().path()).unwrap()).unwrap();
        let system = record["system"].as_str().unwrap();
        let rules = system
            .split("## Rules to evaluate")
            .nth(1)
            .and_then(|s| s.split("## Response").next())
            .expect("a prompt carries its rules");
        for line in rules.lines() {
            if let Some(name) = line.strip_prefix("### ") {
                names.insert(name.trim().to_string());
            }
        }
    }
    names
}

/// Run the real `lint` over `paths` from `dir` against the mock harness and
/// return the rules its judge prompts carried.
fn linted_rules(p: &Project, dir: &Path, paths: &[&str], log_name: &str) -> BTreeSet<String> {
    let log = p.path().join(log_name);
    let out = p
        .lint()
        .current_dir(dir)
        .args(paths)
        .env("LLMLINT_MOCK_PROMPTLOG", &log)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "lint {paths:?} failed:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if log.exists() {
        prompted_rules(&log)
    } else {
        BTreeSet::new()
    }
}

fn as_set(doc: &RulesDocument) -> BTreeSet<String> {
    doc.rules.iter().map(|r| r.name.0.clone()).collect()
}

#[test]
fn rules_for_paths_that_do_not_exist_are_exactly_the_ones_whose_scoping_matches() {
    let p = fixture();
    let paths = ["api/handler.rs", "lib/core_gen.rs", "generated/out.rs"];
    for path in paths {
        assert!(!p.path().join(path).exists());
    }
    let mut args = vec!["--files"];
    args.extend(paths);
    let (doc, _) = rules_json(&p, p.path(), &args);

    assert_eq!(doc.schema_version, rule_listing::SCHEMA_VERSION);
    assert_eq!(
        doc.paths,
        ["api/handler.rs", "generated/out.rs", "lib/core_gen.rs"]
    );
    // Config order, the plugin's rule included; the switched-off rule and the
    // rule scoped to a directory no path is in are absent.
    let mut names = rule_names(&doc);
    let plugin_at = names.iter().position(|n| *n == "plugin_rule").unwrap();
    names.remove(plugin_at);
    assert_eq!(names, ["unscoped", "api_only", "not_generated_code"]);
    let by_name = |n: &str| doc.rules.iter().find(|r| r.name.0 == n).unwrap();

    let unscoped = by_name("unscoped");
    assert_eq!(unscoped.description, "Unscoped invariant holds.");
    assert_eq!(unscoped.agent.0, "reviewer");
    assert_eq!(unscoped.paths, ["api/handler.rs", "lib/core_gen.rs"]);
    let api = by_name("api_only");
    assert_eq!(
        (api.agent.0.as_str(), api.paths.as_slice()),
        ("reviewer", &["api/handler.rs".to_string()][..])
    );
    // The rule's own exclude glob keeps `lib/core_gen.rs` from it.
    let handwritten = by_name("not_generated_code");
    assert_eq!(handwritten.agent.0, "architect");
    assert_eq!(handwritten.paths, ["api/handler.rs"]);
    let plugin = by_name("plugin_rule");
    assert_eq!(plugin.description, "Plugin invariant holds.");
    assert_eq!(plugin.agent.0, "architect");

    // `generated/out.rs` is covered only by the excluded glob: no rule judges it.
    for rule in &doc.rules {
        assert!(
            !rule.paths.contains(&"generated/out.rs".to_string()),
            "{rule:?}"
        );
    }
    // Each judging agent once, with its framing; `idle` judges nothing here.
    let agents: Vec<(&str, &str)> = doc
        .agents
        .iter()
        .map(|a| (a.name.0.as_str(), a.framing.as_str()))
        .collect();
    assert_eq!(agents, [("reviewer", REVIEWER), ("architect", ARCHITECT)]);

    // A path only the excluded glob covers adds no rule on its own.
    let (only_excluded, _) = rules_json(&p, p.path(), &["--files", "generated/out.rs"]);
    assert!(only_excluded.rules.is_empty(), "{only_excluded:?}");
    assert!(only_excluded.agents.is_empty(), "{only_excluded:?}");
}

#[test]
fn rules_markdown_renders_the_same_rules_and_agents() {
    let p = fixture();
    let (doc, _) = rules_json(&p, p.path(), &["--files", "api/handler.rs"]);
    let out = p
        .bare()
        .args(["rules", "--files", "api/handler.rs", "--format", "markdown"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let md = String::from_utf8(out.stdout).unwrap();

    for agent in &doc.agents {
        let heading = format!("### Agent `{}`\n\n{}\n", agent.name, agent.framing);
        assert!(md.contains(&heading), "missing {heading:?}:\n{md}");
    }
    for rule in &doc.rules {
        let heading = format!("#### `{}`\n\n{}\n", rule.name, rule.description);
        assert!(md.contains(&heading), "missing {heading:?}:\n{md}");
        // Listed under its own agent's subsection.
        let agent_at = md.find(&format!("### Agent `{}`", rule.agent)).unwrap();
        let rule_at = md.find(&heading).unwrap();
        let next_agent = md[agent_at + 1..]
            .find("### Agent")
            .map_or(md.len(), |i| agent_at + 1 + i);
        assert!(agent_at < rule_at && rule_at < next_agent, "{md}");
    }
    assert_eq!(md.matches("#### `").count(), doc.rules.len(), "{md}");
    assert_eq!(md.matches("### Agent").count(), doc.agents.len(), "{md}");
    for absent in ["switched_off", "idle", IDLE] {
        assert!(!md.contains(absent), "{absent} must not appear:\n{md}");
    }
}

#[test]
fn a_gitignored_existing_file_is_treated_as_lint_treats_it() {
    let p = fixture();
    p.write(".gitignore", "build/\n");
    p.write("build/out.rs", "fn built() {}\n");
    init_repo(p.path());

    let (doc, _) = rules_json(&p, p.path(), &["--files", "build/out.rs"]);
    let linted = linted_rules(&p, p.path(), &["build/out.rs"], "prompts");
    assert!(!linted.is_empty(), "lint judges a named gitignored file");
    assert_eq!(as_set(&doc), linted);
    let built = doc.rules.iter().find(|r| r.name.0 == "unscoped").unwrap();
    assert_eq!(built.paths, ["build/out.rs"]);
}

#[test]
fn rules_diff_lists_the_rules_for_added_and_changed_paths_not_deleted_ones() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        "version: 1\nrules:\n  \
         - { name: unscoped, description: \"Unscoped invariant holds.\" }\n  \
         - { name: src_rule, description: \"Src invariant holds.\", files: { include: [\"src/**\"] } }\n  \
         - { name: api_rule, description: \"API invariant holds.\", files: { include: [\"api/**\"] } }\n  \
         - { name: docs_rule, description: \"Docs invariant holds.\", files: { include: [\"docs/**\"] } }\n",
    );
    p.write("src/keep.rs", "fn keep() {}\n");
    p.write("src/change.rs", "fn change() {}\n");
    p.write("docs/old.md", "# old\n");
    init_repo(p.path());
    git(p.path(), &["add", "."]);
    git(p.path(), &["commit", "-q", "-m", "base"]);
    let base = git_out(p.path(), &["rev-parse", "HEAD"]);

    p.write("src/added.rs", "fn added() {}\n");
    p.write("api/new.rs", "fn new() {}\n");
    p.write("src/change.rs", "fn change() { changed(); }\n");
    fs::remove_file(p.path().join("docs/old.md")).unwrap();
    git(p.path(), &["add", "-A"]);
    git(p.path(), &["commit", "-q", "-m", "later"]);

    let (diffed, _) = rules_json(&p, p.path(), &["--diff", &base]);
    assert_eq!(
        diffed.paths,
        ["api/new.rs", "src/added.rs", "src/change.rs"]
    );
    assert_eq!(rule_names(&diffed), ["unscoped", "src_rule", "api_rule"]);

    let (named, _) = rules_json(
        &p,
        p.path(),
        &["--files", "src/added.rs", "api/new.rs", "src/change.rs"],
    );
    assert_eq!(diffed.rules, named.rules);
    assert_eq!(diffed.agents, named.agents);

    // The deleted path is what would bring `docs_rule` in; under `--diff` it
    // contributes nothing.
    let (deleted, _) = rules_json(&p, p.path(), &["--files", "docs/old.md"]);
    assert_eq!(rule_names(&deleted), ["unscoped", "docs_rule"]);
}

/// A root config whose `files.include`/`files.exclude` narrow the tree, and a
/// nested config that adds rules and switches one of each off (its own, and the
/// root's by override).
fn nested_fixture() -> Project {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        "version: 1\n\
         files:\n  include: [\"app/**\", \"lib/**\"]\n  exclude: [\"app/vendor/**\"]\n\
         rules:\n  \
           - { name: root_all, description: \"Root invariant holds.\" }\n  \
           - { name: root_lib, description: \"Lib invariant holds.\", files: { include: [\"lib/**\"] } }\n  \
           - { name: root_quieted, description: \"Quieted invariant holds.\" }\n",
    );
    p.write(
        "app/llmlint.yml",
        "rules:\n  \
           - { name: app_rule, description: \"App invariant holds.\" }\n  \
           - { name: app_rs_rule, description: \"App source invariant holds.\", files: { include: [\"*.rs\"] } }\n  \
           - { name: app_off, description: \"Never judged.\", relevance: false }\n  \
           - { name: root_quieted, override: true, relevance: false }\n",
    );
    p
}

const NESTED_FILES: [&str; 5] = [
    "app/main.rs",
    "app/notes.md",
    "app/vendor/dep.rs",
    "lib/util.rs",
    "other/readme.md",
];

#[test]
fn rules_select_what_lint_judges_from_the_root_and_a_subdirectory() {
    let p = nested_fixture();
    for f in NESTED_FILES {
        p.write(f, "content\n");
    }
    let app = p.path().join("app");
    let cases: [(&Path, &str, Vec<&str>); 4] = [
        (p.path(), "root", NESTED_FILES.to_vec()),
        (p.path(), "root-lib", vec!["lib/util.rs", "other/readme.md"]),
        (
            &app,
            "sub",
            vec!["main.rs", "notes.md", "vendor/dep.rs", "../lib/util.rs"],
        ),
        (&app, "sub-md", vec!["notes.md"]),
    ];
    let mut listed = Vec::new();
    for (dir, name, paths) in &cases {
        let mut args = vec!["--files"];
        args.extend(paths.iter().copied());
        let (doc, _) = rules_json(&p, dir, &args);
        let linted = linted_rules(&p, dir, paths, &format!("prompts-{name}"));
        assert_eq!(as_set(&doc), linted, "{name}: rules vs lint over {paths:?}");
        assert!(
            !as_set(&doc).contains("app_off"),
            "{name}: app_off is switched off"
        );
        listed.push(doc);
    }
    // The cases are not vacuous: they differ, and they reach the nested rules.
    assert_eq!(
        as_set(&listed[0]),
        ["app_rs_rule", "app_rule", "root_all", "root_lib"]
            .map(String::from)
            .into()
    );
    // No path lies under `app/`, so (as for `lint`) the nested config is not
    // loaded and its override switching `root_quieted` off does not apply.
    assert_eq!(
        as_set(&listed[1]),
        ["root_all", "root_lib", "root_quieted"]
            .map(String::from)
            .into()
    );
    assert_eq!(
        as_set(&listed[3]),
        ["app_rule", "root_all"].map(String::from).into()
    );

    // The same paths made hypothetical list the same rules.
    for f in NESTED_FILES {
        fs::remove_file(p.path().join(f)).unwrap();
    }
    for ((dir, name, paths), before) in cases.iter().zip(&listed) {
        let mut args = vec!["--files"];
        args.extend(paths.iter().copied());
        let (doc, _) = rules_json(&p, dir, &args);
        assert_eq!(&doc, before, "{name}: hypothetical paths changed the rules");
    }
}

#[test]
fn rules_needs_no_harness_binary_or_harness_configuration() {
    let p = fixture();
    let empty_path = TempDir::new().unwrap();
    let spawns = p.path().join("spawns");
    assert!(!p.path().join("oneharness.toml").exists());
    let out = p
        .bare()
        .env("PATH", empty_path.path())
        // Were llmlint to resolve a harness at all, it would find this one and
        // the mock would log the spawn.
        .env("LLMLINT_ONEHARNESS_BIN", mock_path())
        .env("LLMLINT_MOCK_SPAWNLOG", &spawns)
        .args(["rules", "--files", "api/handler.rs"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let doc: RulesDocument = serde_json::from_slice(&out.stdout).unwrap();
    assert!(!doc.rules.is_empty());
    assert_eq!(
        harness_spawns(&spawns),
        0,
        "rules must never start a harness"
    );
}

#[test]
fn rules_requires_paths_or_a_diff() {
    let p = fixture();
    p.bare()
        .arg("rules")
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "--files <PATH>...|--diff [<BASE>]",
        ));
    p.bare()
        .args(["rules", "--files", "a.rs", "--diff"])
        .assert()
        .code(2);
}

/// Check `value` against the JSON Schema node `schema` (resolving `#/$defs/`
/// refs against `root`): every object's keys are exactly its `properties`, all
/// `required`, each value of the declared type.
fn conforms(root: &Value, schema: &Value, value: &Value, at: &str) {
    if let Some(r) = schema["$ref"].as_str() {
        let name = r.strip_prefix("#/$defs/").expect("local ref");
        return conforms(root, &root["$defs"][name], value, at);
    }
    match schema["type"].as_str().expect("typed schema node") {
        "object" => {
            let obj = value
                .as_object()
                .unwrap_or_else(|| panic!("{at}: not an object"));
            let props = schema["properties"].as_object().unwrap();
            let keys: BTreeSet<&String> = obj.keys().collect();
            assert_eq!(
                keys,
                props.keys().collect(),
                "{at}: keys differ from the declaration"
            );
            for req in schema["required"].as_array().unwrap() {
                assert!(
                    obj.contains_key(req.as_str().unwrap()),
                    "{at}: missing {req}"
                );
            }
            assert_eq!(schema["additionalProperties"], Value::Bool(false), "{at}");
            for (k, v) in obj {
                conforms(root, &props[k], v, &format!("{at}.{k}"));
            }
        }
        "array" => {
            for (i, v) in value
                .as_array()
                .unwrap_or_else(|| panic!("{at}: not an array"))
                .iter()
                .enumerate()
            {
                conforms(root, &schema["items"], v, &format!("{at}[{i}]"));
            }
        }
        "string" => assert!(value.is_string(), "{at}: not a string"),
        "integer" => assert!(value.is_u64(), "{at}: not an unsigned integer"),
        other => panic!("{at}: unexpected schema type {other}"),
    }
}

#[test]
fn the_printed_document_reconciles_with_its_one_declaration() {
    let p = fixture();
    let (doc, value) = rules_json(&p, p.path(), &["--files", "api/handler.rs", "lib/a.rs"]);
    let schema = rule_listing::schema();
    conforms(&schema, &schema, &value, "$");
    // Re-serializing the parsed declaration reproduces the printed document
    // exactly: nothing printed outside the type, nothing reordered.
    assert_eq!(serde_json::to_value(&doc).unwrap(), value);
    // Non-vacuous: every nested shape was walked.
    assert!(!doc.agents.is_empty() && !doc.rules.is_empty());
}

fn names_and_paths(doc: &RulesDocument) -> Vec<(String, Vec<String>)> {
    doc.rules
        .iter()
        .map(|r| (r.name.0.clone(), r.paths.clone()))
        .collect()
}

#[test]
fn rules_honor_cwd_and_an_explicit_config_replacing_discovery() {
    let p = nested_fixture();
    let elsewhere = TempDir::new().unwrap();
    let cwd = p.path().to_str().unwrap();
    // `--cwd` resolves discovery (the nested config included) and the paths
    // from the project, wherever the command itself runs.
    let (from_cwd, _) = rules_json(
        &p,
        elsewhere.path(),
        &["--cwd", cwd, "--files", "app/main.rs"],
    );
    assert_eq!(
        names_and_paths(&from_cwd),
        [
            ("root_all".to_string(), vec!["app/main.rs".to_string()]),
            ("app_rule".to_string(), vec!["app/main.rs".to_string()]),
            ("app_rs_rule".to_string(), vec!["app/main.rs".to_string()]),
        ]
    );
    // `-c` replaces the walk: the nested config is not loaded, so its rules are
    // absent and its override no longer switches `root_quieted` off.
    let (explicit, _) = rules_json(
        &p,
        elsewhere.path(),
        &["--cwd", cwd, "-c", "llmlint.yml", "--files", "app/main.rs"],
    );
    let names: Vec<&str> = rule_names(&explicit);
    assert_eq!(names, ["root_all", "root_quieted"]);
}

#[test]
fn rules_apply_env_and_cli_file_filters_like_lint() {
    let p = fixture();
    let paths = ["api/handler.rs", "lib/core.rs", "docs/guide.md"];
    let mut args = vec!["--files"];
    args.extend(paths);

    // A session `files.include` from the env narrows the rules scoped by the
    // session filter (`unscoped`, the plugin rule), not a rule's own `files`.
    let out = p
        .bare()
        .env("LLMLINT_FILES_INCLUDE", "lib/**")
        .arg("rules")
        .args(&args)
        .output()
        .unwrap();
    assert!(out.status.success());
    let doc: RulesDocument = serde_json::from_slice(&out.stdout).unwrap();
    let got = names_and_paths(&doc);
    assert!(
        got.contains(&("unscoped".into(), vec!["lib/core.rs".into()])),
        "{got:?}"
    );
    assert!(
        got.contains(&("plugin_rule".into(), vec!["lib/core.rs".into()])),
        "{got:?}"
    );
    assert!(
        got.contains(&("api_only".into(), vec!["api/handler.rs".into()])),
        "{got:?}"
    );

    // An env exclude and repeated `--exclude`s all add to the denylist.
    let out = p
        .bare()
        .env("LLMLINT_FILES_EXCLUDE", "api/**")
        .arg("rules")
        .args(&args)
        .args(["--exclude", "lib/**", "--exclude", "docs/**"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let doc: RulesDocument = serde_json::from_slice(&out.stdout).unwrap();
    assert!(doc.rules.is_empty(), "{doc:?}");
    assert_eq!(
        doc.paths,
        ["api/handler.rs", "docs/guide.md", "lib/core.rs"]
    );
}

/// A repo whose history moves through `basetag` (src/a.rs, src/b.rs, src/c.rs,
/// src/old.rs), then a commit changing `a`, then one changing `b` and renaming
/// `old` to `moved`, with `c` changed in the work tree; `diff_base` names the tag.
fn diff_repo() -> (Project, String) {
    let p = Project::new();
    let config = "version: 1\ndiff_base: basetag\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
                  - { name: src_rule, description: \"Src invariant holds.\" }\n";
    p.write("llmlint.yml", config);
    let long: String = (0..40).map(|i| format!("fn line_{i}() {{}}\n")).collect();
    for f in ["src/a.rs", "src/b.rs", "src/c.rs"] {
        p.write(f, "fn x() {}\n");
    }
    p.write("src/old.rs", &long);
    init_repo(p.path());
    git(p.path(), &["add", "."]);
    git(p.path(), &["commit", "-q", "-m", "base"]);
    git(p.path(), &["tag", "basetag"]);
    p.write("src/a.rs", "fn x() { a(); }\n");
    git(p.path(), &["commit", "-qam", "a"]);
    let after_a = git_out(p.path(), &["rev-parse", "HEAD"]);
    p.write("src/b.rs", "fn x() { b(); }\n");
    git(p.path(), &["mv", "src/old.rs", "src/moved.rs"]);
    git(p.path(), &["commit", "-qam", "b and rename"]);
    p.write("src/c.rs", "fn x() { c(); }\n");
    (p, after_a)
}

fn diff_paths(p: &Project, args: &[&str], env: &[(&str, &str)]) -> Vec<String> {
    let mut cmd = p.bare();
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.arg("rules").args(args).output().unwrap();
    assert!(
        out.status.success(),
        "rules {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let doc: RulesDocument = serde_json::from_slice(&out.stdout).unwrap();
    // Every listed path is one the rule judges.
    let judged: Vec<String> = doc.rules.iter().flat_map(|r| r.paths.clone()).collect();
    assert_eq!(judged, doc.paths);
    doc.paths
}

#[test]
fn rules_diff_takes_its_base_from_the_flag_the_env_the_config_or_head() {
    let (p, after_a) = diff_repo();
    let all = ["src/a.rs", "src/b.rs", "src/c.rs", "src/moved.rs"];
    // Bare `--diff`: the config's `diff_base`; the rename lists only its new path.
    assert_eq!(diff_paths(&p, &["--diff"], &[]), all);
    // A `--diff BASE` wins over the config.
    assert_eq!(
        diff_paths(&p, &["--diff", &after_a], &[]),
        ["src/b.rs", "src/c.rs", "src/moved.rs"]
    );
    // `LLMLINT_DIFF_BASE` wins over the config; the flag wins over it.
    assert_eq!(
        diff_paths(&p, &["--diff"], &[("LLMLINT_DIFF_BASE", &after_a)]),
        ["src/b.rs", "src/c.rs", "src/moved.rs"]
    );
    assert_eq!(
        diff_paths(
            &p,
            &["--diff", "basetag"],
            &[("LLMLINT_DIFF_BASE", &after_a)]
        ),
        all
    );
    // A range compares two commits, so the work-tree change is outside it.
    assert_eq!(
        diff_paths(&p, &["--diff", &format!("basetag..{after_a}")], &[]),
        ["src/a.rs"]
    );
    // With no base configured anywhere, `HEAD`: only the work-tree change.
    p.write(
        "llmlint.yml",
        "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
         - { name: src_rule, description: \"Src invariant holds.\" }\n",
    );
    assert_eq!(diff_paths(&p, &["--diff"], &[]), ["src/c.rs"]);
    // A base git cannot resolve is git's own error, never an empty list.
    p.bare()
        .args(["rules", "--diff", "no-such-ref"])
        .assert()
        .code(2);
}

#[test]
fn a_whole_file_ignore_drops_only_the_rule_it_names_like_lint() {
    let p = fixture();
    // Spelled in pieces so this test file itself carries no directive.
    let directive = format!("// llmlint: {}[api_only] generated client", "ignore-file");
    p.write(
        "api/handler.rs",
        &format!("{directive}\nfn handler() {{}}\n"),
    );
    p.write("api/other.rs", "fn other() {}\n");
    let (doc, _) = rules_json(&p, p.path(), &["--files", "api/handler.rs", "api/other.rs"]);
    let got = names_and_paths(&doc);
    let both = vec!["api/handler.rs".to_string(), "api/other.rs".to_string()];
    assert!(got.contains(&("unscoped".into(), both.clone())), "{got:?}");
    assert!(
        got.contains(&("not_generated_code".into(), both)),
        "{got:?}"
    );
    assert!(
        got.contains(&("api_only".into(), vec!["api/other.rs".into()])),
        "{got:?}"
    );

    // Over the ignored file alone the rule drops out, as it does from `lint`.
    let (alone, _) = rules_json(&p, p.path(), &["--files", "api/handler.rs"]);
    assert!(!rule_names(&alone).contains(&"api_only"));
    assert_eq!(
        as_set(&alone),
        linted_rules(&p, p.path(), &["api/handler.rs"], "prompts")
    );
}

#[test]
fn a_malformed_ignore_directive_is_rejected_like_lint_never_honored() {
    let p = fixture();
    // A reason-less `ignore-file` (spelled in pieces so this file carries no
    // directive): the parser alone would still exempt the file, so without the
    // structural pre-flight `api_only` would silently drop out with exit 0.
    let directive = format!("// llmlint: {}[api_only]", "ignore-file");
    p.write(
        "api/handler.rs",
        &format!("{directive}\nfn handler() {{}}\n"),
    );
    let rules = p
        .bare()
        .args(["rules", "--files", "api/handler.rs", "--format", "json"])
        .assert()
        .code(2)
        .stdout("");
    let rules_err = String::from_utf8_lossy(&rules.get_output().stderr).into_owned();
    assert!(rules_err.contains("api/handler.rs:1"), "{rules_err}");
    // `lint` refuses the same file with the same diagnosis, before any judge.
    let lint = p.lint().args(["api/handler.rs"]).assert().code(2);
    let lint_err = String::from_utf8_lossy(&lint.get_output().stderr).into_owned();
    let problem = |e: &str| {
        e.lines()
            .find(|l| l.contains("api/handler.rs:1"))
            .map(str::trim)
            .map(String::from)
    };
    assert_eq!(problem(&rules_err), problem(&lint_err), "{lint_err}");

    // The same directive with its reason is honored, so the fix is the reason.
    p.write(
        "api/handler.rs",
        &format!("{directive} generated client\nfn handler() {{}}\n"),
    );
    let (doc, _) = rules_json(&p, p.path(), &["--files", "api/handler.rs"]);
    assert!(!rule_names(&doc).contains(&"api_only"), "{doc:?}");
}

#[test]
fn an_invalid_config_or_env_override_is_rejected_not_listed_over() {
    // A rule naming an agent nothing declares fails config validation.
    let bad = Project::new();
    bad.write(
        "llmlint.yml",
        "rules:\n  - { name: orphan, agent: nobody, description: \"Orphan holds.\" }\n",
    );
    bad.bare()
        .args(["rules", "--files", "a.rs", "--format", "json"])
        .assert()
        .code(2)
        .stdout("")
        .stderr(predicate::str::contains("nobody"));

    // A malformed `LLMLINT_*` override is located to its variable.
    let p = fixture();
    p.bare()
        .env("LLMLINT_MAX_PROMPT_CHARS", "lots")
        .args(["rules", "--files", "a.rs", "--format", "json"])
        .assert()
        .code(2)
        .stdout("")
        .stderr(predicate::str::contains("LLMLINT_MAX_PROMPT_CHARS"));
    // A malformed glob in the env exclude is refused the same way.
    p.bare()
        .env("LLMLINT_FILES_EXCLUDE", "[")
        .args(["rules", "--files", "a.rs", "--format", "json"])
        .assert()
        .code(2)
        .stdout("");
}

#[test]
fn rules_markdown_handles_framing_nested_headings_fences_and_no_match() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        "version: 1\n\
         agents:\n  \
           framed:\n    prompt_template: |\n      ## Reviewer stance\n      Read as an API owner.\n\
         rules:\n  \
         - name: documented\n    description: |\n      # Contract\n      \
         Each handler documents its errors.\n      ```sh\n      # not a heading\n      ```\n      \
         ~~~text\n      ```\n      # still code: a backtick line does not close a tilde fence\n      \
         ~~~\n      ## After the fences\n    \
         files: { include: [\"api/**\"] }\n  \
         - name: framed_rule\n    agent: framed\n    description: Framed invariant holds.\n    \
         files: { include: [\"api/**\"] }\n",
    );
    let md = |paths: &[&str]| {
        let out = p
            .bare()
            .args(["rules", "--format", "markdown", "--files"])
            .args(paths)
            .output()
            .unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap()
    };
    let text = md(&["api/h.rs"]);
    // The default agent sets no framing: its rules follow its heading directly.
    assert!(
        text.contains("### Agent `default`\n\n#### `documented`\n"),
        "{text}"
    );
    // A heading in the description is demoted below the rule's; one inside
    // fenced code is code, and stays as written.
    assert!(text.contains("\n##### Contract\n"), "{text}");
    assert!(text.contains("```sh\n# not a heading\n```"), "{text}");
    // A tilde fence is code too, and a backtick line inside it does not close
    // it; the heading after both fences is demoted again.
    assert!(
        text.contains(
            "~~~text\n```\n# still code: a backtick line does not close a tilde fence\n~~~\n"
        ),
        "{text}"
    );
    assert!(text.contains("\n###### After the fences\n"), "{text}");
    // A heading in an agent's framing is demoted too.
    assert!(
        text.contains("### Agent `framed`\n\n###### Reviewer stance\nRead as an API owner.\n"),
        "{text}"
    );
    assert!(text.contains("Judges: `api/h.rs`"), "{text}");

    assert_eq!(
        md(&["src/x.rs"]),
        "## llmlint rules for these paths\n\nNo llmlint rule judges these paths.\n"
    );
}

#[test]
fn rules_default_to_json_when_piped() {
    let p = fixture();
    let out = p
        .bare()
        .args(["rules", "--files", "api/handler.rs"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let doc: RulesDocument = serde_json::from_slice(&out.stdout).unwrap();
    assert!(!doc.rules.is_empty());
}

/// On a terminal the default is Markdown. `script` (util-linux) gives the
/// binary a real pseudo-terminal for stdout.
#[cfg(target_os = "linux")]
#[test]
fn rules_default_to_markdown_on_a_terminal() {
    let p = fixture();
    let bin = cargo_bin("llmlint");
    let inner = format!("'{}' rules --files api/handler.rs", bin.display());
    let out = p
        .wire(Command::new("script"))
        .args(["-qec", &inner, "/dev/null"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n");
    assert!(
        text.contains("## llmlint rules for these paths\n"),
        "{text}"
    );
    assert!(text.contains("#### `api_only`"), "{text}");
}
