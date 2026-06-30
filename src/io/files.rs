//! Resolve a [`FileFilter`] to a concrete list of target files, and normalize
//! the explicit CLI file list. Uses `ignore` (gitignore-aware walk) + `globset`.

use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;

use crate::domain::config::FileFilter;
use crate::errors::{Error, Result};

fn build_set(globs: &[String]) -> Result<Option<GlobSet>> {
    if globs.is_empty() {
        return Ok(None);
    }
    let mut b = GlobSetBuilder::new();
    for g in globs {
        let glob =
            Glob::new(g).map_err(|e| Error::InvalidConfig(format!("invalid glob {g:?}: {e}")))?;
        b.add(glob);
    }
    let set = b
        .build()
        .map_err(|e| Error::InvalidConfig(format!("invalid glob set: {e}")))?;
    Ok(Some(set))
}

/// Resolve `filter` to a sorted, de-duplicated list of files **relative to
/// `root`**. An empty `include` set yields no files (nothing to lint).
pub fn resolve(root: &Path, filter: &FileFilter) -> Result<Vec<PathBuf>> {
    let include = match build_set(&filter.include)? {
        Some(s) => s,
        None => return Ok(Vec::new()),
    };
    let exclude = build_set(&filter.exclude)?;

    let mut out = Vec::new();
    // `hidden(false)` so dotfiles like `.llmlint.yml` are visited; `.gitignore`
    // is still respected so we never lint ignored/build files.
    for entry in WalkBuilder::new(root).hidden(false).build() {
        let entry = entry.map_err(|e| Error::Io(format!("walking {}: {e}", root.display())))?;
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        let rel = path.strip_prefix(root).unwrap_or(path);
        let excluded = exclude.as_ref().is_some_and(|e| e.is_match(rel));
        if include.is_match(rel) && !excluded {
            out.push(rel.to_path_buf());
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// Read a target file (given relative to `root`) as UTF-8 text, for scanning
/// inline `llmlint: ignore` directives. Returns `Ok(None)` for a non-UTF-8
/// (binary) file — it can't carry a text directive, so it is skipped rather than
/// failing the run. A genuine read error (e.g. permissions) is propagated.
pub fn read_text(root: &Path, rel: &Path) -> Result<Option<String>> {
    let path = root.join(rel);
    let bytes =
        std::fs::read(&path).map_err(|e| Error::Io(format!("reading {}: {e}", path.display())))?;
    Ok(String::from_utf8(bytes).ok())
}

/// Render a (relative) path with forward slashes, so the prompt the judge sees —
/// and the violation paths it echoes back — are consistent across platforms
/// (a Windows `PathBuf` would otherwise render `\`). Pure string formatting; it
/// lives here next to the other path helpers so `commands` shares one spelling.
pub fn to_slash(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Normalize explicit CLI file paths to be relative to `root` where possible
/// (tidier prompts and output); paths outside `root` are kept as given.
pub fn from_cli(root: &Path, files: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = files
        .iter()
        .map(|f| {
            f.strip_prefix(root)
                .map(Path::to_path_buf)
                .unwrap_or_else(|_| f.clone())
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn touch(root: &Path, rel: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, "x").unwrap();
    }

    #[test]
    fn include_exclude_globs() {
        let dir = tempdir().unwrap();
        touch(dir.path(), "src/a.rs");
        touch(dir.path(), "src/b.rs");
        touch(dir.path(), "src/gen.rs");
        touch(dir.path(), "README.md");
        let filter = FileFilter {
            include: vec!["src/**/*.rs".into()],
            exclude: vec!["**/gen.rs".into()],
        };
        let files = resolve(dir.path(), &filter).unwrap();
        let names: Vec<String> = files.iter().map(|p| p.display().to_string()).collect();
        assert!(names.iter().any(|n| n.ends_with("a.rs")));
        assert!(names.iter().any(|n| n.ends_with("b.rs")));
        assert!(!names.iter().any(|n| n.ends_with("gen.rs")));
        assert!(!names.iter().any(|n| n.ends_with("README.md")));
    }

    #[test]
    fn double_star_matches_root_level_file() {
        let dir = tempdir().unwrap();
        touch(dir.path(), "llmlint.yml");
        touch(dir.path(), "nested/llmlint.yml");
        let filter = FileFilter {
            include: vec!["**/llmlint.yml".into()],
            exclude: vec![],
        };
        let files = resolve(dir.path(), &filter).unwrap();
        // `**/llmlint.yml` must match both the root and nested config file.
        assert_eq!(files.len(), 2, "got {files:?}");
    }

    #[test]
    fn empty_include_yields_nothing() {
        let dir = tempdir().unwrap();
        touch(dir.path(), "src/a.rs");
        let files = resolve(dir.path(), &FileFilter::default()).unwrap();
        assert!(files.is_empty());
    }

    #[test]
    fn invalid_glob_is_reported() {
        let filter = FileFilter {
            include: vec!["[".into()],
            exclude: vec![],
        };
        assert!(matches!(
            resolve(Path::new("."), &filter),
            Err(Error::InvalidConfig(_))
        ));
    }

    #[test]
    fn read_text_reads_utf8_and_skips_binary() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.rs"), "// llmlint: ignore[r] x\n").unwrap();
        // An invalid UTF-8 byte sequence: not text, so it carries no directive.
        fs::write(dir.path().join("blob.bin"), [0xff, 0xfe, 0x00]).unwrap();
        assert_eq!(
            read_text(dir.path(), Path::new("a.rs")).unwrap().as_deref(),
            Some("// llmlint: ignore[r] x\n")
        );
        assert_eq!(read_text(dir.path(), Path::new("blob.bin")).unwrap(), None);
    }

    #[test]
    fn read_text_missing_file_is_an_error() {
        let dir = tempdir().unwrap();
        assert!(matches!(
            read_text(dir.path(), Path::new("nope.rs")),
            Err(Error::Io(_))
        ));
    }

    #[test]
    fn from_cli_relativizes_and_sorts() {
        let root = Path::new("/repo");
        let files = vec![PathBuf::from("/repo/b.rs"), PathBuf::from("/repo/a.rs")];
        let out = from_cli(root, &files);
        assert_eq!(out, vec![PathBuf::from("a.rs"), PathBuf::from("b.rs")]);
    }
}
