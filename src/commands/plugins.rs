//! `llmlint plugins`: report the on-disk plugin cache, and `llmlint plugins
//! clear`: empty it.
//!
//! Both are deterministic and free of any model, oneharness, or network call —
//! they read (or remove) what [`crate::io::plugins`] wrote, which is where the
//! cache's own contract lives. One line per cached entry, self-contained, so a
//! plugin resolving to an unexpected version is read off the report rather than
//! inferred.

// llmlint: ignore-file[new_code_lands_in_a_project] The rule asks which Nx
// project covers this path; llmlint has no project graph, by the deliberate
// exclusion AGENTS.md records under "Stack and composition".

use std::path::PathBuf;

use serde_json::json;

use crate::cli::{OutputFormat, PluginsArgs, PluginsCommand};
use crate::errors::{Error, Result};
use crate::io::plugins::{self, CachedPlugin, ResolveOpts};

pub fn run(args: PluginsArgs) -> Result<i32> {
    let dir = cache_dir(args.dir)?;
    match args.command.unwrap_or(PluginsCommand::List) {
        PluginsCommand::List => {
            let entries = plugins::list_cached(&dir)?;
            print!("{}", render_list(&dir, &entries, args.format));
        }
        PluginsCommand::Clear => {
            let removed = plugins::clear_cached(&dir)?;
            print!("{}", render_clear(&dir, removed, args.format));
        }
    }
    Ok(0)
}

/// `--dir`, else the same directory resolution a lint run uses (the
/// `LLMLINT_CACHE_DIR` override, else the platform cache dir).
fn cache_dir(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(d) = explicit {
        return Ok(d);
    }
    ResolveOpts::from_env()?.cache_dir.ok_or_else(|| {
        Error::Io(format!(
            "no plugin cache directory could be determined (no HOME/XDG_CACHE_HOME); \
             set {} or pass --dir",
            plugins::CACHE_DIR_VAR
        ))
    })
}

/// One self-contained line per cached entry, so a reader can grep it and a
/// scripted check can read a single line without tracking a group header.
fn render_list(dir: &std::path::Path, entries: &[CachedPlugin], format: OutputFormat) -> String {
    if format == OutputFormat::Json {
        // Serialized straight from `CachedPlugin`, so the machine-readable shape
        // is the model rather than a second statement of it.
        let doc = json!({ "dir": dir.display().to_string(), "plugins": entries });
        return format!(
            "{}\n",
            serde_json::to_string_pretty(&doc).unwrap_or_default()
        );
    }
    let mut out = format!("plugin cache: {}\n", dir.display());
    if entries.is_empty() {
        out.push_str("  (empty — no plugin has been fetched into this cache)\n");
        return out;
    }
    for e in entries {
        let newer = match &e.newer {
            Some(v) => format!("  newer: {v}"),
            None => String::new(),
        };
        out.push_str(&format!(
            "  {}@{}  version {}  confirmed {}{newer}\n",
            e.url,
            e.pin,
            e.version,
            e.confirmed_at_utc()
        ));
    }
    out
}

fn render_clear(dir: &std::path::Path, removed: usize, format: OutputFormat) -> String {
    if format == OutputFormat::Json {
        let doc = json!({ "dir": dir.display().to_string(), "cleared": removed });
        return format!(
            "{}\n",
            serde_json::to_string_pretty(&doc).unwrap_or_default()
        );
    }
    format!(
        "llmlint: cleared {removed} cached plugin entr{} from {}\n",
        if removed == 1 { "y" } else { "ies" },
        dir.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::version::{Version, VersionReq};
    use std::path::Path;

    fn entry(version: &str, newer: Option<&str>) -> CachedPlugin {
        CachedPlugin {
            url: "https://x/rules.yml".into(),
            pin: VersionReq::parse("1").unwrap(),
            version: Version::parse(version).unwrap(),
            confirmed_at: std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000),
            newer: newer.map(|v| Version::parse(v).unwrap()),
        }
    }

    #[test]
    fn list_names_url_pin_version_time_and_newer() {
        let out = render_list(
            Path::new("/cache"),
            &[entry("1.2", Some("1.4")), entry("1.4", None)],
            OutputFormat::Human,
        );
        assert!(out.contains("plugin cache: /cache"), "got:\n{out}");
        assert!(
            out.contains(
                "https://x/rules.yml@1  version 1.2  confirmed 2023-11-14T22:13:20Z  newer: 1.4"
            ),
            "got:\n{out}"
        );
        assert!(out.contains("version 1.4  confirmed 2023-11-14T22:13:20Z\n"));
        assert!(!out.contains("version 1.4  confirmed 2023-11-14T22:13:20Z  newer"));
    }

    #[test]
    fn an_empty_cache_says_so_rather_than_printing_nothing() {
        let out = render_list(Path::new("/cache"), &[], OutputFormat::Human);
        assert!(out.contains("(empty"), "got:\n{out}");
    }

    #[test]
    fn json_carries_the_same_fields() {
        let out = render_list(
            Path::new("/cache"),
            &[entry("1.2", Some("1.4"))],
            OutputFormat::Json,
        );
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["dir"], "/cache");
        assert_eq!(v["plugins"][0]["url"], "https://x/rules.yml");
        assert_eq!(v["plugins"][0]["pin"], "1");
        assert_eq!(v["plugins"][0]["version"], "1.2");
        assert_eq!(v["plugins"][0]["newer"], "1.4");
        assert_eq!(v["plugins"][0]["confirmed_at"], "2023-11-14T22:13:20Z");
    }

    #[test]
    fn clear_reports_what_it_removed() {
        assert!(render_clear(Path::new("/cache"), 1, OutputFormat::Human)
            .contains("1 cached plugin entry from /cache"));
        assert!(render_clear(Path::new("/cache"), 0, OutputFormat::Human)
            .contains("0 cached plugin entries"));
        let v: serde_json::Value =
            serde_json::from_str(&render_clear(Path::new("/cache"), 2, OutputFormat::Json))
                .unwrap();
        assert_eq!(v["cleared"], 2);
    }

    #[test]
    fn explicit_dir_wins_over_the_environment() {
        assert_eq!(
            cache_dir(Some(PathBuf::from("/tmp/x"))).unwrap(),
            PathBuf::from("/tmp/x")
        );
    }
}
