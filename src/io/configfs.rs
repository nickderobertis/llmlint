//! Config discovery, parsing (anchors + `<<` merge keys), and recursive
//! `plugins:` resolution — local files and remote/versioned URLs (see
//! [`crate::io::plugins`]).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::domain::config::{Config, Provenance, ProvenanceBuilder};
use crate::domain::version::VersionReq;
use crate::errors::{io_err, Error, Result};
use crate::io::plugins::{self, ResolveOpts};

/// Maximum depth of transitive `plugins:` resolution. A config's `plugins`
/// pull in further configs, whose own `plugins` are pulled in turn, and so on.
/// Cycles are already made safe by the visited-set dedup (a key is loaded once),
/// so this is a defense-in-depth bound that stops a pathologically deep *acyclic*
/// include graph from exhausting the stack — it surfaces a clear error instead.
const MAX_PLUGIN_DEPTH: usize = 100;

/// Config file names searched for, in priority order, when walking up the tree.
pub const CONFIG_NAMES: &[&str] = &[
    "llmlint.yml",
    "llmlint.yaml",
    ".llmlint.yml",
    ".llmlint.yaml",
];

/// The merged config plus the ordered list of sources that contributed to it
/// (file paths and plugin URLs), for provenance.
#[derive(Debug)]
pub struct Loaded {
    pub config: Config,
    pub sources: Vec<String>,
    /// Per-item provenance: which source contributed each rule, agent, and
    /// top-level setting in `config`. Lets `llmlint config` show where an item
    /// is defined, so a rule can be traced to the file that must be edited.
    pub provenance: Provenance,
}

/// Walk up from `start` to the filesystem root, returning the nearest config.
pub fn discover(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        for name in CONFIG_NAMES {
            let p = d.join(name);
            if p.is_file() {
                return Some(p);
            }
        }
        dir = d.parent();
    }
    None
}

/// Parse one YAML document into a [`Config`], resolving anchors/aliases (done
/// by the parser) and `<<` merge keys (via `apply_merge`).
pub fn parse(text: &str, origin: &str) -> Result<Config> {
    let err = |e: serde_yaml_ng::Error| Error::ConfigParse {
        path: origin.to_string(),
        message: e.to_string(),
    };
    let mut value: serde_yaml_ng::Value = serde_yaml_ng::from_str(text).map_err(err)?;
    value.apply_merge().map_err(err)?;
    // The top-level config-include key was renamed `include` -> `plugins` (to
    // avoid confusion with `files.include`). Unknown top-level keys are allowed
    // (anchors live in throwaway keys), so a stale `include:` would silently do
    // nothing; catch it with a clear migration error instead.
    if let serde_yaml_ng::Value::Mapping(m) = &value {
        if m.contains_key(serde_yaml_ng::Value::from("include")) {
            return Err(Error::ConfigParse {
                path: origin.to_string(),
                message: "top-level `include` was renamed to `plugins` (it pulls in other \
                          configs; `files.include` is the file glob). Rename the key to `plugins`."
                    .to_string(),
            });
        }
    }
    serde_yaml_ng::from_value(value).map_err(err)
}

/// Load and merge config from explicit entry files (from `--config`), or, when
/// `entries` is empty, the nearest discovered config above `cwd`. `plugins`
/// (local files or remote/versioned URLs) are merged recursively and **nearer
/// the root wins**: a config's own top-level settings take precedence over its
/// plugins', a plugin's over its own plugins', and an earlier-listed plugin over
/// a later sibling; a plugin only fills settings the including config left unset
/// (see [`Config::merge_plugin`]). Rules and agents from every config are
/// contributed. Each pulled-in config's own `plugins` are resolved transitively.
/// Diamonds and
/// cycles are de-duplicated by absolute path / plugin key, and the transitive
/// depth is bounded by `MAX_PLUGIN_DEPTH`.
pub fn load(entries: &[PathBuf], cwd: &Path) -> Result<Loaded> {
    let entry_paths: Vec<PathBuf> = if entries.is_empty() {
        match discover(cwd) {
            Some(p) => vec![p],
            None => {
                return Err(Error::ConfigNotFound {
                    names: CONFIG_NAMES.join(", "),
                    dir: cwd.display().to_string(),
                })
            }
        }
    } else {
        entries.iter().map(|p| absolutize(p, cwd)).collect()
    };

    let opts = ResolveOpts::from_env();
    let mut visited: BTreeSet<String> = BTreeSet::new();
    let mut sources: Vec<String> = Vec::new();
    let mut prov = ProvenanceBuilder::default();
    let mut acc: Option<Config> = None;

    for path in &entry_paths {
        load_node(
            Node::File(path.clone()),
            0,
            &opts,
            &mut visited,
            &mut sources,
            &mut prov,
            &mut acc,
        )?;
    }

    let mut config = acc.unwrap_or_default();
    // After every plugin is folded in, layer `override` rules onto the base rule
    // they extend (and surface a duplicate name that didn't opt into `override`).
    // Resolve first so `prov.finish()` sees validated rules (one base per name).
    crate::domain::config::resolve_overrides(&mut config)?;

    Ok(Loaded {
        config,
        sources,
        provenance: prov.finish(),
    })
}

enum Node {
    File(PathBuf),
    /// A URL plugin: the bare URL, an optional version pin, and a stable dedup
    /// key (`url` or `url@pin`). The text is fetched in [`Node::read`] — after
    /// the visited check — so a diamond never refetches.
    Remote {
        url: String,
        req: Option<VersionReq>,
        key: String,
    },
}

impl Node {
    /// Parse a `plugins:` spec into a node. Pure: no I/O happens here (so a
    /// duplicate is skipped before any fetch).
    fn resolve(spec: &str, base_dir: Option<&Path>) -> Result<Node> {
        match plugins::parse_spec(spec)? {
            plugins::PluginRef::Local(p) => {
                let abs = if p.is_absolute() {
                    p
                } else {
                    match base_dir {
                        Some(d) => d.join(p),
                        None => {
                            return Err(Error::InvalidConfig(format!(
                                "cannot resolve relative plugin {spec:?} from a remote plugin"
                            )))
                        }
                    }
                };
                Ok(Node::File(abs))
            }
            plugins::PluginRef::Remote { url, req } => {
                let key = match &req {
                    Some(r) => format!("{url}@{r}"),
                    None => url.clone(),
                };
                Ok(Node::Remote { url, req, key })
            }
        }
    }

    fn key(&self) -> String {
        match self {
            Node::File(p) => normalize(p).display().to_string(),
            Node::Remote { key, .. } => key.clone(),
        }
    }

    fn origin(&self) -> String {
        self.key()
    }

    /// Returns `(text, base_dir_for_relative_plugins)`.
    fn read(&self, opts: &ResolveOpts) -> Result<(String, Option<PathBuf>)> {
        match self {
            Node::File(p) => {
                let text = std::fs::read_to_string(p)
                    .map_err(|e| io_err(format!("reading config {}", p.display()), e))?;
                Ok((text, p.parent().map(Path::to_path_buf)))
            }
            // Remote plugins can pull in further URL plugins, but not relative
            // file paths (there is no local base directory).
            Node::Remote { url, req, .. } => Ok((plugins::load_remote(url, req, opts)?, None)),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn load_node(
    node: Node,
    depth: usize,
    opts: &ResolveOpts,
    visited: &mut BTreeSet<String>,
    sources: &mut Vec<String>,
    prov: &mut ProvenanceBuilder,
    acc: &mut Option<Config>,
) -> Result<()> {
    // Entry files are depth 0; their `plugins` are depth 1, and so on. Bound the
    // transitive chain before doing any I/O so a pathological graph fails fast.
    if depth > MAX_PLUGIN_DEPTH {
        return Err(Error::PluginDepthExceeded {
            max: MAX_PLUGIN_DEPTH,
        });
    }

    let key = node.key();
    if !visited.insert(key.clone()) {
        return Ok(()); // already loaded (diamond/cycle) — skip
    }
    sources.push(key);

    let (text, base_dir) = node.read(opts)?;
    let origin = node.origin();
    let cfg = parse(&text, &origin)?;
    let child_specs = cfg.plugins.clone();

    // Record provenance in merge order (before folding in), so first-writer-wins
    // entries match the value that survives the merge.
    prov.record(&cfg, &origin);

    match acc {
        None => *acc = Some(cfg),
        Some(a) => a.merge_plugin(cfg),
    }

    for spec in child_specs {
        let child = Node::resolve(&spec, base_dir.as_deref())?;
        load_node(child, depth + 1, opts, visited, sources, prov, acc)?;
    }
    Ok(())
}

fn absolutize(p: &Path, cwd: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    }
}

/// Lexically normalize a path (collapse `.`/`..`) without touching the
/// filesystem, so the dedup key is stable even for not-yet-read files.
fn normalize(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn parse_resolves_anchors_and_merge_keys() {
        let yaml = r#"
x-prompts:
  shared: &shared "be terse"
agents:
  a:
    prompt_template: *shared
  b:
    <<: &defaults { harness: claude-code }
    model: opus
rules:
  - name: only_rule
    description: "true when ok; false otherwise."
"#;
        let cfg = parse(yaml, "test").unwrap();
        assert_eq!(cfg.agents["a"].prompt_template.as_deref(), Some("be terse"));
        assert_eq!(cfg.agents["b"].harness.as_deref(), Some("claude-code"));
        assert_eq!(cfg.agents["b"].model.as_deref(), Some("opus"));
    }

    #[test]
    fn parse_rejects_unknown_nested_field() {
        let yaml = "rules:\n  - name: r\n    description: d\n    bogus: 1\n";
        assert!(matches!(parse(yaml, "t"), Err(Error::ConfigParse { .. })));
    }

    #[test]
    fn discover_walks_up() {
        let dir = tempdir().unwrap();
        let nested = dir.path().join("a/b/c");
        fs::create_dir_all(&nested).unwrap();
        fs::write(dir.path().join("llmlint.yml"), "version: 1\n").unwrap();
        let found = discover(&nested).unwrap();
        assert_eq!(found, dir.path().join("llmlint.yml"));
    }

    #[test]
    fn load_missing_config_errors() {
        let dir = tempdir().unwrap();
        let err = load(&[], dir.path()).unwrap_err();
        assert!(matches!(err, Error::ConfigNotFound { .. }));
    }

    #[test]
    fn load_merges_file_and_bundled_plugins() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("team.yml"),
            "rules:\n  - name: team_rule\n    description: \"true when ok; false otherwise.\"\n",
        )
        .unwrap();
        let plugin = format!("{}@1", crate::io::assets::CONFIG_LINT_URL);
        let root = dir.path().join("llmlint.yml");
        fs::write(
            &root,
            format!(
                "version: 1\nplugins:\n  - ./team.yml\n  - {plugin}\nrules:\n  \
                 - name: root_rule\n    description: \"true when ok; false otherwise.\"\n"
            ),
        )
        .unwrap();
        let loaded = load(&[root], dir.path()).unwrap();
        let names: Vec<&str> = loaded
            .config
            .rules
            .iter()
            .map(|r| r.name.as_str())
            .collect();
        assert!(names.contains(&"root_rule"));
        assert!(names.contains(&"team_rule"));
        assert!(names.contains(&"name_matches_description")); // from the bundled plugin
        assert!(loaded.sources.iter().any(|s| s == &plugin));
    }

    #[test]
    fn removed_llmlint_scheme_errors() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("llmlint.yml");
        fs::write(&root, "plugins:\n  - llmlint:config-lint\n").unwrap();
        assert!(matches!(
            load(&[root], dir.path()),
            Err(Error::PluginSpec(_))
        ));
    }

    #[test]
    fn renamed_include_key_is_a_clear_error() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("llmlint.yml");
        fs::write(&root, "include:\n  - ./team.yml\n").unwrap();
        let err = load(&[root], dir.path()).unwrap_err();
        assert!(err.to_string().contains("renamed to `plugins`"));
    }

    #[test]
    fn plugins_resolve_transitively() {
        // root -> mid -> leaf: each config's own `plugins` are pulled in, so a
        // rule three levels deep lands in the merged config.
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("leaf.yml"),
            "rules:\n  - name: leaf_rule\n    description: \"true when ok; false otherwise.\"\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("mid.yml"),
            "plugins:\n  - ./leaf.yml\nrules:\n  - name: mid_rule\n    \
             description: \"true when ok; false otherwise.\"\n",
        )
        .unwrap();
        let root = dir.path().join("llmlint.yml");
        fs::write(
            &root,
            "version: 1\nplugins:\n  - ./mid.yml\nrules:\n  - name: root_rule\n    \
             description: \"true when ok; false otherwise.\"\n",
        )
        .unwrap();

        let loaded = load(&[root], dir.path()).unwrap();
        let names: Vec<&str> = loaded
            .config
            .rules
            .iter()
            .map(|r| r.name.as_str())
            .collect();
        assert_eq!(names, ["root_rule", "mid_rule", "leaf_rule"]);
    }

    #[test]
    fn top_level_scalars_resolve_nearest_root_wins() {
        // root -> mid -> leaf. Each sets a different scalar; the nearest config to
        // set one wins, and a deeper plugin only fills what shallower ones left
        // unset.
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("leaf.yml"),
            "rationales: true\noneharness:\n  model: leaf-model\n  timeout: 7\n\
             prompt_template: leaf-tmpl\nrules: []\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("mid.yml"),
            "plugins:\n  - ./leaf.yml\noneharness:\n  model: mid-model\nrules: []\n",
        )
        .unwrap();
        let root = dir.path().join("llmlint.yml");
        fs::write(
            &root,
            "version: 1\nplugins:\n  - ./mid.yml\nrationales: false\nrules: []\n",
        )
        .unwrap();

        let cfg = load(&[root], dir.path()).unwrap().config;
        // root set rationales -> root wins over both plugins.
        assert_eq!(cfg.rationales, Some(false));
        // root left model unset; mid is nearer than leaf -> mid wins.
        assert_eq!(cfg.oneharness.model.as_deref(), Some("mid-model"));
        // only leaf set these -> they fill through.
        assert_eq!(cfg.oneharness.timeout, Some(7));
        assert_eq!(cfg.prompt_template.as_deref(), Some("leaf-tmpl"));
    }

    #[test]
    fn provenance_traces_each_item_to_its_source() {
        // root -> mid -> leaf. Each contributes rules/agents/settings; provenance
        // must name the file each item came from, with first-writer-wins for
        // settings/agents and every source for a rule (base + override).
        let desc = "    description: \"true when ok; false otherwise.\"\n";
        let dir = tempdir().unwrap();
        let leaf = dir.path().join("leaf.yml");
        fs::write(
            &leaf,
            format!(
                "rationales: true\nagents:\n  shared:\n    harness: codex\nrules:\n  \
                 - name: leaf_rule\n{desc}  - name: shared_rule\n{desc}"
            ),
        )
        .unwrap();
        let mid = dir.path().join("mid.yml");
        fs::write(
            &mid,
            format!(
                "plugins:\n  - ./leaf.yml\noneharness:\n  model: mid-model\nagents:\n  \
                 shared:\n    harness: claude-code\nrules:\n  - name: mid_rule\n{desc}"
            ),
        )
        .unwrap();
        let root = dir.path().join("llmlint.yml");
        fs::write(
            &root,
            format!(
                "version: 1\nplugins:\n  - ./mid.yml\nrationales: false\nrules:\n  \
                 - name: root_rule\n{desc}  - name: shared_rule\n    override: true\n    judges: 3\n"
            ),
        )
        .unwrap();

        let prov = load(std::slice::from_ref(&root), dir.path())
            .unwrap()
            .provenance;
        let key = |p: &Path| normalize(p).display().to_string();

        // Settings: root set version + rationales; only mid set the model; only
        // the leaf set rationales originally but root wins (first writer).
        assert_eq!(prov.settings["version"], key(&root));
        assert_eq!(prov.settings["rationales"], key(&root));
        assert_eq!(prov.settings["oneharness.model"], key(&mid));

        // Agent declared by both mid and leaf -> nearest-root (mid) wins.
        assert_eq!(prov.agents["shared"], key(&mid));

        // Each rule names its definition site; a rule with no override has no
        // per-field entries.
        assert_eq!(prov.rules["root_rule"].source, key(&root));
        assert!(prov.rules["root_rule"].fields.is_empty());
        assert_eq!(prov.rules["mid_rule"].source, key(&mid));
        assert_eq!(prov.rules["leaf_rule"].source, key(&leaf));

        // `shared_rule` is defined in the leaf, but the root override changed
        // `judges` -> provenance points `judges` at the root file while the
        // definition (and the inherited description) stays the leaf.
        let shared = &prov.rules["shared_rule"];
        assert_eq!(shared.source, key(&leaf));
        assert_eq!(shared.fields["judges"], key(&root));
        assert!(!shared.fields.contains_key("description"));
    }

    #[test]
    fn plugin_chain_deeper_than_max_depth_errors() {
        // A straight, acyclic chain longer than MAX_PLUGIN_DEPTH can't be
        // dedup'd away (every file is distinct), so it trips the depth bound.
        let dir = tempdir().unwrap();
        let total = MAX_PLUGIN_DEPTH + 2;
        for i in 0..total {
            let path = dir.path().join(format!("c{i}.yml"));
            let body = if i + 1 < total {
                format!("plugins:\n  - ./c{}.yml\nrules: []\n", i + 1)
            } else {
                "rules: []\n".to_string()
            };
            fs::write(&path, body).unwrap();
        }
        let err = load(&[dir.path().join("c0.yml")], dir.path()).unwrap_err();
        assert!(matches!(err, Error::PluginDepthExceeded { .. }));
    }

    #[test]
    fn plugin_cycle_is_safe() {
        let dir = tempdir().unwrap();
        let a = dir.path().join("a.yml");
        let b = dir.path().join("b.yml");
        fs::write(&a, "plugins:\n  - ./b.yml\nrules: []\n").unwrap();
        fs::write(&b, "plugins:\n  - ./a.yml\nrules: []\n").unwrap();
        // Must terminate rather than recurse forever.
        let loaded = load(&[a], dir.path()).unwrap();
        assert_eq!(loaded.sources.len(), 2);
    }
}
