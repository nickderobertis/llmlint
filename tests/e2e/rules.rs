//! `llmlint rules`: the rules that would judge a set of paths, read before any of
//! them is written. Each journey drives the real binary over a fixture config of
//! its own; the parity journeys run the real `lint` beside it, with the paid
//! model doubled at the harness boundary (the mock oneharness, recording every
//! prompt it received), and require the two to name the same rules.

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
    doc.rules.iter().map(|r| r.name.as_str()).collect()
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
    doc.rules.iter().map(|r| r.name.clone()).collect()
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
    let by_name = |n: &str| doc.rules.iter().find(|r| r.name == n).unwrap();

    let unscoped = by_name("unscoped");
    assert_eq!(unscoped.description, "Unscoped invariant holds.");
    assert_eq!(unscoped.agent, "reviewer");
    assert_eq!(unscoped.paths, ["api/handler.rs", "lib/core_gen.rs"]);
    let api = by_name("api_only");
    assert_eq!(
        (api.agent.as_str(), api.paths.as_slice()),
        ("reviewer", &["api/handler.rs".to_string()][..])
    );
    // The rule's own exclude glob keeps `lib/core_gen.rs` from it.
    let handwritten = by_name("not_generated_code");
    assert_eq!(handwritten.agent, "architect");
    assert_eq!(handwritten.paths, ["api/handler.rs"]);
    let plugin = by_name("plugin_rule");
    assert_eq!(plugin.description, "Plugin invariant holds.");
    assert_eq!(plugin.agent, "architect");

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
        .map(|a| (a.name.as_str(), a.framing.as_str()))
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
    let built = doc.rules.iter().find(|r| r.name == "unscoped").unwrap();
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
