//! `llmlint plugins`: report the on-disk plugin cache, and `llmlint plugins
//! clear`: empty it.
//!
//! Both are deterministic and free of any model, oneharness, or network call —
//! they read (or remove) what [`crate::io::plugins`] wrote. They exist because
//! the cost of a stale cached plugin was almost entirely *diagnosis*: a pin is a
//! range, so a rule the plugin declares today can be absent from the copy a
//! long-lived host resolved weeks ago, and the symptom (correct, current
//! suppressions reported as naming rules that do not exist) points at everything
//! except the cache. One line per cached entry — its URL, its pin, the version
//! it resolved to, when the origin last confirmed it, and whether a newer
//! version satisfying that pin is known — answers the question directly.

use std::path::PathBuf;
use std::time::{Duration, UNIX_EPOCH};

use serde_json::json;

use crate::cli::{OutputFormat, PluginsArgs, PluginsCommand};
use crate::errors::{Error, Result};
use crate::io::history;
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
        let items: Vec<_> = entries
            .iter()
            .map(|e| {
                json!({
                    "url": e.url,
                    "pin": e.pin,
                    "version": e.version,
                    "fetched_at": timestamp(e.fetched_at),
                    "newer": e.newer,
                })
            })
            .collect();
        let doc = json!({ "dir": dir.display().to_string(), "plugins": items });
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
        let pin = match &e.pin {
            Some(p) => format!("@{p}"),
            None => String::new(),
        };
        let newer = match &e.newer {
            Some(v) => format!("  newer: {v}"),
            None => String::new(),
        };
        out.push_str(&format!(
            "  {}{pin}  version {}  fetched {}{newer}\n",
            e.url,
            e.version,
            timestamp(e.fetched_at)
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

/// Epoch seconds as the same RFC 3339 UTC stamp the history records use.
fn timestamp(secs: u64) -> String {
    history::format_timestamp(UNIX_EPOCH + Duration::from_secs(secs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn entry(version: &str, newer: Option<&str>) -> CachedPlugin {
        CachedPlugin {
            url: "https://x/rules.yml".into(),
            pin: Some("1".into()),
            version: version.into(),
            fetched_at: 1_700_000_000,
            newer: newer.map(str::to_string),
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
                "https://x/rules.yml@1  version 1.2  fetched 2023-11-14T22:13:20Z  newer: 1.4"
            ),
            "got:\n{out}"
        );
        // The entry the pin resolves to has nothing newer to report.
        assert!(out.contains("version 1.4  fetched 2023-11-14T22:13:20Z\n"));
        assert!(!out.contains("version 1.4  fetched 2023-11-14T22:13:20Z  newer"));
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
        assert_eq!(v["plugins"][0]["fetched_at"], "2023-11-14T22:13:20Z");
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
