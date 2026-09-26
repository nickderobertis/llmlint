use std::fs::read_to_string as load_source;
use std::path::Path;

/// A package manifest: name, version, and dependency names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub dependencies: Vec<String>,
}

/// Parse `key = value` manifest text. Unknown keys are ignored;
/// `dependencies` is a comma-separated list.
pub fn parse_manifest(text: &str) -> Manifest {
    let mut manifest = Manifest { name: String::new(), version: String::new(), dependencies: Vec::new() };
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else { continue };
        match key.trim() {
            "name" => manifest.name = value.trim().to_string(),
            "version" => manifest.version = value.trim().to_string(),
            "dependencies" => {
                manifest.dependencies = value
                    .split(',')
                    .map(str::trim)
                    .filter(|dependency| !dependency.is_empty())
                    .map(str::to_string)
                    .collect();
            }
            _ => {}
        }
    }
    manifest
}

/// The manifest for the package rooted at `package_dir`, or an empty manifest
/// when its `package.conf` cannot be loaded.
pub fn manifest_for(package_dir: &Path) -> Manifest {
    let text = load_source(package_dir.join("package.conf")).unwrap_or_default();
    parse_manifest(&text)
}
