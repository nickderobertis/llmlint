// llmlint: ignore-file[new_code_lands_in_a_project] a single binary crate with no Nx project graph (AGENTS.md) has no project for a test file to belong to; Cargo's [[test]] target is its home
//! Drift gate for what this repository *releases*, and the contract of the probe
//! that answers what a registry currently serves for it.
//!
//! A consumer that sequences work across repositories holds a dependent task
//! until the artifact it depends on is released. `release-targets.toml` at the
//! repository root is this repository's declaration of those artifacts and
//! `scripts/release-probe.sh` answers what a registry serves for each one.
//!
//! The declaration is written against the **canonical release-target schema**,
//! defined once in `docs/contract.md` of github.com/nickderobertis/onevcs;
//! [`schema`] restates it as a reader that refuses, so a dropped field or a
//! malformed identifier fails here rather than reaching a consumer that cannot
//! read it. That restatement is reconciled against the canonical implementation
//! (onevcs's `crates/onevcs/src/declaration.rs` and `releases.rs`) by the
//! `#[ignore]`-d network test
//! [`the_restated_schema_matches_the_canonical_definition`].
//!
//! The declaration is also the thing that goes stale silently, so this suite
//! never trusts it: it derives the published set from the *real* release
//! configuration — the workflows and the manifests they publish from — and fails
//! in both directions.
//!
//! The probe's answers are proven by driving the real script against a local
//! stand-in registry (the script honours a base-URL override). The one test that
//! reads the public registries is `#[ignore]`-d so the gate stays offline; run
//! it with `just test-release-targets`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use schema::Declaration;

/// The canonical release-target schema (`schema_version = 1`), as a reader that
/// refuses. Deliberately narrow: the keys, alphabets, and document-level refusals
/// the canonical contract fixes, and nothing beside them.
mod schema {
    use std::collections::BTreeSet;

    use serde::Deserialize;

    /// The schema version this gate reads, and the oldest it accepts.
    pub const SCHEMA_VERSION: i64 = 1;
    pub const MAX_PROSE: usize = 400;
    pub const MAX_IDENTIFIER: usize = 128;
    pub const MAX_TARGET_NAME: usize = 64;

    pub const TOP_LEVEL_KEYS: [&str; 4] = ["schema_version", "probe", "target", "retired"];
    pub const TARGET_KEYS: [&str; 6] = ["id", "name", "what", "published_by", "manifest", "covers"];
    pub const RETIRED_KEYS: [&str; 2] = ["id", "why"];

    /// What one repository publishes, as its `release-targets.toml` declares it.
    #[derive(Debug, Deserialize)]
    pub struct Declaration {
        pub schema_version: i64,
        #[serde(default)]
        pub probe: Option<String>,
        #[serde(rename = "target", default)]
        pub targets: Vec<DeclaredTarget>,
        #[serde(rename = "retired", default)]
        pub retired: Vec<RetiredArtifact>,
    }

    /// One consumable artifact: something a dependent names to depend on it.
    #[derive(Debug, Deserialize)]
    pub struct DeclaredTarget {
        /// `<registry>:<name>`, where `<name>` is exactly what that registry serves.
        pub id: String,
        /// The short name a consumer's plan waits on this target by.
        pub name: String,
        pub what: String,
        /// The workflow and job that publish it.
        pub published_by: String,
        #[serde(default)]
        pub manifest: Option<String>,
        #[serde(default)]
        pub covers: Vec<String>,
    }

    /// Something this repository once published and does not publish again.
    #[derive(Debug, Deserialize)]
    pub struct RetiredArtifact {
        pub id: String,
        pub why: String,
    }

    /// Read one declaration's text, or say what is wrong with it.
    pub fn parse(document: &str, origin: &str) -> Result<Declaration, String> {
        let value: toml::Value = toml::from_str(document)
            .map_err(|e| format!("the release declaration at {origin} is not TOML: {e}"))?;
        let Some(declared) = value
            .get("schema_version")
            .and_then(toml::Value::as_integer)
        else {
            return Err(format!(
                "the release declaration at {origin} declares no schema_version"
            ));
        };
        if declared < SCHEMA_VERSION {
            return Err(format!(
                "the release declaration at {origin} declares schema_version {declared}; this \
                 gate reads schema_version {SCHEMA_VERSION} and newer"
            ));
        }
        // A later schema's keys are read leniently; at the version this gate knows,
        // a misspelled key is refused rather than read as an absent one.
        if declared == SCHEMA_VERSION {
            refuse_unknown_keys(&value, origin)?;
        }
        let declaration: Declaration = toml::from_str(document).map_err(|e| {
            format!(
                "the release declaration at {origin} is not the shape schema_version \
                 {SCHEMA_VERSION} declares: {e}"
            )
        })?;
        validate(&declaration)
            .map_err(|e| format!("the release declaration at {origin} is refused: {e}"))?;
        Ok(declaration)
    }

    fn refuse_unknown_keys(document: &toml::Value, origin: &str) -> Result<(), String> {
        let unknown = |table: &str, key: &str| {
            format!(
                "the release declaration at {origin} names {key:?} in {table}, which \
                 schema_version {SCHEMA_VERSION} does not declare"
            )
        };
        let top = document
            .as_table()
            .ok_or_else(|| format!("the release declaration at {origin} is not a table"))?;
        for key in top.keys() {
            if !TOP_LEVEL_KEYS.contains(&key.as_str()) {
                return Err(unknown("the document", key));
            }
        }
        for (array, keys) in [("target", &TARGET_KEYS[..]), ("retired", &RETIRED_KEYS[..])] {
            let entries = top.get(array).and_then(toml::Value::as_array);
            for (index, entry) in entries.into_iter().flatten().enumerate() {
                for key in entry.as_table().into_iter().flat_map(|t| t.keys()) {
                    if !keys.contains(&key.as_str()) {
                        return Err(unknown(&format!("[[{array}]] {}", index + 1), key));
                    }
                }
            }
        }
        Ok(())
    }

    /// `<registry>:<name>`: one colon, a lowercase registry word, and a name in
    /// the alphabet every registry serves.
    fn registry_id(value: &str) -> Result<(), String> {
        if value.len() > MAX_IDENTIFIER {
            return Err(format!("the identifier {value:?} is too long"));
        }
        let Some((registry, name)) = value.split_once(':') else {
            return Err(format!(
                "the identifier {value:?} names no registry; spell it <registry>:<name>"
            ));
        };
        if registry.is_empty()
            || !registry
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(format!(
                "the identifier {value:?} names the registry {registry:?}, which is not one \
                 word of lowercase letters, digits, and '-'"
            ));
        }
        if !name.starts_with(|c: char| c.is_ascii_alphanumeric())
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@' | '/'))
        {
            return Err(format!(
                "the identifier {value:?} names {name:?}, which is not a name a registry serves"
            ));
        }
        Ok(())
    }

    fn target_name(value: &str) -> Result<(), String> {
        if value.is_empty()
            || value.len() > MAX_TARGET_NAME
            || !value.starts_with(|c: char| c.is_ascii_alphanumeric())
            || !value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err(format!(
                "the target name {value:?} is not 1-{MAX_TARGET_NAME} letters, digits, '-', \
                 '_', or '.' starting with a letter or digit"
            ));
        }
        Ok(())
    }

    fn prose(value: &str) -> Result<(), String> {
        if value.trim().is_empty() {
            return Err("`what`, `published_by`, and `why` may not be blank".to_owned());
        }
        if value.len() > MAX_PROSE || value.chars().any(char::is_control) {
            return Err(format!(
                "the prose {value:?} is not one line of at most {MAX_PROSE} characters"
            ));
        }
        Ok(())
    }

    /// A path relative to the repository root, judged by its spelling alone.
    fn repository_path(value: &str) -> Result<(), String> {
        const SEPARATORS: [char; 2] = ['/', '\\'];
        let mut chars = value.chars();
        let drive = matches!(
            (chars.next(), chars.next()),
            (Some(drive), Some(':')) if drive.is_ascii_alphabetic()
        );
        if value.is_empty()
            || value.starts_with(SEPARATORS)
            || drive
            || value.split(SEPARATORS).any(|component| component == "..")
        {
            return Err(format!(
                "the path {value:?} is not a path inside the repository, relative to its root"
            ));
        }
        Ok(())
    }

    fn validate(declaration: &Declaration) -> Result<(), String> {
        if let Some(probe) = &declaration.probe {
            repository_path(probe)?;
        }
        if declaration.targets.is_empty() {
            return Err("it declares no [[target]]".to_owned());
        }
        let mut names = BTreeSet::new();
        let mut ids = BTreeSet::new();
        let mut covered = BTreeSet::new();
        for target in &declaration.targets {
            let at = |e: String| format!("[[target]] {:?}: {e}", target.id);
            registry_id(&target.id).map_err(at)?;
            target_name(&target.name).map_err(at)?;
            prose(&target.what).map_err(at)?;
            prose(&target.published_by).map_err(at)?;
            if let Some(manifest) = &target.manifest {
                repository_path(manifest).map_err(at)?;
            }
            if !names.insert(target.name.as_str()) {
                return Err(at(format!(
                    "the short name {:?} is taken twice",
                    target.name
                )));
            }
            if !ids.insert(target.id.as_str()) {
                return Err(at("the identifier is declared twice".to_owned()));
            }
            for id in &target.covers {
                registry_id(id).map_err(at)?;
                if *id == target.id || !covered.insert(id.as_str()) {
                    return Err(at(format!("{id:?} is covered twice or covers itself")));
                }
            }
        }
        for retired in &declaration.retired {
            registry_id(&retired.id)?;
            prose(&retired.why)?;
            if ids.contains(retired.id.as_str()) {
                return Err(format!("{:?} is both a target and retired", retired.id));
            }
        }
        Ok(())
    }
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

fn declaration_text() -> String {
    read(&repo_root().join("release-targets.toml"))
}

/// The real release configuration a published set is derived from.
struct ReleaseConfig {
    /// `(repo-relative workflow path, text)` for every workflow.
    workflows: Vec<(String, String)>,
    cargo_toml: String,
    pyproject: String,
    release_plz: String,
}

impl ReleaseConfig {
    fn real() -> Self {
        let root = repo_root();
        let mut workflows: Vec<(String, String)> = fs::read_dir(root.join(".github/workflows"))
            .expect("reading .github/workflows")
            .map(|entry| entry.expect("workflow dir entry").path())
            .filter(|path| {
                matches!(
                    path.extension().and_then(|e| e.to_str()),
                    Some("yml" | "yaml")
                )
            })
            .map(|path| {
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                (format!(".github/workflows/{name}"), read(&path))
            })
            .collect();
        workflows.sort();
        Self {
            workflows,
            cargo_toml: read(&root.join("Cargo.toml")),
            pyproject: read(&root.join("pyproject.toml")),
            release_plz: read(&root.join("release-plz.toml")),
        }
    }

    fn with_workflow(mut self, path: &str, edit: impl Fn(&str) -> String) -> Self {
        let entry = self
            .workflows
            .iter_mut()
            .find(|(p, _)| p == path)
            .unwrap_or_else(|| panic!("no workflow {path}"));
        let edited = edit(&entry.1);
        assert_ne!(edited, entry.1, "the edit to {path} changed nothing");
        entry.1 = edited;
        self
    }
}

/// Who publishes one registry-qualified artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Publisher {
    workflow: String,
    job: String,
    manifest: &'static str,
}

fn toml_field(document: &str, origin: &str, table: &str, key: &str) -> Option<String> {
    let value: toml::Value =
        toml::from_str(document).unwrap_or_else(|e| panic!("{origin} is not TOML: {e}"));
    value.get(table)?.get(key)?.as_str().map(str::to_owned)
}

/// The registry-qualified artifacts the workflows publish, derived from the real
/// release configuration rather than an inventory transcribed into this test:
///
/// * crates.io — a `cargo publish` step (not `--dry-run`) publishes Cargo.toml's
///   `[package] name`; so does a `release-plz release` step unless
///   `release-plz.toml` sets `publish = false`.
/// * PyPI — a `pypa/gh-action-pypi-publish` step publishes pyproject.toml's
///   `[project] name` (the one distribution this tree builds).
///
/// Any other registry-publishing command is an error: a kind of artifact this
/// gate does not derive has to be derived and declared, not shipped unseen.
fn published(config: &ReleaseConfig) -> Result<BTreeMap<String, Vec<Publisher>>, String> {
    let crate_name = toml_field(&config.cargo_toml, "Cargo.toml", "package", "name")
        .ok_or("Cargo.toml has no [package] name")?;
    let dist_name = toml_field(&config.pyproject, "pyproject.toml", "project", "name")
        .ok_or("pyproject.toml has no [project] name")?;
    let release_plz_publishes =
        toml_field_bool(&config.release_plz, "workspace", "publish").unwrap_or(true);

    let mut out: BTreeMap<String, Vec<Publisher>> = BTreeMap::new();
    for (path, text) in &config.workflows {
        let doc: serde_yaml_ng::Value =
            serde_yaml_ng::from_str(text).map_err(|e| format!("{path} is not YAML: {e}"))?;
        let Some(jobs) = doc.get("jobs").and_then(serde_yaml_ng::Value::as_mapping) else {
            continue;
        };
        for (job, body) in jobs {
            let job = job
                .as_str()
                .ok_or_else(|| format!("{path}: a job key is not a string"))?;
            let steps = body
                .get("steps")
                .and_then(serde_yaml_ng::Value::as_sequence);
            for step in steps.into_iter().flatten() {
                let run = step
                    .get("run")
                    .and_then(serde_yaml_ng::Value::as_str)
                    .unwrap_or("");
                let uses = step
                    .get("uses")
                    .and_then(serde_yaml_ng::Value::as_str)
                    .unwrap_or("");
                let words: Vec<&str> = run.split_whitespace().collect();
                let has = |a: &str, b: &str| words.windows(2).any(|w| w[0] == a && w[1] == b);
                let mut publish = |id: String, manifest: &'static str| {
                    out.entry(id).or_default().push(Publisher {
                        workflow: path.clone(),
                        job: job.to_owned(),
                        manifest,
                    });
                };
                if has("cargo", "publish") && !words.contains(&"--dry-run") {
                    publish(format!("crate:{crate_name}"), "Cargo.toml");
                }
                if has("release-plz", "release") && release_plz_publishes {
                    publish(format!("crate:{crate_name}"), "Cargo.toml");
                }
                if uses.starts_with("pypa/gh-action-pypi-publish") {
                    publish(format!("pypi:{dist_name}"), "pyproject.toml");
                }
                for (a, b) in [
                    ("npm", "publish"),
                    ("pnpm", "publish"),
                    ("yarn", "publish"),
                    ("twine", "upload"),
                    ("maturin", "publish"),
                    ("maturin", "upload"),
                    ("uv", "publish"),
                    ("gem", "push"),
                ] {
                    if has(a, b) {
                        return Err(format!(
                            "{path} job `{job}` runs `{a} {b}`, a registry publish this gate \
                             does not derive; derive it here and declare it in \
                             release-targets.toml"
                        ));
                    }
                }
            }
        }
    }
    Ok(out)
}

fn toml_field_bool(document: &str, table: &str, key: &str) -> Option<bool> {
    let value: toml::Value = toml::from_str(document).ok()?;
    value.get(table)?.get(key)?.as_bool()
}

/// Where a declaration and the release configuration disagree, in both
/// directions. Empty is agreement.
fn drift(declaration: &Declaration, config: &ReleaseConfig) -> Vec<String> {
    let published = match published(config) {
        Ok(published) => published,
        Err(e) => return vec![e],
    };
    let mut problems = Vec::new();
    let declared: BTreeSet<&str> = declaration.targets.iter().map(|t| t.id.as_str()).collect();
    for (id, publishers) in &published {
        if !declared.contains(id.as_str()) {
            let by: Vec<String> = publishers
                .iter()
                .map(|p| format!("{} job `{}`", p.workflow, p.job))
                .collect();
            problems.push(format!(
                "{id} is published by {} but release-targets.toml declares no target for it",
                by.join(", ")
            ));
        }
    }
    for target in &declaration.targets {
        let Some(publishers) = published.get(&target.id) else {
            problems.push(format!(
                "{} is declared but nothing in the workflows publishes it (check the \
                 manifest's package name and the publishing step)",
                target.id
            ));
            continue;
        };
        for p in publishers {
            if !target.published_by.contains(&p.workflow)
                || !target.published_by.contains(&format!("`{}`", p.job))
            {
                problems.push(format!(
                    "{}'s published_by ({:?}) does not name its publisher: {} job `{}`",
                    target.id, target.published_by, p.workflow, p.job
                ));
            }
            if target.manifest.as_deref() != Some(p.manifest) {
                problems.push(format!(
                    "{}'s manifest is {:?}, but it is published from {}",
                    target.id, target.manifest, p.manifest
                ));
            }
        }
    }
    problems
}

/// The document conforms to the canonical schema and names exactly the two
/// targets consumers wait on, by the short names they wait on them by.
#[test]
fn the_declaration_conforms_to_the_canonical_schema() {
    let declaration = schema::parse(&declaration_text(), "release-targets.toml").unwrap();
    assert_eq!(declaration.schema_version, schema::SCHEMA_VERSION);
    assert_eq!(
        declaration.probe.as_deref(),
        Some("scripts/release-probe.sh")
    );
    assert!(repo_root().join("scripts/release-probe.sh").is_file());
    let targets: Vec<(&str, &str, Option<&str>)> = declaration
        .targets
        .iter()
        .map(|t| (t.id.as_str(), t.name.as_str(), t.manifest.as_deref()))
        .collect();
    assert_eq!(
        targets,
        [
            ("crate:llmlint", "crate", Some("Cargo.toml")),
            ("pypi:llmlint-cli", "cli", Some("pyproject.toml")),
        ]
    );
    for target in &declaration.targets {
        let manifest = target.manifest.as_deref().unwrap();
        assert!(
            repo_root().join(manifest).is_file(),
            "{manifest} is missing"
        );
    }
    assert!(declaration.retired.is_empty());
}

/// The whole point: the declaration and the real release configuration agree —
/// every published artifact is declared, and every declared one is published by
/// the job and from the manifest the declaration names.
#[test]
fn the_declaration_matches_the_real_release_configuration() {
    let declaration = schema::parse(&declaration_text(), "release-targets.toml").unwrap();
    let config = ReleaseConfig::real();
    assert_eq!(drift(&declaration, &config), Vec::<String>::new());
    let published = published(&config).unwrap();
    assert_eq!(
        published["crate:llmlint"],
        [Publisher {
            workflow: ".github/workflows/release.yml".into(),
            job: "publish-crate".into(),
            manifest: "Cargo.toml",
        }]
    );
    assert_eq!(
        published["pypi:llmlint-cli"],
        [Publisher {
            workflow: ".github/workflows/release.yml".into(),
            job: "publish-pypi".into(),
            manifest: "pyproject.toml",
        }]
    );
}

/// The drift check fails in both directions, driven over the real release
/// configuration edited to really disagree with the real declaration.
#[test]
fn the_drift_check_fails_a_declaration_that_disagrees_with_release_yml() {
    let declaration = schema::parse(&declaration_text(), "release-targets.toml").unwrap();
    let release = ".github/workflows/release.yml";
    let expect = |config: ReleaseConfig, needle: &str| {
        let problems = drift(&declaration, &config);
        assert!(
            problems.iter().any(|p| p.contains(needle)),
            "expected a problem mentioning {needle:?}, got {problems:#?}"
        );
    };

    // The crate's publishing job renamed: published_by no longer names it.
    expect(
        ReleaseConfig::real().with_workflow(release, |t| {
            t.replace("\n  publish-crate:\n", "\n  ship-crate:\n")
        }),
        "does not name its publisher: .github/workflows/release.yml job `ship-crate`",
    );
    // The wheel's publishing job renamed.
    expect(
        ReleaseConfig::real().with_workflow(release, |t| {
            t.replace("\n  publish-pypi:\n", "\n  upload-pypi:\n")
        }),
        "job `upload-pypi`",
    );
    // A manifest's package name no longer matches the declared id: both directions.
    let mut renamed = ReleaseConfig::real();
    renamed.cargo_toml =
        renamed
            .cargo_toml
            .replacen("name = \"llmlint\"", "name = \"llmlint-core\"", 1);
    expect(renamed, "crate:llmlint-core is published by");
    let mut renamed = ReleaseConfig::real();
    renamed.pyproject =
        renamed
            .pyproject
            .replacen("name = \"llmlint-cli\"", "name = \"llmlint-bin\"", 1);
    expect(
        renamed,
        "pypi:llmlint-cli is declared but nothing in the workflows publishes it",
    );
    // release.yml publishes an artifact the declaration does not name.
    expect(
        ReleaseConfig::real().with_workflow(release, |t| {
            t.replace(
                "      - run: cargo publish --locked\n",
                "      - run: cargo publish --locked\n      - run: npm publish\n",
            )
        }),
        "runs `npm publish`",
    );
    // release-plz turned into a second crate publisher.
    let mut plz = ReleaseConfig::real();
    plz.release_plz = plz.release_plz.replace("publish = false", "publish = true");
    expect(plz, ".github/workflows/release-plz.yml job `release-plz`");
    // The publishing step removed: a declared target nothing publishes.
    expect(
        ReleaseConfig::real().with_workflow(release, |t| {
            t.replace("      - uses: pypa/gh-action-pypi-publish@release/v1\n        with:\n          packages-dir: dist\n", "")
        }),
        "pypi:llmlint-cli is declared but nothing",
    );
}

/// Network tier: the schema [`schema`] restates has not moved upstream.
///
/// The restatement is of a contract this repository does not own, so it is the
/// one thing here that can drift silently. It is reconciled against the
/// *implementation* a consumer's reader enforces — its constants, its version-1
/// key lists, and the expressions its rules are made of — and against the
/// readable range, so the day onevcs stops reading `schema_version = 1` this
/// fails rather than a consumer refusing what this repository publishes.
/// `#[ignore]`-d like the other network test; run via `just test-release-targets`.
#[test]
#[ignore = "network: reads nickderobertis/onevcs; run via `just test-release-targets`"]
fn the_restated_schema_matches_the_canonical_definition() {
    const CANONICAL: &str =
        "https://raw.githubusercontent.com/nickderobertis/onevcs/HEAD/crates/onevcs/src";

    fn upstream(file: &str) -> String {
        let url = format!("{CANONICAL}/{file}");
        let output = std::process::Command::new("curl")
            .args(["-q", "--silent", "--show-error", "--fail", "--location"])
            .args(["--max-time", "30", &url])
            .output()
            .unwrap_or_else(|e| panic!("curl is needed to read {url}: {e}"));
        assert!(
            output.status.success(),
            "could not read the canonical schema at {url}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("the canonical schema is UTF-8")
    }

    /// The value of `const <name>` in a Rust source file, from `=` to `;`,
    /// joined across lines.
    fn constant(source: &str, origin: &str, name: &str) -> String {
        let start = [format!("\nconst {name}:"), format!("\npub const {name}:")]
            .iter()
            .find_map(|decl| source.find(decl.as_str()))
            .unwrap_or_else(|| {
                panic!(
                    "{origin} no longer declares `{name}`; reread this suite's schema against it"
                )
            });
        // After the `=`, so a `;` in the type (`[&str; 4]`) is not the end.
        let rest = &source[start..];
        let rest = &rest[rest.find('=').unwrap() + 1..];
        let value = &rest[..rest.find(';').unwrap()];
        value.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    fn number(source: &str, origin: &str, name: &str) -> i64 {
        constant(source, origin, name)
            .parse()
            .unwrap_or_else(|e| panic!("{origin}'s `{name}` is not a number: {e}"))
    }

    /// Every quoted string of a Rust array literal.
    fn strings(literal: &str) -> Vec<String> {
        literal
            .split('"')
            .skip(1)
            .step_by(2)
            .map(str::to_owned)
            .collect()
    }

    let (declaration, declaration_rs) = (upstream("declaration.rs"), "declaration.rs");
    let (releases, releases_rs) = (upstream("releases.rs"), "releases.rs");

    let oldest = number(&declaration, declaration_rs, "OLDEST_SCHEMA_VERSION");
    let newest = number(&declaration, declaration_rs, "SCHEMA_VERSION");
    assert!(
        (oldest..=newest).contains(&schema::SCHEMA_VERSION),
        "onevcs reads schema_version {oldest}..={newest}; release-targets.toml declares {}",
        schema::SCHEMA_VERSION
    );
    for (source, origin, name, restated) in [
        (&declaration, declaration_rs, "MAX_PROSE", schema::MAX_PROSE),
        (
            &declaration,
            declaration_rs,
            "MAX_IDENTIFIER",
            schema::MAX_IDENTIFIER,
        ),
        (
            &releases,
            releases_rs,
            "MAX_TARGET_NAME",
            schema::MAX_TARGET_NAME,
        ),
    ] {
        let canonical = number(source, origin, name);
        assert_eq!(
            restated as i64, canonical,
            "{origin} declares {name} = {canonical}"
        );
    }
    for (name, restated) in [
        ("TOP_LEVEL_KEYS", &schema::TOP_LEVEL_KEYS[..]),
        ("TARGET_KEYS", &schema::TARGET_KEYS[..]),
        ("RETIRED_KEYS", &schema::RETIRED_KEYS[..]),
    ] {
        let canonical = strings(&constant(&declaration, declaration_rs, name));
        assert_eq!(
            restated, canonical,
            "{declaration_rs} declares {name} = {canonical:?}"
        );
    }
    // The rules themselves: each is the expression one canonical check is made
    // of, restated verbatim in `schema`.
    for (source, origin, expression) in [
        (
            &declaration,
            declaration_rs,
            "c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'",
        ),
        (
            &declaration,
            declaration_rs,
            "c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@' | '/')",
        ),
        (
            &releases,
            releases_rs,
            "c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')",
        ),
        (&declaration, declaration_rs, "value.trim().is_empty()"),
        (
            &declaration,
            declaration_rs,
            "value.chars().any(char::is_control)",
        ),
        (
            &declaration,
            declaration_rs,
            "const SEPARATORS: [char; 2] = ['/', '\\\\'];",
        ),
        (&declaration, declaration_rs, "component == \"..\""),
        (
            &declaration,
            declaration_rs,
            "(Some(drive), Some(':')) if drive.is_ascii_alphabetic()",
        ),
    ] {
        assert!(
            source.contains(expression),
            "{origin} no longer contains `{expression}`; the canonical rule changed and \
             `schema` restates the one it replaced"
        );
    }
}

/// A document missing what the schema requires is refused with a message
/// naming the defect.
#[test]
fn a_document_that_does_not_conform_is_refused() {
    let good = declaration_text();
    let cases: [(&str, String, &str); 8] = [
        (
            "no version",
            good.replace("schema_version = 1\n", ""),
            "declares no schema_version",
        ),
        (
            "older version",
            good.replace("schema_version = 1", "schema_version = 0"),
            "reads schema_version 1 and newer",
        ),
        (
            "misspelled key",
            good.replacen("manifest =", "manifset =", 1),
            "\"manifset\"",
        ),
        (
            "unqualified id",
            good.replace("\"crate:llmlint\"", "\"llmlint\""),
            "names no registry",
        ),
        (
            "duplicate name",
            good.replace("name = \"cli\"", "name = \"crate\""),
            "taken twice",
        ),
        (
            "absolute probe",
            good.replace("\"scripts/release-probe.sh\"", "\"/usr/bin/probe\""),
            "not a path inside the repository",
        ),
        (
            "blank what",
            good.replacen("what = \"The llmlint crate", "what = \" \" #", 1),
            "may not be blank",
        ),
        (
            "no targets",
            "schema_version = 1\n".to_owned(),
            "declares no [[target]]",
        ),
    ];
    for (case, document, needle) in cases {
        assert_ne!(document, good, "{case}: the edit changed nothing");
        let error = schema::parse(&document, "fixture").expect_err(case);
        assert!(error.contains(needle), "{case}: {error}");
    }
    // A later schema's unknown keys are read leniently.
    let later = good
        .replace("schema_version = 1", "schema_version = 2")
        .replacen("manifest =", "channel = \"stable\"\nmanifest =", 1);
    schema::parse(&later, "fixture").expect("a later schema is read leniently");
}

/// The probe, driven for real: a bash script run with a scrubbed environment
/// against a stand-in registry on localhost. Unix-only, like the repo's other
/// subprocess-script tests.
#[cfg(unix)]
mod probe {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::process::{Command, Output};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    use tempfile::TempDir;

    use super::repo_root;

    const CRATES_URL: &str = "LLMLINT_RELEASE_PROBE_CRATES_URL";
    const PYPI_URL: &str = "LLMLINT_RELEASE_PROBE_PYPI_URL";
    /// The bound every answer must arrive within.
    const BOUND: Duration = Duration::from_secs(60);

    /// How the stand-in registry answers every request.
    #[derive(Clone)]
    enum Reply {
        Status(u16, &'static str),
        /// Accept the connection, read the request, and never respond.
        Stall,
    }

    /// A stand-in registry on 127.0.0.1 that records every request it receives.
    struct Registry {
        url: String,
        requests: Arc<Mutex<Vec<String>>>,
    }

    fn registry(reply: Reply) -> Registry {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&requests);
        thread::spawn(move || {
            let mut held: Vec<TcpStream> = Vec::new();
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                let mut buf = [0u8; 4096];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => request.extend_from_slice(&buf[..n]),
                    }
                }
                log.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&request).into_owned());
                match &reply {
                    Reply::Stall => held.push(stream),
                    Reply::Status(status, body) => {
                        let _ = write!(
                            stream,
                            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n\
                             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                    }
                }
            }
        });
        Registry { url, requests }
    }

    /// Run the real probe from the repository root with nothing in its
    /// environment but PATH, a scratch HOME, and `extra`.
    fn run(args: &[&str], extra: &[(&str, &str)]) -> (Output, Duration, TempDir) {
        let home = TempDir::new().unwrap();
        let mut command = Command::new(repo_root().join("scripts/release-probe.sh"));
        command
            .args(args)
            .current_dir(repo_root())
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", home.path());
        for (key, value) in extra {
            command.env(key, value);
        }
        let started = Instant::now();
        let output = command.output().expect("spawning scripts/release-probe.sh");
        (output, started.elapsed(), home)
    }

    fn against(registry: &Registry, id: &str) -> (Output, Duration) {
        let (output, elapsed, _home) = run(
            &[id],
            &[(CRATES_URL, &registry.url), (PYPI_URL, &registry.url)],
        );
        (output, elapsed)
    }

    fn stdout(output: &Output) -> String {
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn stderr(output: &Output) -> String {
        String::from_utf8_lossy(&output.stderr).into_owned()
    }

    fn assert_version(output: &Output, elapsed: Duration, version: &str) {
        assert!(output.status.success(), "stderr: {}", stderr(output));
        assert_eq!(stdout(output), format!("{version}\n"));
        assert!(elapsed < BOUND, "took {elapsed:?}");
    }

    fn assert_no_release_yet(output: &Output, elapsed: Duration) {
        assert!(output.status.success(), "stderr: {}", stderr(output));
        assert_eq!(stdout(output), "", "no release yet is empty stdout");
        assert!(elapsed < BOUND, "took {elapsed:?}");
    }

    fn assert_not_answered(output: &Output, elapsed: Duration, reason: &str) {
        assert!(!output.status.success(), "not answered must exit non-zero");
        assert_eq!(stdout(output), "", "not answered must leave stdout empty");
        let err = stderr(output);
        assert!(
            err.contains(reason),
            "expected {reason:?} on stderr, got {err:?}"
        );
        assert!(elapsed < BOUND, "took {elapsed:?}");
    }

    const CRATE_BODY: &str = r#"{"crate":{"name":"llmlint","max_stable_version":"0.4.2","newest_version":"0.5.0-rc.1"}}"#;
    const PYPI_BODY: &str = r#"{"info":{"name":"llmlint-cli","version":"0.4.2"}}"#;

    #[test]
    fn the_crate_answers_the_stable_version_crates_io_serves() {
        let registry = registry(Reply::Status(200, CRATE_BODY));
        let (output, elapsed) = against(&registry, "crate:llmlint");
        assert_version(&output, elapsed, "0.4.2");
        let requests = registry.requests.lock().unwrap();
        assert!(
            requests[0].starts_with("GET /api/v1/crates/llmlint HTTP/1.1\r\n"),
            "{requests:?}"
        );
    }

    #[test]
    fn a_crate_with_only_prereleases_answers_its_newest_version() {
        let registry = registry(Reply::Status(
            200,
            r#"{"crate":{"max_stable_version":null,"newest_version":"0.1.0-alpha.1"}}"#,
        ));
        let (output, elapsed) = against(&registry, "crate:llmlint");
        assert_version(&output, elapsed, "0.1.0-alpha.1");
    }

    #[test]
    fn the_wheel_answers_the_version_pypi_serves() {
        let registry = registry(Reply::Status(200, PYPI_BODY));
        let (output, elapsed) = against(&registry, "pypi:llmlint-cli");
        assert_version(&output, elapsed, "0.4.2");
        let requests = registry.requests.lock().unwrap();
        assert!(
            requests[0].starts_with("GET /pypi/llmlint-cli/json HTTP/1.1\r\n"),
            "{requests:?}"
        );
    }

    #[test]
    fn a_registry_with_no_release_answers_no_release_yet() {
        for id in ["crate:llmlint", "pypi:llmlint-cli"] {
            let registry = registry(Reply::Status(404, r#"{"errors":[{"detail":"Not Found"}]}"#));
            let (output, elapsed) = against(&registry, id);
            assert_no_release_yet(&output, elapsed);
        }
    }

    #[test]
    fn a_registry_error_status_is_not_answered() {
        for status in [500, 403] {
            let registry = registry(Reply::Status(status, "{}"));
            let (output, elapsed) = against(&registry, "pypi:llmlint-cli");
            assert_not_answered(&output, elapsed, &format!("answered HTTP {status}"));
        }
    }

    #[test]
    fn an_unreadable_version_is_not_answered() {
        for body in [
            "not json",
            r#"{"info":{}}"#,
            r#"{"info":{"version":""}}"#,
            r#"{"info":{"version":"1.0\n2.0"}}"#,
        ] {
            let registry = registry(Reply::Status(200, body));
            let (output, elapsed) = against(&registry, "pypi:llmlint-cli");
            assert_not_answered(&output, elapsed, "with no version this probe could read");
        }
    }

    #[test]
    fn an_unreachable_registry_is_not_answered() {
        // Bind then drop: nothing listens on the port, so the connection is refused.
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let url = format!("http://127.0.0.1:{port}");
        let (output, elapsed, _home) = run(&["crate:llmlint"], &[(CRATES_URL, &url)]);
        assert_not_answered(&output, elapsed, "could not read");
    }

    #[test]
    fn a_registry_that_stalls_is_not_answered_within_the_bound() {
        let registry = registry(Reply::Stall);
        let (output, elapsed) = against(&registry, "crate:llmlint");
        assert_not_answered(&output, elapsed, "could not read");
        assert!(
            !registry.requests.lock().unwrap().is_empty(),
            "the connection was accepted"
        );
    }

    /// Every run above already has every credential variable unset (the
    /// environment is cleared); this one asserts it answers regardless, and that
    /// credentials planted in the environment, a ~/.curlrc, and a ~/.netrc never
    /// reach the registry.
    #[test]
    fn it_answers_with_no_credential_and_sends_none_it_finds() {
        let registry = registry(Reply::Status(200, CRATE_BODY));
        let (output, elapsed, _home) = run(&["crate:llmlint"], &[(CRATES_URL, &registry.url)]);
        assert_version(&output, elapsed, "0.4.2");

        const SECRET: &str = "sekrit-probe-credential";
        let home = TempDir::new().unwrap();
        std::fs::write(
            home.path().join(".curlrc"),
            format!("header = \"Authorization: Bearer {SECRET}\"\nnetrc\n"),
        )
        .unwrap();
        std::fs::write(
            home.path().join(".netrc"),
            format!("machine 127.0.0.1 login user password {SECRET}\n"),
        )
        .unwrap();
        let home_path = home.path().to_str().unwrap().to_owned();
        let mut env = vec![
            (CRATES_URL, registry.url.as_str()),
            ("HOME", home_path.as_str()),
        ];
        for var in [
            "CARGO_REGISTRY_TOKEN",
            "CARGO_REGISTRIES_CRATES_IO_TOKEN",
            "PYPI_TOKEN",
            "PYPI_API_TOKEN",
            "TWINE_USERNAME",
            "TWINE_PASSWORD",
            "GITHUB_TOKEN",
            "GH_TOKEN",
            "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
        ] {
            env.push((var, SECRET));
        }
        let (output, elapsed, _scratch) = run(&["crate:llmlint"], &env);
        assert_version(&output, elapsed, "0.4.2");
        let requests = registry.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        for request in requests.iter() {
            assert!(
                !request.contains(SECRET),
                "a credential reached the registry: {request}"
            );
            assert!(
                !request.to_ascii_lowercase().contains("authorization:"),
                "{request}"
            );
        }
    }

    /// No registry is involved: each of these is refused before any lookup.
    #[test]
    fn an_identifier_other_than_the_declared_two_is_not_answered() {
        // A registry that would answer, so a wrong answer could not hide behind a
        // lookup failure.
        let registry = registry(Reply::Status(200, CRATE_BODY));
        for (id, reason) in [
            ("llmlint", "expected a registry-qualified"),
            ("npm:llmlint", "publishes to crate: and pypi: only"),
            ("crate:", "is not a registry artifact name"),
            ("crate:../llmlint", "is not a registry artifact name"),
            ("pypi:llm lint", "is not a registry artifact name"),
            ("crate:serde", "not a release target of this repository"),
            ("pypi:llmlint", "not a release target of this repository"),
            (
                "pypi:onejudge-cli",
                "not a release target of this repository",
            ),
        ] {
            let (output, elapsed) = against(&registry, id);
            assert_not_answered(&output, elapsed, reason);
        }
        assert!(
            registry.requests.lock().unwrap().is_empty(),
            "no lookup was made"
        );
    }

    /// A base-URL override is validated at the boundary: anything but a bare
    /// http(s) origin is not answered, and no request is made.
    #[test]
    fn a_malformed_registry_override_is_not_answered() {
        for base in [
            "file:///etc/passwd",
            "http://user:secret@127.0.0.1:9",
            "http://127.0.0.1:9/elsewhere?x=1",
            "crates.io",
        ] {
            let (output, elapsed, _home) = run(&["crate:llmlint"], &[(CRATES_URL, base)]);
            assert_not_answered(
                &output,
                elapsed,
                "is not a bare http(s)://host[:port] origin",
            );
        }
    }

    #[test]
    fn the_probe_takes_exactly_one_identifier() {
        for args in [&[][..], &["crate:llmlint", "pypi:llmlint-cli"][..]] {
            let (output, elapsed, _home) = run(args, &[]);
            assert_not_answered(&output, elapsed, "exactly one argument");
        }
    }

    /// Network tier: the real public registries answer a version or no release
    /// yet for both declared targets, never not answered.
    #[test]
    #[ignore = "network: reads crates.io and PyPI; run via `just test-release-targets`"]
    fn the_public_registries_answer_every_declared_target() {
        for id in ["crate:llmlint", "pypi:llmlint-cli"] {
            let (output, elapsed, _home) = run(&[id], &[]);
            assert!(output.status.success(), "{id}: {}", stderr(&output));
            let out = stdout(&output);
            assert!(
                out.is_empty() || (out.ends_with('\n') && out.trim().lines().count() == 1),
                "{id}: {out:?}"
            );
            assert!(elapsed < BOUND);
        }
    }
}
