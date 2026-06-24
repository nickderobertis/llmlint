//! Plugin (`plugins:`) resolution: parse a spec into a local path or a URL
//! (optionally version-pinned), fetch a URL's YAML, validate its declared
//! version against the pin, and cache versioned fetches on disk so an unchanged
//! pin never refetches.
//!
//! A spec is one of:
//! - a **local path** (`./team.yml`, `/abs/rules.yml`) — resolved relative to
//!   the including file;
//! - a **URL** (`https://…`, `http://…`, `file://…`), optionally pinned with a
//!   trailing `@version` (`…/rules.yml@1.2.3`).
//!
//! `http(s)` URLs are fetched over HTTPS with a pure-Rust client (`ureq` on
//! rustls with bundled Mozilla roots) — no external tools and no system TLS, so
//! the binary stays self-contained and cross-platform. Standard `HTTP(S)_PROXY`
//! / `NO_PROXY` env vars are honored. `file://` URLs are read directly. Bundled
//! plugins (see [`crate::io::assets::bundled_url`]) short-circuit to their
//! embedded copy and never touch the network or cache.
//!
//! **Caching:** a *pinned* fetch (`url@version`) is written under the cache dir
//! keyed by the URL and the pin, and reused on later runs without refetching —
//! the pin is the cache key, so bumping it (the plugin author publishing a new
//! version) is what busts the cache. An *unpinned* URL has no stable identity
//! and is fetched every run.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::domain::version::{Version, VersionReq};
use crate::errors::{io_err, Error, Result};
use crate::io::assets;

/// A parsed `plugins:` entry.
#[derive(Debug, PartialEq, Eq)]
pub enum PluginRef {
    /// A local config file (path resolved relative to the including file).
    Local(PathBuf),
    /// A remote/URL config, optionally pinned to a version.
    Remote {
        url: String,
        req: Option<VersionReq>,
    },
}

/// Environment-derived knobs for plugin resolution.
#[derive(Debug, Clone)]
pub struct ResolveOpts {
    /// Where pinned fetches are cached. `None` disables caching entirely.
    pub cache_dir: Option<PathBuf>,
    /// Force a refetch even when a cached copy exists.
    pub refresh: bool,
}

impl ResolveOpts {
    /// Build from the environment: `LLMLINT_CACHE_DIR` (else the platform cache
    /// dir) and `LLMLINT_PLUGIN_REFRESH`.
    pub fn from_env() -> Self {
        let cache_dir = std::env::var_os("LLMLINT_CACHE_DIR")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .or_else(default_cache_dir);
        let refresh = std::env::var_os("LLMLINT_PLUGIN_REFRESH")
            .map(|v| v == "1" || v == "true")
            .unwrap_or(false);
        ResolveOpts { cache_dir, refresh }
    }
}

/// The platform cache directory for llmlint plugins, or `None` if no home/cache
/// directory can be determined.
fn default_cache_dir() -> Option<PathBuf> {
    if let Some(x) = non_empty_var("XDG_CACHE_HOME") {
        return Some(PathBuf::from(x).join("llmlint").join("plugins"));
    }
    #[cfg(windows)]
    if let Some(a) = non_empty_var("LOCALAPPDATA") {
        return Some(
            PathBuf::from(a)
                .join("llmlint")
                .join("cache")
                .join("plugins"),
        );
    }
    if let Some(h) = non_empty_var("HOME") {
        return Some(
            PathBuf::from(h)
                .join(".cache")
                .join("llmlint")
                .join("plugins"),
        );
    }
    None
}

fn non_empty_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|s| !s.is_empty())
}

/// Parse a `plugins:` spec into a [`PluginRef`].
pub fn parse_spec(spec: &str) -> Result<PluginRef> {
    if spec.starts_with("llmlint:") {
        // The pre-URL bundled-plugin scheme. Give a clear migration message
        // rather than treating it as a (missing) local file.
        let hint = if spec == "llmlint:config-lint" {
            format!(" (use {:?})", format!("{}@1", assets::CONFIG_LINT_URL))
        } else {
            String::new()
        };
        return Err(Error::PluginSpec(format!(
            "the `llmlint:` plugin scheme was removed; reference plugins by URL{hint}"
        )));
    }
    if is_url(spec) {
        let (url, req) = split_version(spec)?;
        return Ok(PluginRef::Remote { url, req });
    }
    Ok(PluginRef::Local(PathBuf::from(spec)))
}

fn is_url(spec: &str) -> bool {
    spec.starts_with("http://") || spec.starts_with("https://") || spec.starts_with("file://")
}

/// Split a trailing `@version` pin off a URL. Only a suffix made entirely of
/// digits and dots is treated as a pin, so userinfo (`https://user@host/…`) and
/// other `@`s are left alone.
fn split_version(spec: &str) -> Result<(String, Option<VersionReq>)> {
    if let Some(at) = spec.rfind('@') {
        let ver = &spec[at + 1..];
        if !ver.is_empty() && ver.chars().all(|c| c.is_ascii_digit() || c == '.') {
            let req = VersionReq::parse(ver).map_err(Error::PluginSpec)?;
            return Ok((spec[..at].to_string(), Some(req)));
        }
    }
    Ok((spec.to_string(), None))
}

/// Resolve a remote plugin to its YAML text, honoring the embedded bundle, the
/// on-disk cache, and the version pin.
pub fn load_remote(url: &str, req: &Option<VersionReq>, opts: &ResolveOpts) -> Result<String> {
    // Bundled plugins resolve offline from the embedded copy.
    if let Some(content) = assets::bundled_url(url) {
        validate_version(url, req, content)?;
        return Ok(content.to_string());
    }

    let cache_path = match (req, &opts.cache_dir) {
        (Some(req), Some(dir)) => Some(cache_path(dir, url, req)),
        _ => None,
    };

    // A pinned fetch already cached is reused without refetching.
    if !opts.refresh {
        if let Some(p) = &cache_path {
            if p.is_file() {
                return std::fs::read_to_string(p)
                    .map_err(|e| io_err(format!("reading cached plugin {}", p.display()), e));
            }
        }
    }

    let text = raw_fetch(url)?;
    validate_version(url, req, &text)?;

    if let Some(p) = &cache_path {
        write_cache(p, &text)?;
    }
    Ok(text)
}

/// Probe just the top-level `version` of a fetched plugin config.
#[derive(Deserialize)]
struct VersionProbe {
    #[serde(default)]
    version: Option<Version>,
}

/// Check a fetched plugin's declared version against the requested pin. An
/// unpinned plugin accepts any (or no) declared version.
fn validate_version(url: &str, req: &Option<VersionReq>, text: &str) -> Result<()> {
    let Some(req) = req else {
        return Ok(());
    };
    let probe: VersionProbe = serde_yaml_ng::from_str(text).map_err(|e| Error::PluginFetch {
        url: url.to_string(),
        message: format!("reading plugin version: {e}"),
    })?;
    match probe.version {
        Some(v) if req.matches(&v) => Ok(()),
        Some(v) => Err(Error::PluginVersionMismatch {
            url: url.to_string(),
            requested: req.to_string(),
            declared: v.to_string(),
        }),
        None => Err(Error::PluginMissingVersion {
            url: url.to_string(),
            requested: req.to_string(),
        }),
    }
}

/// Fetch a URL's text: a direct read for `file://`, otherwise an HTTPS GET.
fn raw_fetch(url: &str) -> Result<String> {
    if let Some(path) = file_url_path(url) {
        return std::fs::read_to_string(&path).map_err(|e| Error::PluginFetch {
            url: url.to_string(),
            message: format!("reading {}: {e}", path.display()),
        });
    }
    http_get(url)
}

/// HTTPS GET via `ureq` (rustls, bundled roots). Honors `HTTP(S)_PROXY` /
/// `NO_PROXY`; a non-2xx status or transport error becomes a [`Error::PluginFetch`].
fn http_get(url: &str) -> Result<String> {
    let fetch_err = |message: String| Error::PluginFetch {
        url: url.to_string(),
        message,
    };
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .proxy(ureq::Proxy::try_from_env())
        .build()
        .into();
    let mut resp = agent
        .get(url)
        .call()
        .map_err(|e| fetch_err(e.to_string()))?;
    resp.body_mut()
        .read_to_string()
        .map_err(|e| fetch_err(format!("reading response body: {e}")))
}

/// Map a `file://` URL to a filesystem path (`None` for other schemes).
fn file_url_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    // `file://localhost/path` and `file:///path` both mean `/path`.
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    Some(PathBuf::from(drive_letter_path(rest)))
}

/// On Windows a `file://` URL for an absolute path is `file:///C:/dir/x` — after
/// stripping the scheme the remainder is `/C:/dir/x`, so drop the leading slash
/// that precedes the drive letter. A no-op elsewhere (and for already-drive-less
/// paths).
#[cfg(windows)]
fn drive_letter_path(s: &str) -> &str {
    let b = s.as_bytes();
    if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
        &s[1..]
    } else {
        s
    }
}

#[cfg(not(windows))]
fn drive_letter_path(s: &str) -> &str {
    s
}

/// On-disk cache location for a pinned fetch: a per-URL subdir (named by a hash
/// of the URL) holding one file per pin.
fn cache_path(dir: &Path, url: &str, req: &VersionReq) -> PathBuf {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut h);
    dir.join(format!("{:016x}", h.finish()))
        .join(format!("{req}.yml"))
}

fn write_cache(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| io_err(format!("creating plugin cache dir {}", parent.display()), e))?;
    }
    std::fs::write(path, text)
        .map_err(|e| io_err(format!("writing plugin cache {}", path.display()), e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn opts_with_cache(dir: &Path) -> ResolveOpts {
        ResolveOpts {
            cache_dir: Some(dir.to_path_buf()),
            refresh: false,
        }
    }

    /// Build a valid `file://` URL from a path on any platform: forward slashes,
    /// with a leading slash before a Windows drive letter (`C:/x` -> `///C:/x`).
    fn file_url(path: &Path) -> String {
        let s = path.display().to_string().replace('\\', "/");
        if s.starts_with('/') {
            format!("file://{s}")
        } else {
            format!("file:///{s}")
        }
    }

    #[test]
    fn parse_spec_classifies_local_and_remote() {
        assert_eq!(
            parse_spec("./team.yml").unwrap(),
            PluginRef::Local(PathBuf::from("./team.yml"))
        );
        assert_eq!(
            parse_spec("https://x/p.yml").unwrap(),
            PluginRef::Remote {
                url: "https://x/p.yml".into(),
                req: None
            }
        );
        match parse_spec("https://x/p.yml@1.2").unwrap() {
            PluginRef::Remote { url, req } => {
                assert_eq!(url, "https://x/p.yml");
                assert_eq!(req.unwrap().to_string(), "1.2");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parse_spec_rejects_removed_scheme() {
        let err = parse_spec("llmlint:config-lint").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("was removed"));
        assert!(msg.contains("config_lint.yml"));
    }

    #[test]
    fn split_version_ignores_userinfo_at() {
        // A trailing `@host…` is not a pin (not all digits/dots).
        let (url, req) = split_version("https://user@example.com/p.yml").unwrap();
        assert_eq!(url, "https://user@example.com/p.yml");
        assert!(req.is_none());
    }

    #[test]
    fn file_url_path_strips_scheme_and_localhost() {
        assert_eq!(
            file_url_path("file:///a/b.yml"),
            Some(PathBuf::from("/a/b.yml"))
        );
        assert_eq!(
            file_url_path("file://localhost/a/b.yml"),
            Some(PathBuf::from("/a/b.yml"))
        );
        assert!(file_url_path("https://x/p.yml").is_none());
        #[cfg(windows)]
        assert_eq!(
            file_url_path("file:///C:/a/b.yml"),
            Some(PathBuf::from("C:/a/b.yml"))
        );
    }

    #[test]
    fn cache_path_is_per_url_and_per_pin() {
        let dir = tempdir().unwrap();
        let a = cache_path(
            dir.path(),
            "https://x/p.yml",
            &VersionReq::parse("1").unwrap(),
        );
        let b = cache_path(
            dir.path(),
            "https://x/p.yml",
            &VersionReq::parse("2").unwrap(),
        );
        let c = cache_path(
            dir.path(),
            "https://y/p.yml",
            &VersionReq::parse("1").unwrap(),
        );
        assert_ne!(a, b); // same url, different pin
        assert_ne!(a, c); // different url
        assert_eq!(a.file_name().unwrap(), "1.yml");
    }

    #[test]
    fn file_plugin_is_fetched_validated_and_cached() {
        let dir = tempdir().unwrap();
        let plugin = dir.path().join("plug.yml");
        std::fs::write(
            &plugin,
            "version: 1\nrules:\n  - {name: r, description: d}\n",
        )
        .unwrap();
        let cache = tempdir().unwrap();
        let opts = opts_with_cache(cache.path());
        let url = file_url(&plugin);
        let req = Some(VersionReq::parse("1").unwrap());

        let text = load_remote(&url, &req, &opts).unwrap();
        assert!(text.contains("name: r"));

        // Now mutate the source; a cached pin must NOT refetch.
        std::fs::write(&plugin, "version: 1\nrules: []\n").unwrap();
        let again = load_remote(&url, &req, &opts).unwrap();
        assert!(
            again.contains("name: r"),
            "expected cached copy, got: {again}"
        );

        // refresh: true ignores the cache and re-reads the (now empty) source.
        let refreshed = load_remote(
            &url,
            &req,
            &ResolveOpts {
                refresh: true,
                ..opts.clone()
            },
        )
        .unwrap();
        assert!(!refreshed.contains("name: r"));
    }

    #[test]
    fn unpinned_plugin_is_not_cached() {
        let dir = tempdir().unwrap();
        let plugin = dir.path().join("plug.yml");
        std::fs::write(&plugin, "rules: []\n").unwrap();
        let cache = tempdir().unwrap();
        let opts = opts_with_cache(cache.path());
        load_remote(&file_url(&plugin), &None, &opts).unwrap();
        // No cache files written for an unpinned fetch.
        let entries = std::fs::read_dir(cache.path()).unwrap().count();
        assert_eq!(entries, 0);
    }

    #[test]
    fn version_mismatch_and_missing_are_errors() {
        let dir = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let opts = opts_with_cache(cache.path());

        let v2 = dir.path().join("v2.yml");
        std::fs::write(&v2, "version: 2\nrules: []\n").unwrap();
        let err = load_remote(
            &file_url(&v2),
            &Some(VersionReq::parse("1").unwrap()),
            &opts,
        )
        .unwrap_err();
        assert!(matches!(err, Error::PluginVersionMismatch { .. }));

        let none = dir.path().join("none.yml");
        std::fs::write(&none, "rules: []\n").unwrap();
        let err = load_remote(
            &file_url(&none),
            &Some(VersionReq::parse("1").unwrap()),
            &opts,
        )
        .unwrap_err();
        assert!(matches!(err, Error::PluginMissingVersion { .. }));
    }

    #[test]
    fn missing_file_url_is_a_fetch_error() {
        let opts = ResolveOpts {
            cache_dir: None,
            refresh: false,
        };
        let err = load_remote("file:///no/such/plugin.yml", &None, &opts).unwrap_err();
        assert!(matches!(err, Error::PluginFetch { .. }));
    }

    #[test]
    fn bundled_url_resolves_offline_and_validates_pin() {
        let opts = ResolveOpts {
            cache_dir: None,
            refresh: false,
        };
        // Resolves from the embedded copy — no network, no cache.
        let text = load_remote(
            assets::CONFIG_LINT_URL,
            &Some(VersionReq::parse("1").unwrap()),
            &opts,
        )
        .unwrap();
        assert!(text.contains("name_matches_description"));
        // A pin the embedded version can't satisfy still errors.
        let err = load_remote(
            assets::CONFIG_LINT_URL,
            &Some(VersionReq::parse("2").unwrap()),
            &opts,
        )
        .unwrap_err();
        assert!(matches!(err, Error::PluginVersionMismatch { .. }));
    }

    #[test]
    fn http_connection_failure_is_a_fetch_error() {
        // Port 1 refuses immediately, exercising the transport-error branch of
        // the HTTPS client without any external network.
        let opts = ResolveOpts {
            cache_dir: None,
            refresh: false,
        };
        let err = load_remote("http://127.0.0.1:1/nope.yml", &None, &opts).unwrap_err();
        assert!(matches!(err, Error::PluginFetch { .. }));
    }

    #[test]
    fn unparseable_plugin_version_is_a_fetch_error() {
        let dir = tempdir().unwrap();
        let plugin = dir.path().join("bad.yml");
        // Invalid YAML so the version probe fails to parse.
        std::fs::write(&plugin, "version: : :\n  - oops\n").unwrap();
        let opts = ResolveOpts {
            cache_dir: None,
            refresh: false,
        };
        let err = load_remote(
            &file_url(&plugin),
            &Some(VersionReq::parse("1").unwrap()),
            &opts,
        )
        .unwrap_err();
        assert!(matches!(err, Error::PluginFetch { .. }));
    }

    #[test]
    fn from_env_reads_cache_dir_override() {
        // Exercise the env-driven constructor without disturbing global state
        // beyond this test's scope.
        let prev = std::env::var_os("LLMLINT_CACHE_DIR");
        std::env::set_var("LLMLINT_CACHE_DIR", "/tmp/llmlint-cache-test");
        let opts = ResolveOpts::from_env();
        assert_eq!(
            opts.cache_dir,
            Some(PathBuf::from("/tmp/llmlint-cache-test"))
        );
        assert!(!opts.refresh);
        match prev {
            Some(v) => std::env::set_var("LLMLINT_CACHE_DIR", v),
            None => std::env::remove_var("LLMLINT_CACHE_DIR"),
        }
    }
}
