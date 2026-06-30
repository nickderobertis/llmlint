//! Config discovery (nested: up the tree *and* down into the subtree), parsing
//! (anchors + `<<` merge keys), and recursive `plugins:` resolution — local files
//! and remote/versioned URLs (see [`crate::io::plugins`]).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

use crate::domain::config::{Config, FileFilter, Provenance, ProvenanceBuilder, Rule};
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
/// (file paths and plugin URLs), for provenance, and the per-rule directory
/// scopes (where each rule's file globs are rooted — see [`RuleScope`]).
#[derive(Debug)]
pub struct Loaded {
    pub config: Config,
    pub sources: Vec<String>,
    /// Per-item provenance: which source contributed each rule, agent, and
    /// top-level setting in `config`. Lets `llmlint config` show where an item
    /// is defined, so a rule can be traced to the file that must be edited.
    pub provenance: Provenance,
    /// Maps each rule name to the directory its file globs are rooted at and the
    /// fallback file filter from the config that declared it. With nested
    /// discovery a rule from `a/b/llmlint.yml` is rooted at `a/b`, so its `*.txt`
    /// means `a/b/*.txt`. For explicit `--config` every rule is rooted at `cwd`.
    pub scopes: BTreeMap<String, RuleScope>,
}

/// Where a rule's file globs are rooted, and the fallback file filter to use when
/// the rule (and its agent) declare no `files` of their own.
#[derive(Debug, Clone)]
pub struct RuleScope {
    /// The directory the rule's config lived in; globs resolve relative to it.
    pub dir: PathBuf,
    /// The origin config's effective `files` filter (after its own plugins fold
    /// in), used when neither the rule nor its agent overrides `files`.
    pub files: FileFilter,
}

/// Walk up from `start` to the filesystem root, returning the nearest config.
pub fn discover(start: &Path) -> Option<PathBuf> {
    discover_all(start).into_iter().next()
}

/// Walk up from `start` to the filesystem root, collecting **every** config file
/// found along the way, **nearest first**. Within a single directory the first
/// name in [`CONFIG_NAMES`] priority order wins (at most one config per dir).
///
/// This is what makes configs *nest*: a config beside the files being linted, a
/// project config above it, and a user-level config higher still all layer
/// together, with the most-local one treated as the include root so it wins (see
/// [`load`]) — each parent directory's config fills only what nearer ones leave
/// unset, exactly as a plugin would.
pub fn discover_all(start: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut dir = Some(start);
    while let Some(d) = dir {
        for name in CONFIG_NAMES {
            let p = d.join(name);
            if p.is_file() {
                found.push(p);
                break; // one config per directory (highest-priority name)
            }
        }
        dir = d.parent();
    }
    found
}

/// Walk **down** through `cwd`'s subtree (gitignore-aware), collecting the config
/// file in each directory *strictly below* `cwd` (one per directory, highest
/// `CONFIG_NAMES` priority). This is the cascade half of nested discovery: a
/// config in a subdirectory governs the files under it ("parts of a project"),
/// with its globs rooted at its own directory. `cwd`'s own config is found by
/// [`discover_all`], so it is excluded here. Returns paths sorted for determinism.
pub fn discover_subtree(cwd: &Path) -> Vec<PathBuf> {
    let prio = |p: &Path| {
        p.file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| CONFIG_NAMES.iter().position(|c| *c == n))
    };
    // One chosen config per directory; keep the highest-priority name present.
    let mut by_dir: BTreeMap<PathBuf, PathBuf> = BTreeMap::new();
    for entry in WalkBuilder::new(cwd).hidden(false).build().flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        let Some(p) = prio(path) else { continue };
        let Some(dir) = path.parent() else { continue };
        if dir == cwd {
            continue; // cwd's own config is discover_all's job
        }
        match by_dir.get(dir) {
            Some(existing) if prio(existing).unwrap_or(usize::MAX) <= p => {}
            _ => {
                by_dir.insert(dir.to_path_buf(), path.to_path_buf());
            }
        }
    }
    by_dir.into_values().collect()
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

/// Load and merge config. With explicit `--config` entries this is
/// `load_explicit` (the files merged into one config, all globs rooted at
/// `cwd`, no cascade); otherwise it is nested `load_discovered` — every config
/// found walking up from `cwd` *and* down through its subtree, with each rule
/// scoped to its own config's directory.
///
/// In both paths a config's `plugins` (local files or remote/versioned URLs) are
/// merged recursively and **nearer the root wins**: a config's own settings take
/// precedence over its plugins', a plugin's over its own plugins', and an
/// earlier-listed plugin over a later sibling (see [`Config::merge_plugin`]). Each
/// pulled-in config's own `plugins` are resolved transitively; diamonds and cycles
/// are de-duplicated by absolute path / plugin key, and the transitive depth is
/// bounded by `MAX_PLUGIN_DEPTH`.
pub fn load(entries: &[PathBuf], cwd: &Path) -> Result<Loaded> {
    if entries.is_empty() {
        load_discovered(cwd)
    } else {
        load_explicit(entries, cwd)
    }
}

/// Explicit `--config`: load + merge the given files into one config (no cascade,
/// all globs rooted at `cwd`). The first entry supplies the top-level scalars, the
/// rest contribute rules/agents — exactly the pre-nesting behavior.
fn load_explicit(entries: &[PathBuf], cwd: &Path) -> Result<Loaded> {
    let opts = ResolveOpts::from_env();
    let mut visited: BTreeSet<String> = BTreeSet::new();
    let mut sources: Vec<String> = Vec::new();
    let mut prov = ProvenanceBuilder::default();
    let mut acc: Option<Config> = None;

    for path in entries {
        load_node(
            Node::File(absolutize(path, cwd)),
            0,
            &opts,
            &mut visited,
            &mut sources,
            &mut prov,
            true, // explicit entries are session settings sources
            &mut acc,
        )?;
    }

    let mut config = acc.unwrap_or_default();
    // After every plugin is folded in, layer `override` rules onto the base rule
    // they extend (and surface a duplicate name that didn't opt into `override`).
    // Resolve first so `prov.finish()` sees validated rules (one base per name).
    crate::domain::config::resolve_overrides(&mut config)?;
    // Every rule is rooted at cwd against the merged global filter.
    let scope = RuleScope {
        dir: cwd.to_path_buf(),
        files: config.files.clone(),
    };
    let scopes = config
        .rules
        .iter()
        .map(|r| (r.name.clone(), scope.clone()))
        .collect();
    Ok(Loaded {
        config,
        sources,
        provenance: prov.finish(),
        scopes,
    })
}

/// One discovered config and its directory, plus how far it sits from `cwd` and
/// in which direction. Drives both ordering (nearest-`cwd` wins) and the
/// settings-vs-scope split (descendants scope rules but never retune the session).
struct Unit {
    path: PathBuf,
    dir: PathBuf,
    distance: usize,
    is_descendant: bool,
}

/// Nested discovery: merge **every** config found walking up from `cwd` to the
/// filesystem root *and* down through `cwd`'s subtree. Settings (model, timeout,
/// template, rationales, default `files`) come from `cwd`-and-up only, nearest
/// wins; agents and rules come from all configs. Each rule is **scoped to its own
/// config's directory** — its globs root there (so a subtree config's `*.txt`
/// means `<that dir>/*.txt`) — while resolved paths stay relative to `cwd`. Rule
/// names share one namespace, so `override` extends across the chain and a genuine
/// duplicate name is still an error.
fn load_discovered(cwd: &Path) -> Result<Loaded> {
    let ancestors = discover_all(cwd);
    let descendants = discover_subtree(cwd);
    if ancestors.is_empty() && descendants.is_empty() {
        return Err(Error::ConfigNotFound {
            names: CONFIG_NAMES.join(", "),
            dir: cwd.display().to_string(),
        });
    }

    // Order all units nearest-`cwd` first; on a tie, ancestors (the broader,
    // project-level configs) before descendants, then by path for determinism.
    let mut units: Vec<Unit> = Vec::new();
    for path in &ancestors {
        let dir = path.parent().unwrap_or(cwd).to_path_buf();
        let distance = cwd.strip_prefix(&dir).map_or(0, |r| r.components().count());
        units.push(Unit {
            path: path.clone(),
            dir,
            distance,
            is_descendant: false,
        });
    }
    for path in &descendants {
        let dir = path.parent().unwrap_or(cwd).to_path_buf();
        let distance = dir.strip_prefix(cwd).map_or(0, |r| r.components().count());
        units.push(Unit {
            path: path.clone(),
            dir,
            distance,
            is_descendant: true,
        });
    }
    units.sort_by(|a, b| {
        a.distance
            .cmp(&b.distance)
            .then(a.is_descendant.cmp(&b.is_descendant))
            .then_with(|| a.path.cmp(&b.path))
    });

    let opts = ResolveOpts::from_env();
    // One shared visited set: a plugin pulled in by an earlier (nearer-`cwd`) unit
    // is not re-loaded by a later one, so a shared plugin's rules are attributed to
    // the nearest unit and never duplicated across units.
    let mut visited: BTreeSet<String> = BTreeSet::new();
    let mut sources: Vec<String> = Vec::new();
    // Provenance is recorded across units in the same nearest-`cwd`-first order, so
    // first-writer-wins for settings/agents matches the merge; subtree units record
    // only their items (settings come from `cwd`-and-up only).
    let mut prov = ProvenanceBuilder::default();

    let mut session = Config::default();
    // Rules paired with their origin scope, in nearest-`cwd`-first order.
    let mut scoped: Vec<(Rule, RuleScope)> = Vec::new();

    for unit in &units {
        let mut acc: Option<Config> = None;
        load_node(
            Node::File(unit.path.clone()),
            0,
            &opts,
            &mut visited,
            &mut sources,
            &mut prov,
            !unit.is_descendant,
            &mut acc,
        )?;
        let Some(unit_cfg) = acc else { continue }; // already-visited as a plugin
        let scope = RuleScope {
            dir: unit.dir.clone(),
            files: unit_cfg.files.clone(),
        };
        // Agents from every config; nearest-`cwd` wins on a name clash.
        for (name, agent) in &unit_cfg.agents {
            session
                .agents
                .entry(name.clone())
                .or_insert_with(|| agent.clone());
        }
        // Session-level settings only from `cwd`-and-up, nearest wins.
        if !unit.is_descendant {
            fold_session_settings(&mut session, &unit_cfg);
        }
        for rule in &unit_cfg.rules {
            scoped.push((rule.clone(), scope.clone()));
        }
    }

    // Scope per rule name from its definition (base) site, falling back to the
    // first occurrence — so a rule's globs root where it is defined, matching the
    // `source` its provenance reports. Then resolve overrides on the global rule
    // list (which keeps that first occurrence).
    let mut scopes: BTreeMap<String, RuleScope> = BTreeMap::new();
    for (rule, scope) in scoped.iter().filter(|(r, _)| !r.r#override) {
        scopes
            .entry(rule.name.clone())
            .or_insert_with(|| scope.clone());
    }
    for (rule, scope) in &scoped {
        scopes
            .entry(rule.name.clone())
            .or_insert_with(|| scope.clone());
    }
    session.rules = scoped.into_iter().map(|(rule, _)| rule).collect();
    crate::domain::config::resolve_overrides(&mut session)?;
    scopes.retain(|name, _| session.rules.iter().any(|r| &r.name == name));

    Ok(Loaded {
        config: session,
        sources,
        provenance: prov.finish(),
        scopes,
    })
}

/// Fold a `cwd`-or-ancestor config's session-level settings under `session`
/// (first-writer-wins, so the nearer-`cwd` config already in `session` keeps its
/// values). Agents and rules are handled by the caller; this is scalars + the
/// default file filter only.
fn fold_session_settings(session: &mut Config, unit: &Config) {
    session.version = session.version.take().or_else(|| unit.version.clone());
    session.prompt_template = session
        .prompt_template
        .take()
        .or_else(|| unit.prompt_template.clone());
    if session.files.is_empty() {
        session.files = unit.files.clone();
    }
    session.oneharness.merge_under(unit.oneharness.clone());
    session.rationales = session.rationales.or(unit.rationales);
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
    // Whether this node's top-level settings count as a setting's source. True for
    // entry files and `cwd`-and-up discovered configs; false for a cascaded subtree
    // config, whose settings never retune the session (only its items count).
    record_settings: bool,
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
    // entries match the value that survives the merge. A cascaded subtree config
    // records only its items, never its settings.
    if record_settings {
        prov.record(&cfg, &origin);
    } else {
        prov.record_items(&cfg, &origin);
    }

    match acc {
        None => *acc = Some(cfg),
        Some(a) => a.merge_plugin(cfg),
    }

    for spec in child_specs {
        let child = Node::resolve(&spec, base_dir.as_deref())?;
        // A subtree config's plugins are part of that subtree, so they inherit its
        // settings-exclusion too.
        load_node(
            child,
            depth + 1,
            opts,
            visited,
            sources,
            prov,
            record_settings,
            acc,
        )?;
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
    fn discover_all_collects_every_ancestor_nearest_first() {
        let dir = tempdir().unwrap();
        let mid = dir.path().join("a");
        let leaf = mid.join("b");
        fs::create_dir_all(&leaf).unwrap();
        fs::write(dir.path().join("llmlint.yml"), "version: 1\n").unwrap();
        fs::write(mid.join("llmlint.yml"), "version: 1\n").unwrap();
        fs::write(leaf.join("llmlint.yml"), "version: 1\n").unwrap();
        // Nearest first: leaf, then mid, then the root — one config per directory.
        let found = discover_all(&leaf);
        assert_eq!(
            found,
            vec![
                leaf.join("llmlint.yml"),
                mid.join("llmlint.yml"),
                dir.path().join("llmlint.yml"),
            ]
        );
    }

    #[test]
    fn nested_configs_merge_with_most_local_winning() {
        // A user/project/local layout discovered by walking up: a user-level
        // config at the root, a project config a level down, a local config in the
        // leaf where linting starts. The most-local config wins each scalar; every
        // config contributes its rules; a deeper config fills only the gaps.
        let dir = tempdir().unwrap();
        let proj = dir.path().join("proj");
        let leaf = proj.join("src");
        fs::create_dir_all(&leaf).unwrap();
        // User level (root): sets timeout + a rule; rationales true.
        fs::write(
            dir.path().join("llmlint.yml"),
            "version: 1\nrationales: true\noneharness:\n  model: user-model\n  timeout: 9\n\
             rules:\n  - name: user_rule\n    description: \"true when ok; false otherwise.\"\n",
        )
        .unwrap();
        // Project level: overrides the model, leaves timeout unset, adds a rule.
        fs::write(
            proj.join("llmlint.yml"),
            "oneharness:\n  model: proj-model\nrules:\n  - name: proj_rule\n    \
             description: \"true when ok; false otherwise.\"\n",
        )
        .unwrap();
        // Local level (where linting runs): flips rationales, adds a rule.
        fs::write(
            leaf.join("llmlint.yml"),
            "rationales: false\nrules:\n  - name: local_rule\n    \
             description: \"true when ok; false otherwise.\"\n",
        )
        .unwrap();

        let cfg = load(&[], &leaf).unwrap().config;
        // Local set rationales -> local wins over both ancestors.
        assert_eq!(cfg.rationales, Some(false));
        // Local left model unset; project is nearer than user -> project wins.
        assert_eq!(cfg.oneharness.model.as_deref(), Some("proj-model"));
        // Only the user level set timeout -> it fills through.
        assert_eq!(cfg.oneharness.timeout, Some(9));
        // Every config contributes its rule, most-local first.
        let names: Vec<&str> = cfg.rules.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["local_rule", "proj_rule", "user_rule"]);
    }

    #[test]
    fn discover_subtree_finds_descendant_configs_one_per_dir() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("a/b")).unwrap();
        fs::write(dir.path().join("llmlint.yml"), "rules: []\n").unwrap(); // cwd: excluded
        fs::write(dir.path().join("a/llmlint.yml"), "rules: []\n").unwrap();
        // Two names in one dir -> highest priority (`llmlint.yaml` before dotfiles).
        fs::write(dir.path().join("a/b/llmlint.yaml"), "rules: []\n").unwrap();
        fs::write(dir.path().join("a/b/.llmlint.yml"), "rules: []\n").unwrap();
        let found: BTreeSet<PathBuf> = discover_subtree(dir.path()).into_iter().collect();
        let expected: BTreeSet<PathBuf> = [
            dir.path().join("a/llmlint.yml"),
            dir.path().join("a/b/llmlint.yaml"), // highest priority in a/b
        ]
        .into_iter()
        .collect();
        assert_eq!(found, expected);
    }

    #[test]
    fn cascade_scopes_each_rule_to_its_config_directory() {
        // A cwd config and a subtree config; each rule is rooted at its own
        // config's directory, and both rules are contributed.
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("sub")).unwrap();
        fs::write(
            dir.path().join("llmlint.yml"),
            "version: 1\nfiles:\n  include: [\"**/*.rs\"]\nrules:\n  - name: root_rule\n    \
             description: \"true when ok; false otherwise.\"\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("sub/llmlint.yml"),
            "files:\n  include: [\"*.txt\"]\nrules:\n  - name: sub_rule\n    \
             description: \"true when ok; false otherwise.\"\n",
        )
        .unwrap();

        let loaded = load(&[], dir.path()).unwrap();
        let names: Vec<&str> = loaded
            .config
            .rules
            .iter()
            .map(|r| r.name.as_str())
            .collect();
        assert_eq!(names, ["root_rule", "sub_rule"]);
        // root_rule is rooted at cwd; sub_rule at the subtree directory.
        assert_eq!(loaded.scopes["root_rule"].dir, dir.path());
        assert_eq!(loaded.scopes["sub_rule"].dir, dir.path().join("sub"));
        // The subtree rule's fallback filter is its own config's `files`.
        assert_eq!(loaded.scopes["sub_rule"].files.include, vec!["*.txt"]);
    }

    #[test]
    fn descendant_configs_scope_rules_but_do_not_retune_session_settings() {
        // A descendant sets a different model + its own agent; the session model
        // stays the cwd config's, while the descendant's agent and rule are still
        // contributed (and the rule is scoped to the subtree).
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("sub")).unwrap();
        fs::write(
            dir.path().join("llmlint.yml"),
            "version: 1\noneharness:\n  model: root-model\nrules:\n  - name: root_rule\n    \
             description: \"true when ok; false otherwise.\"\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("sub/llmlint.yml"),
            "oneharness:\n  model: sub-model\nagents:\n  scoped:\n    model: x\n\
             rules:\n  - name: sub_rule\n    agent: scoped\n    \
             description: \"true when ok; false otherwise.\"\n",
        )
        .unwrap();

        let loaded = load(&[], dir.path()).unwrap();
        // Session model comes from cwd-and-up only -> the descendant is ignored.
        assert_eq!(
            loaded.config.oneharness.model.as_deref(),
            Some("root-model")
        );
        // The descendant's agent is still available (rules reference it)...
        assert!(loaded.config.agents.contains_key("scoped"));
        // ...and its rule is contributed, scoped to the subtree.
        assert_eq!(loaded.scopes["sub_rule"].dir, dir.path().join("sub"));
    }

    #[test]
    fn discovery_succeeds_with_only_a_descendant_config() {
        // No config at cwd or above, but a subtree config exists: the run is still
        // configured (the configured subtree), not a ConfigNotFound.
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("sub")).unwrap();
        fs::write(
            dir.path().join("sub/llmlint.yml"),
            "rules:\n  - name: only_sub\n    description: \"true when ok; false otherwise.\"\n",
        )
        .unwrap();
        let loaded = load(&[], dir.path()).unwrap();
        let names: Vec<&str> = loaded
            .config
            .rules
            .iter()
            .map(|r| r.name.as_str())
            .collect();
        assert_eq!(names, ["only_sub"]);
        assert_eq!(loaded.scopes["only_sub"].dir, dir.path().join("sub"));
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
