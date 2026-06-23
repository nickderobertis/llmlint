//! Config discovery, parsing (anchors + `<<` merge keys), and recursive
//! `include` resolution — which doubles as the plugin system.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::domain::config::Config;
use crate::errors::{io_err, Error, Result};
use crate::io::assets;

/// Config file names searched for, in priority order, when walking up the tree.
pub const CONFIG_NAMES: &[&str] = &[
    "llmlint.yml",
    "llmlint.yaml",
    ".llmlint.yml",
    ".llmlint.yaml",
];

/// The merged config plus the ordered list of sources that contributed to it
/// (file paths and bundled plugin ids), for provenance.
#[derive(Debug)]
pub struct Loaded {
    pub config: Config,
    pub sources: Vec<String>,
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
    serde_yaml_ng::from_value(value).map_err(err)
}

/// Load and merge config from explicit entry files (from `--config`), or, when
/// `entries` is empty, the nearest discovered config above `cwd`. `include`d
/// configs (file paths or bundled `llmlint:` ids) are merged recursively; the
/// first entry provides the top-level scalars, the rest contribute rules and
/// agents. Diamonds and cycles are de-duplicated by absolute path / id.
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

    let mut visited: BTreeSet<String> = BTreeSet::new();
    let mut sources: Vec<String> = Vec::new();
    let mut acc: Option<Config> = None;

    for path in &entry_paths {
        load_node(
            Node::File(path.clone()),
            &mut visited,
            &mut sources,
            &mut acc,
        )?;
    }

    Ok(Loaded {
        config: acc.unwrap_or_default(),
        sources,
    })
}

enum Node {
    File(PathBuf),
    Bundled(String, &'static str),
}

impl Node {
    fn resolve(spec: &str, base_dir: Option<&Path>) -> Result<Node> {
        if spec.starts_with("llmlint:") {
            let content = assets::bundled(spec).ok_or_else(|| Error::UnknownPlugin(spec.into()))?;
            return Ok(Node::Bundled(spec.to_string(), content));
        }
        let p = PathBuf::from(spec);
        let abs = if p.is_absolute() {
            p
        } else {
            match base_dir {
                Some(d) => d.join(p),
                None => {
                    return Err(Error::InvalidConfig(format!(
                        "cannot resolve relative include {spec:?} from a bundled plugin"
                    )))
                }
            }
        };
        Ok(Node::File(abs))
    }

    fn key(&self) -> String {
        match self {
            Node::File(p) => normalize(p).display().to_string(),
            Node::Bundled(id, _) => id.clone(),
        }
    }

    fn origin(&self) -> String {
        self.key()
    }

    /// Returns `(text, base_dir_for_relative_includes)`.
    fn read(&self) -> Result<(String, Option<PathBuf>)> {
        match self {
            Node::File(p) => {
                let text = std::fs::read_to_string(p)
                    .map_err(|e| io_err(format!("reading config {}", p.display()), e))?;
                Ok((text, p.parent().map(Path::to_path_buf)))
            }
            Node::Bundled(_, content) => Ok((content.to_string(), None)),
        }
    }
}

fn load_node(
    node: Node,
    visited: &mut BTreeSet<String>,
    sources: &mut Vec<String>,
    acc: &mut Option<Config>,
) -> Result<()> {
    let key = node.key();
    if !visited.insert(key.clone()) {
        return Ok(()); // already loaded (diamond/cycle) — skip
    }
    sources.push(key);

    let (text, base_dir) = node.read()?;
    let cfg = parse(&text, &node.origin())?;
    let includes = cfg.include.clone();

    match acc {
        None => *acc = Some(cfg),
        Some(a) => a.merge_rules_and_agents(cfg),
    }

    for inc in includes {
        let child = Node::resolve(&inc, base_dir.as_deref())?;
        load_node(child, visited, sources, acc)?;
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
    description: "TRUE when ok; FALSE otherwise."
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
    fn load_merges_includes_and_bundled_plugin() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("team.yml"),
            "rules:\n  - name: team_rule\n    description: \"TRUE when ok; FALSE otherwise.\"\n",
        )
        .unwrap();
        let root = dir.path().join("llmlint.yml");
        fs::write(
            &root,
            "version: 1\ninclude:\n  - ./team.yml\n  - llmlint:config-lint\nrules:\n  \
             - name: root_rule\n    description: \"TRUE when ok; FALSE otherwise.\"\n",
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
        assert!(names.contains(&"name_matches_description")); // from the plugin
        assert!(loaded.sources.iter().any(|s| s == "llmlint:config-lint"));
    }

    #[test]
    fn unknown_plugin_errors() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("llmlint.yml");
        fs::write(&root, "include:\n  - llmlint:does-not-exist\n").unwrap();
        assert!(matches!(
            load(&[root], dir.path()),
            Err(Error::UnknownPlugin(_))
        ));
    }

    #[test]
    fn include_cycle_is_safe() {
        let dir = tempdir().unwrap();
        let a = dir.path().join("a.yml");
        let b = dir.path().join("b.yml");
        fs::write(&a, "include:\n  - ./b.yml\nrules: []\n").unwrap();
        fs::write(&b, "include:\n  - ./a.yml\nrules: []\n").unwrap();
        // Must terminate rather than recurse forever.
        let loaded = load(&[a], dir.path()).unwrap();
        assert_eq!(loaded.sources.len(), 2);
    }
}
