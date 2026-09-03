//! Plugin (`plugins:`) resolution: parse a spec into a local path or a URL
//! (optionally version-pinned), fetch a URL's YAML, validate its declared
//! version against the pin, and cache fetches on disk keyed by the version the
//! fetched document *declares*.
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
//! **Caching.** A pin like `@1` is a *range* — the plugin promises that
//! non-breaking bumps reach consumers automatically — so the pin cannot be the
//! cache key: keying by it made the first version a machine fetched the version
//! it kept forever, and a long-lived host then judged against weeks-old rules
//! while reporting correct, current suppressions as naming rules that do not
//! exist. The on-disk shape is therefore:
//!
//! - an entry is keyed by the URL and **the version the fetched document
//!   declares** (`<url-hash>/v<version>.yml`), so a new minor is a new entry
//!   rather than a hit on the old one;
//! - beside each entry stands its metadata ([`CacheMeta`], `v<version>.json`):
//!   the URL, the pin the fetch was made under, the resolved version, when it
//!   was fetched, and whatever revalidation validators the response carried
//!   (`ETag` / `Last-Modified`);
//! - resolution takes the **newest** cached entry satisfying the pin and
//!   revalidates it against the origin once its metadata is older than the
//!   freshness window ([`ResolveOpts::ttl_secs`]) — a `304 Not Modified` answer
//!   refreshes the timestamp and reuses the entry, and a newer document is
//!   stored as a new entry under its own version and used;
//! - a revalidation that cannot be made — offline, a transport failure, a
//!   refusal — **reuses the cached entry and does not fail the run**: a cache is
//!   a speed-up and must never become a network dependency;
//! - an entry written under the previous layout (a filename naming the *pin*,
//!   with no metadata beside it) is never read as though it named a resolved
//!   version — it is simply invisible, and the plugin is fetched afresh.
//!
//! The freshness window (`LLMLINT_PLUGIN_TTL`, seconds) and the force-refetch
//! switch (`LLMLINT_PLUGIN_REFRESH`) are the two knobs, both read from the
//! environment. An *unpinned* URL has no stable identity and is fetched every
//! run, uncached.
//!
//! `llmlint plugins` reports the cache and `llmlint plugins clear` empties it
//! (see [`crate::commands::plugins`]).

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::domain::version::{Version, VersionReq};
use crate::errors::{io_err, Error, Result};
use crate::io::assets;

/// Env var overriding the cache directory.
pub const CACHE_DIR_VAR: &str = "LLMLINT_CACHE_DIR";
/// Env var forcing a refetch even when a cached entry exists.
pub const REFRESH_VAR: &str = "LLMLINT_PLUGIN_REFRESH";
/// Env var setting the freshness window, in seconds.
pub const TTL_VAR: &str = "LLMLINT_PLUGIN_TTL";
/// Default freshness window: an entry confirmed within the last hour is reused
/// without asking the origin anything, so a burst of runs costs one request.
pub const DEFAULT_TTL_SECS: u64 = 3600;

/// Version of the on-disk cache-entry metadata shape ([`CacheMeta`]) — a
/// serialized contract other processes and later releases read, so it is bumped
/// deliberately. An entry whose metadata declares a different schema is ignored
/// (and refetched) rather than misread, the same way a previous-layout entry is.
pub const CACHE_SCHEMA: u32 = 1;

/// Filename prefix of a cache entry, which is followed by the **resolved**
/// version (`v1.4.2.yml` + `v1.4.2.json`). The prefix plus the required sidecar
/// metadata is what keeps a previous-layout file (named for a *pin*, e.g.
/// `1.yml`, with no sidecar) from ever being read as an entry.
const ENTRY_PREFIX: &str = "v";

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
    /// Where fetches are cached. `None` disables caching entirely.
    pub cache_dir: Option<PathBuf>,
    /// Force a refetch even when a cached entry exists, replacing what the cache
    /// holds.
    pub refresh: bool,
    /// Freshness window in seconds: a cached entry confirmed longer ago than
    /// this is revalidated against the origin before it is reused. `0` always
    /// revalidates.
    pub ttl_secs: u64,
}

impl ResolveOpts {
    /// Build from the environment: [`CACHE_DIR_VAR`] (else the platform cache
    /// dir), [`REFRESH_VAR`], and [`TTL_VAR`]. A malformed TTL is an exit-2
    /// [`Error::Env`] located to the variable — validated at the boundary, never
    /// silently ignored.
    pub fn from_env() -> Result<Self> {
        let cache_dir = std::env::var_os(CACHE_DIR_VAR)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .or_else(default_cache_dir);
        let refresh = std::env::var_os(REFRESH_VAR)
            .map(|v| v == "1" || v == "true")
            .unwrap_or(false);
        let ttl_secs = match non_empty_var(TTL_VAR) {
            Some(v) => v.trim().parse::<u64>().map_err(|_| Error::Env {
                var: TTL_VAR.to_string(),
                message: format!(
                    "expected a whole number of seconds (the plugin cache freshness \
                     window); got {v:?}"
                ),
            })?,
            None => DEFAULT_TTL_SECS,
        };
        Ok(ResolveOpts {
            cache_dir,
            refresh,
            ttl_secs,
        })
    }
}

/// Where a resolved plugin's text came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// The copy embedded in the binary (never network, never cache).
    Bundled,
    /// An on-disk cache entry (fresh, or revalidated as unchanged, or kept
    /// because the origin could not be reached).
    Cache,
    /// Freshly fetched from the origin.
    Fetched,
}

impl Origin {
    /// Short label for diagnostics.
    pub fn label(self) -> &'static str {
        match self {
            Origin::Bundled => "bundled",
            Origin::Cache => "cache",
            Origin::Fetched => "fetched",
        }
    }
}

/// What one `plugins:` URL resolved to, for diagnostics: the URL, the pin it was
/// requested under, the version the document declares, and where the text came
/// from. Carried on [`crate::io::configfs::Loaded`] so a message about a rule
/// nothing declares can name the plugins in play and their resolved versions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginResolution {
    pub url: String,
    pub pin: Option<String>,
    pub version: Option<String>,
    pub origin: Origin,
}

impl PluginResolution {
    /// One human line: `url@pin -> version 1.4 (from cache)`.
    pub fn to_human(&self) -> String {
        let pin = match &self.pin {
            Some(p) => format!("@{p}"),
            None => String::new(),
        };
        let version = match &self.version {
            Some(v) => format!("version {v}"),
            None => "no declared version".to_string(),
        };
        format!(
            "{}{pin} -> {version} (from {})",
            self.url,
            self.origin.label()
        )
    }
}

/// A resolved plugin: its YAML text plus how it resolved.
#[derive(Debug, Clone)]
pub struct Resolution {
    pub text: String,
    pub info: PluginResolution,
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
/// on-disk cache (freshness window + revalidation), and the version pin.
pub fn load_remote(url: &str, req: &Option<VersionReq>, opts: &ResolveOpts) -> Result<Resolution> {
    // Bundled plugins resolve offline from the embedded copy.
    if let Some(content) = assets::bundled_url(url) {
        validate_version(url, req.as_ref(), content)?;
        return Ok(resolution(
            url,
            req.as_ref(),
            declared_version(url, content).unwrap_or(None),
            content.to_string(),
            Origin::Bundled,
        ));
    }

    match (req, &opts.cache_dir) {
        // Only a pinned fetch has a stable identity worth caching.
        (Some(r), Some(dir)) => load_cached(url, r, &url_dir(dir, url), opts),
        _ => {
            let body = fetch_body(url)?;
            validate_version(url, req.as_ref(), &body.text)?;
            let version = declared_version(url, &body.text)?;
            Ok(resolution(
                url,
                req.as_ref(),
                version,
                body.text,
                Origin::Fetched,
            ))
        }
    }
}

/// The cached half of [`load_remote`]: pick the newest entry satisfying the pin,
/// revalidate it when stale, and fall back to fetching when the cache holds
/// nothing usable (or `--refresh` forces it).
fn load_cached(url: &str, req: &VersionReq, dir: &Path, opts: &ResolveOpts) -> Result<Resolution> {
    let entries = if opts.refresh {
        Vec::new() // a forced refetch never consults the cache
    } else {
        read_entries(dir, url)
    };
    if let Some(entry) = newest_matching(&entries, req) {
        let now = now_secs();
        if !is_stale(&entry.meta, opts.ttl_secs, now) {
            return read_entry(url, req, entry);
        }
        match revalidate(url, &entry.meta) {
            // Unchanged: the origin confirmed the entry, so its clock restarts.
            Ok(None) => {
                touch(entry, now)?;
                return read_entry(url, req, entry);
            }
            Ok(Some(body)) => {
                // A newer document satisfying the same pin is adopted with no
                // change to the consuming repository. One that does *not*
                // satisfy the pin (the origin moved past the pinned range, or
                // serves something unreadable) leaves the pinned entry standing:
                // the consumer asked for this range and the cache still has it.
                if let Some(v) = declared_version(url, &body.text).unwrap_or(None) {
                    if req.matches(&v) {
                        store(dir, url, req, &v, &body, now)?;
                        return Ok(resolution(
                            url,
                            Some(req),
                            Some(v),
                            body.text,
                            Origin::Fetched,
                        ));
                    }
                }
                return read_entry(url, req, entry);
            }
            // Offline, a transport failure, a refusal: a cache is a speed-up, not
            // a network dependency, so the run keeps working from what it has.
            // The timestamp is deliberately *not* refreshed, so the next run
            // tries the origin again.
            Err(_) => return read_entry(url, req, entry),
        }
    }

    let body = fetch_body(url)?;
    validate_version(url, Some(req), &body.text)?;
    // A pinned fetch that validated always declares a version.
    let version = declared_version(url, &body.text)?;
    if let Some(v) = &version {
        store(dir, url, req, v, &body, now_secs())?;
    }
    Ok(resolution(
        url,
        Some(req),
        version,
        body.text,
        Origin::Fetched,
    ))
}

fn resolution(
    url: &str,
    pin: Option<&VersionReq>,
    version: Option<Version>,
    text: String,
    origin: Origin,
) -> Resolution {
    Resolution {
        text,
        info: PluginResolution {
            url: url.to_string(),
            pin: pin.map(VersionReq::to_string),
            version: version.map(|v| v.to_string()),
            origin,
        },
    }
}

/// Read a cache entry's document. A cached entry that has gone unreadable since
/// it was listed is a real I/O fault (the run would otherwise silently judge
/// against nothing), so it surfaces rather than being swallowed.
fn read_entry(url: &str, req: &VersionReq, entry: &Entry) -> Result<Resolution> {
    let text = std::fs::read_to_string(&entry.data)
        .map_err(|e| io_err(format!("reading cached plugin {}", entry.data.display()), e))?;
    Ok(resolution(
        url,
        Some(req),
        Some(entry.version.clone()),
        text,
        Origin::Cache,
    ))
}

// ---- the on-disk cache ----------------------------------------------------

/// Metadata stored beside each cache entry. Written as `v<version>.json` next to
/// the `v<version>.yml` document it describes; an entry without readable
/// metadata of the current [`CACHE_SCHEMA`] is not an entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheMeta {
    /// On-disk shape version (see [`CACHE_SCHEMA`]).
    pub schema: u32,
    /// The URL this document was fetched from.
    pub url: String,
    /// The pin the fetch was made under (`@1`), which is a *range*, not this
    /// entry's identity.
    pub pin: Option<String>,
    /// The version the fetched document declares — the entry's key.
    pub version: String,
    /// When the origin last confirmed this entry, in seconds since the Unix
    /// epoch (a `304` refreshes it, so it means "confirmed", not "downloaded").
    pub fetched_at: u64,
    /// The `ETag` the response carried, for the next conditional request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    /// The `Last-Modified` the response carried.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
}

/// One cache entry: its metadata and the paths of the two files that hold it.
#[derive(Debug, Clone)]
struct Entry {
    meta: CacheMeta,
    version: Version,
    data: PathBuf,
    meta_path: PathBuf,
}

/// A cached plugin as [`list_cached`] reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedPlugin {
    pub url: String,
    pub pin: Option<String>,
    pub version: String,
    pub fetched_at: u64,
    /// The newest *other* cached version of the same URL that also satisfies
    /// this entry's pin, if any — i.e. this entry is no longer what the pin
    /// resolves to.
    pub newer: Option<String>,
}

/// The per-URL cache subdirectory, named by a hash of the URL.
fn url_dir(dir: &Path, url: &str) -> PathBuf {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut h);
    dir.join(format!("{:016x}", h.finish()))
}

fn entry_paths(dir: &Path, version: &Version) -> (PathBuf, PathBuf) {
    (
        dir.join(format!("{ENTRY_PREFIX}{version}.yml")),
        dir.join(format!("{ENTRY_PREFIX}{version}.json")),
    )
}

/// Every readable entry in one URL's cache subdirectory. Anything that is not a
/// current-schema entry pair — a previous-layout `1.yml` naming a *pin*, a
/// half-written entry, metadata for another URL that collided on the hash — is
/// skipped, never misread. Reading the cache can therefore never fail the run.
fn read_entries(dir: &Path, url: &str) -> Vec<Entry> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for meta_path in rd.flatten().map(|e| e.path()) {
        if meta_path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&meta_path) else {
            continue;
        };
        let Ok(meta) = serde_json::from_str::<CacheMeta>(&text) else {
            continue;
        };
        if meta.schema != CACHE_SCHEMA || (!url.is_empty() && meta.url != url) {
            continue;
        }
        let Ok(version) = Version::parse(&meta.version) else {
            continue;
        };
        let (data, _) = entry_paths(dir, &version);
        if !data.is_file() {
            continue;
        }
        out.push(Entry {
            meta,
            version,
            data,
            meta_path,
        });
    }
    out
}

/// The newest cached entry satisfying `req` — what a pin resolves to.
fn newest_matching<'a>(entries: &'a [Entry], req: &VersionReq) -> Option<&'a Entry> {
    entries
        .iter()
        .filter(|e| req.matches(&e.version))
        .max_by(|a, b| a.version.cmp(&b.version))
}

/// Whether an entry is older than the freshness window. A `fetched_at` in the
/// future (clock skew) counts as fresh.
fn is_stale(meta: &CacheMeta, ttl_secs: u64, now: u64) -> bool {
    now.saturating_sub(meta.fetched_at) >= ttl_secs
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Write an entry (document + metadata) under its **resolved** version.
fn store(
    dir: &Path,
    url: &str,
    req: &VersionReq,
    version: &Version,
    body: &Body,
    now: u64,
) -> Result<()> {
    std::fs::create_dir_all(dir)
        .map_err(|e| io_err(format!("creating plugin cache dir {}", dir.display()), e))?;
    let (data, meta_path) = entry_paths(dir, version);
    std::fs::write(&data, &body.text)
        .map_err(|e| io_err(format!("writing plugin cache {}", data.display()), e))?;
    write_meta(
        &meta_path,
        &CacheMeta {
            schema: CACHE_SCHEMA,
            url: url.to_string(),
            pin: Some(req.to_string()),
            version: version.to_string(),
            fetched_at: now,
            etag: body.etag.clone(),
            last_modified: body.last_modified.clone(),
        },
    )
}

/// Record that the origin confirmed an entry is still current.
fn touch(entry: &Entry, now: u64) -> Result<()> {
    let mut meta = entry.meta.clone();
    meta.fetched_at = now;
    write_meta(&entry.meta_path, &meta)
}

fn write_meta(path: &Path, meta: &CacheMeta) -> Result<()> {
    let json = serde_json::to_string_pretty(meta).map_err(|e| Error::Io(e.to_string()))?;
    std::fs::write(path, format!("{json}\n"))
        .map_err(|e| io_err(format!("writing plugin cache {}", path.display()), e))
}

/// Every cached plugin entry under `dir`, sorted by URL then version, each
/// carrying whether a newer cached version satisfying its pin is known.
pub fn list_cached(dir: &Path) -> Result<Vec<CachedPlugin>> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Ok(Vec::new()); // no cache directory yet — an empty cache
    };
    let mut entries: Vec<Entry> = Vec::new();
    for sub in rd.flatten().map(|e| e.path()) {
        // An empty `url` filter accepts any entry: listing has no URL in hand,
        // and each entry's metadata names its own.
        entries.extend(read_entries(&sub, ""));
    }
    entries.sort_by(|a, b| {
        a.meta
            .url
            .cmp(&b.meta.url)
            .then_with(|| a.version.cmp(&b.version))
    });
    let mut out = Vec::with_capacity(entries.len());
    for e in &entries {
        let req = e
            .meta
            .pin
            .as_deref()
            .and_then(|p| VersionReq::parse(p).ok());
        let newer = entries
            .iter()
            .filter(|o| {
                o.meta.url == e.meta.url
                    && o.version > e.version
                    && req.as_ref().is_none_or(|r| r.matches(&o.version))
            })
            .map(|o| o.version.clone())
            .max()
            .map(|v| v.to_string());
        out.push(CachedPlugin {
            url: e.meta.url.clone(),
            pin: e.meta.pin.clone(),
            version: e.meta.version.clone(),
            fetched_at: e.meta.fetched_at,
            newer,
        });
    }
    Ok(out)
}

/// Remove every cached plugin entry under `dir`, returning how many were
/// removed. Only recognized entry pairs (and the per-URL subdirectories they
/// leave empty) are touched — a shared cache directory keeps whatever else it
/// holds.
pub fn clear_cached(dir: &Path) -> Result<usize> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Ok(0);
    };
    let mut removed = 0;
    for sub in rd.flatten().map(|e| e.path()) {
        for entry in read_entries(&sub, "") {
            std::fs::remove_file(&entry.data)
                .map_err(|e| io_err(format!("removing {}", entry.data.display()), e))?;
            std::fs::remove_file(&entry.meta_path)
                .map_err(|e| io_err(format!("removing {}", entry.meta_path.display()), e))?;
            removed += 1;
        }
        // Prune the per-URL directory when nothing of ours (or anyone's) is left.
        let _ = std::fs::remove_dir(&sub);
    }
    Ok(removed)
}

// ---- fetching -------------------------------------------------------------

/// A fetched document plus whatever revalidation validators the origin supplied.
#[derive(Debug, Clone, Default)]
struct Body {
    text: String,
    etag: Option<String>,
    last_modified: Option<String>,
}

/// Probe just the top-level `version` of a fetched plugin config.
#[derive(Deserialize)]
struct VersionProbe {
    #[serde(default)]
    version: Option<Version>,
}

/// The version a fetched plugin declares, if any. Unparseable YAML is a fetch
/// error (the document is not an llmlint config).
fn declared_version(url: &str, text: &str) -> Result<Option<Version>> {
    let probe: VersionProbe = serde_yaml_ng::from_str(text).map_err(|e| Error::PluginFetch {
        url: url.to_string(),
        message: format!("reading plugin version: {e}"),
    })?;
    Ok(probe.version)
}

/// Check a fetched plugin's declared version against the requested pin. An
/// unpinned plugin accepts any (or no) declared version.
fn validate_version(url: &str, req: Option<&VersionReq>, text: &str) -> Result<()> {
    let Some(req) = req else {
        return Ok(());
    };
    match declared_version(url, text)? {
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

/// Fetch a URL's document unconditionally: a direct read for `file://`,
/// otherwise an HTTPS GET.
fn fetch_body(url: &str) -> Result<Body> {
    match file_url_path(url) {
        Some(path) => read_file_body(url, &path),
        // An unconditional request carries no validator, so the origin has
        // nothing to answer `304` to; `http_get` rejects one as a bad status.
        None => Ok(http_get(url, None)?.unwrap_or_default()),
    }
}

/// Ask the origin whether a cached entry is still current. `Ok(None)` is
/// "unchanged" (a `304 Not Modified`); `Ok(Some(body))` is the current document,
/// which the caller compares against the pin; `Err` is a revalidation that could
/// not be made.
fn revalidate(url: &str, meta: &CacheMeta) -> Result<Option<Body>> {
    match file_url_path(url) {
        // A `file://` origin supplies no validators, so revalidation is a plain
        // re-read and the document itself is the answer.
        Some(path) => read_file_body(url, &path).map(Some),
        None => http_get(url, Some(meta)),
    }
}

fn read_file_body(url: &str, path: &Path) -> Result<Body> {
    let text = std::fs::read_to_string(path).map_err(|e| Error::PluginFetch {
        url: url.to_string(),
        message: format!("reading {}: {e}", path.display()),
    })?;
    Ok(Body {
        text,
        ..Body::default()
    })
}

/// HTTPS GET via `ureq` (rustls, bundled roots). Honors `HTTP(S)_PROXY` /
/// `NO_PROXY`. With `conditional` set, the cached entry's validators go out as
/// `If-None-Match` / `If-Modified-Since` and a `304` answers `Ok(None)`. A
/// transport error or any other non-2xx status becomes an [`Error::PluginFetch`].
fn http_get(url: &str, conditional: Option<&CacheMeta>) -> Result<Option<Body>> {
    let fetch_err = |message: String| Error::PluginFetch {
        url: url.to_string(),
        message,
    };
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .proxy(ureq::Proxy::try_from_env())
        // Statuses are read here (a `304` is an answer, not a failure), so the
        // client must not turn them into transport errors.
        .http_status_as_error(false)
        .build()
        .into();
    let mut req = agent.get(url);
    if let Some(meta) = conditional {
        if let Some(etag) = &meta.etag {
            req = req.header("If-None-Match", etag);
        }
        if let Some(lm) = &meta.last_modified {
            req = req.header("If-Modified-Since", lm);
        }
    }
    let mut resp = req.call().map_err(|e| fetch_err(e.to_string()))?;
    let status = resp.status().as_u16();
    if conditional.is_some() && status == 304 {
        return Ok(None);
    }
    if !(200..300).contains(&status) {
        return Err(fetch_err(format!("HTTP status {status}")));
    }
    let header = |name: &str| {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let etag = header("etag");
    let last_modified = header("last-modified");
    let text = resp
        .body_mut()
        .read_to_string()
        .map_err(|e| fetch_err(format!("reading response body: {e}")))?;
    Ok(Some(Body {
        text,
        etag,
        last_modified,
    }))
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

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;

    use super::*;
    use tempfile::tempdir;

    /// A throwaway localhost HTTP origin for the revalidation journeys: it serves
    /// a body with an `ETag` and answers `304 Not Modified` to a request whose
    /// `If-None-Match` matches, counting the full-body responses it sent. This
    /// exercises the real HTTP client (localhost only — no external network).
    struct TestOrigin {
        base: String,
        bodies: Arc<AtomicUsize>,
        state: Arc<Mutex<(String, String, u16)>>,
    }

    impl TestOrigin {
        fn serve(body: &str, etag: &str) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let bodies = Arc::new(AtomicUsize::new(0));
            let state = Arc::new(Mutex::new((body.to_string(), etag.to_string(), 200)));
            let (bodies_t, state_t) = (Arc::clone(&bodies), Arc::clone(&state));
            thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { continue };
                    let mut buf = [0u8; 2048];
                    let n = stream.read(&mut buf).unwrap_or(0);
                    // Header names are case-insensitive and the HTTP client
                    // sends them lowercased, so match on a lowercased request.
                    let request = String::from_utf8_lossy(&buf[..n]).to_lowercase();
                    let (body, etag, status) = state_t.lock().unwrap().clone();
                    let resp = if status != 200 {
                        format!("HTTP/1.1 {status} Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    } else if request.contains(&format!("if-none-match: {}", etag.to_lowercase())) {
                        format!("HTTP/1.1 304 Not Modified\r\nETag: {etag}\r\nConnection: close\r\n\r\n")
                    } else {
                        bodies_t.fetch_add(1, Ordering::SeqCst);
                        format!(
                            "HTTP/1.1 200 OK\r\nETag: {etag}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len(),
                        )
                    };
                    let _ = stream.write_all(resp.as_bytes());
                }
            });
            TestOrigin {
                base: format!("http://127.0.0.1:{port}"),
                bodies,
                state,
            }
        }
        fn url(&self, path: &str) -> String {
            format!("{}{path}", self.base)
        }
        fn bodies(&self) -> usize {
            self.bodies.load(Ordering::SeqCst)
        }
        fn set_status(&self, status: u16) {
            self.state.lock().unwrap().2 = status;
        }
    }

    /// Options with a cache and the default freshness window (so a just-written
    /// entry is reused without asking the origin).
    fn opts_with_cache(dir: &Path) -> ResolveOpts {
        ResolveOpts {
            cache_dir: Some(dir.to_path_buf()),
            refresh: false,
            ttl_secs: DEFAULT_TTL_SECS,
        }
    }

    /// Options that revalidate on every resolution.
    fn always_revalidate(dir: &Path) -> ResolveOpts {
        ResolveOpts {
            ttl_secs: 0,
            ..opts_with_cache(dir)
        }
    }

    fn no_cache() -> ResolveOpts {
        ResolveOpts {
            cache_dir: None,
            refresh: false,
            ttl_secs: DEFAULT_TTL_SECS,
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

    fn plugin_yaml(version: &str, rule: &str) -> String {
        format!("version: {version}\nrules:\n  - {{name: {rule}, description: d}}\n")
    }

    fn req(s: &str) -> Option<VersionReq> {
        Some(VersionReq::parse(s).unwrap())
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
    fn an_entry_is_keyed_by_the_resolved_version_not_the_pin() {
        let dir = tempdir().unwrap();
        // Two documents differing only in a non-breaking bump are two entries.
        let a = entry_paths(dir.path(), &Version::parse("1.2").unwrap());
        let b = entry_paths(dir.path(), &Version::parse("1.4").unwrap());
        assert_ne!(a.0, b.0);
        assert_eq!(a.0.file_name().unwrap(), "v1.2.yml");
        assert_eq!(a.1.file_name().unwrap(), "v1.2.json");
        // Different URLs stay in different subdirectories.
        assert_ne!(
            url_dir(dir.path(), "https://x/p.yml"),
            url_dir(dir.path(), "https://y/p.yml")
        );
    }

    #[test]
    fn file_plugin_is_fetched_validated_and_cached() {
        let dir = tempdir().unwrap();
        let plugin = dir.path().join("plug.yml");
        std::fs::write(&plugin, plugin_yaml("1", "r")).unwrap();
        let cache = tempdir().unwrap();
        let opts = opts_with_cache(cache.path());
        let url = file_url(&plugin);

        let res = load_remote(&url, &req("1"), &opts).unwrap();
        assert!(res.text.contains("name: r"));
        assert_eq!(res.info.origin, Origin::Fetched);
        assert_eq!(res.info.version.as_deref(), Some("1"));

        // Within the freshness window the entry is reused with no origin read:
        // mutating the source changes nothing.
        std::fs::write(&plugin, plugin_yaml("1", "changed")).unwrap();
        let again = load_remote(&url, &req("1"), &opts).unwrap();
        assert!(again.text.contains("name: r"), "got: {}", again.text);
        assert_eq!(again.info.origin, Origin::Cache);

        // refresh: true reaches the origin and replaces what the cache holds.
        let refreshed = load_remote(
            &url,
            &req("1"),
            &ResolveOpts {
                refresh: true,
                ..opts.clone()
            },
        )
        .unwrap();
        assert!(refreshed.text.contains("name: changed"));
        assert_eq!(refreshed.info.origin, Origin::Fetched);
        // …and the replacement is what a later cached read sees.
        let after = load_remote(&url, &req("1"), &opts).unwrap();
        assert!(after.text.contains("name: changed"));
        assert_eq!(after.info.origin, Origin::Cache);
    }

    #[test]
    fn a_non_breaking_bump_at_the_origin_is_adopted_under_the_same_pin() {
        let dir = tempdir().unwrap();
        let plugin = dir.path().join("plug.yml");
        std::fs::write(&plugin, plugin_yaml("1.2", "old_rule")).unwrap();
        let cache = tempdir().unwrap();
        let opts = always_revalidate(cache.path());
        let url = file_url(&plugin);

        let first = load_remote(&url, &req("1"), &opts).unwrap();
        assert_eq!(first.info.version.as_deref(), Some("1.2"));

        // The plugin author publishes 1.4 — the consumer changes nothing.
        std::fs::write(&plugin, plugin_yaml("1.4", "new_rule")).unwrap();
        let second = load_remote(&url, &req("1"), &opts).unwrap();
        assert!(second.text.contains("new_rule"), "got: {}", second.text);
        assert_eq!(second.info.version.as_deref(), Some("1.4"));

        // Both versions are now cached as separate entries, and the pin resolves
        // to the newer one even without reaching the origin again.
        let cached = load_remote(&url, &req("1"), &opts_with_cache(cache.path())).unwrap();
        assert_eq!(cached.info.origin, Origin::Cache);
        assert_eq!(cached.info.version.as_deref(), Some("1.4"));
        let listed = list_cached(cache.path()).unwrap();
        assert_eq!(listed.len(), 2, "{listed:?}");
        assert_eq!(listed[0].version, "1.2");
        assert_eq!(listed[0].newer.as_deref(), Some("1.4"));
        assert_eq!(listed[1].version, "1.4");
        assert_eq!(listed[1].newer, None);
        // A more specific pin still resolves to its own range.
        let pinned = load_remote(&url, &req("1.2"), &opts_with_cache(cache.path())).unwrap();
        assert_eq!(pinned.info.version.as_deref(), Some("1.2"));
    }

    #[test]
    fn an_unreachable_origin_keeps_the_run_working_from_cache() {
        let dir = tempdir().unwrap();
        let plugin = dir.path().join("plug.yml");
        std::fs::write(&plugin, plugin_yaml("1", "r")).unwrap();
        let cache = tempdir().unwrap();
        let opts = always_revalidate(cache.path());
        let url = file_url(&plugin);
        load_remote(&url, &req("1"), &opts).unwrap();

        // The origin goes away; revalidation cannot be made.
        std::fs::remove_file(&plugin).unwrap();
        let offline = load_remote(&url, &req("1"), &opts).unwrap();
        assert!(offline.text.contains("name: r"));
        assert_eq!(offline.info.origin, Origin::Cache);

        // With nothing cached, the same unreachable origin is still an error.
        let empty = tempdir().unwrap();
        let err = load_remote(&url, &req("1"), &always_revalidate(empty.path())).unwrap_err();
        assert!(matches!(err, Error::PluginFetch { .. }));
    }

    #[test]
    fn an_origin_that_left_the_pinned_range_keeps_the_pinned_entry() {
        let dir = tempdir().unwrap();
        let plugin = dir.path().join("plug.yml");
        std::fs::write(&plugin, plugin_yaml("1", "r")).unwrap();
        let cache = tempdir().unwrap();
        let opts = always_revalidate(cache.path());
        let url = file_url(&plugin);
        load_remote(&url, &req("1"), &opts).unwrap();

        // The origin now publishes 2.0 at the same URL: the `@1` consumer keeps
        // the 1.x it pinned rather than failing a run that was working.
        std::fs::write(&plugin, plugin_yaml("2", "breaking")).unwrap();
        let kept = load_remote(&url, &req("1"), &opts).unwrap();
        assert!(kept.text.contains("name: r"), "got: {}", kept.text);
        assert_eq!(kept.info.origin, Origin::Cache);

        // An unreadable document at the origin is treated the same way.
        std::fs::write(&plugin, "version: : :\n  - oops\n").unwrap();
        let kept = load_remote(&url, &req("1"), &opts).unwrap();
        assert!(kept.text.contains("name: r"));
    }

    #[test]
    fn a_previous_layout_entry_is_never_read_as_a_resolved_version() {
        let dir = tempdir().unwrap();
        let plugin = dir.path().join("plug.yml");
        std::fs::write(&plugin, plugin_yaml("1.4", "current")).unwrap();
        let cache = tempdir().unwrap();
        let url = file_url(&plugin);
        // A pin-named file from the previous cache layout, with no metadata.
        let sub = url_dir(cache.path(), &url);
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("1.yml"), plugin_yaml("1.0", "stale")).unwrap();

        let res = load_remote(&url, &req("1"), &opts_with_cache(cache.path())).unwrap();
        assert!(res.text.contains("current"), "got: {}", res.text);
        assert_eq!(res.info.version.as_deref(), Some("1.4"));
        // It is also invisible to the reporting verb.
        let listed = list_cached(cache.path()).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].version, "1.4");
        // …and the stale file is left alone by a clear (it is not ours to read).
        assert_eq!(clear_cached(cache.path()).unwrap(), 1);
        assert!(sub.join("1.yml").is_file());
    }

    #[test]
    fn unreadable_or_foreign_cache_files_are_skipped() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("notjson.txt"), "x").unwrap();
        std::fs::write(dir.path().join("bad.json"), "{not json").unwrap();
        // Right shape, wrong schema version.
        std::fs::write(
            dir.path().join("v9.json"),
            r#"{"schema":99,"url":"u","pin":"1","version":"9","fetched_at":0}"#,
        )
        .unwrap();
        // Current schema, but the document beside it is missing.
        std::fs::write(
            dir.path().join("v8.json"),
            r#"{"schema":1,"url":"u","pin":"1","version":"8","fetched_at":0}"#,
        )
        .unwrap();
        // Current schema, unparseable version.
        std::fs::write(
            dir.path().join("vx.json"),
            r#"{"schema":1,"url":"u","pin":"1","version":"x","fetched_at":0}"#,
        )
        .unwrap();
        assert!(read_entries(dir.path(), "u").is_empty());
        // Metadata for another URL never answers for this one (hash collision).
        std::fs::write(dir.path().join("v7.yml"), "version: 7\n").unwrap();
        std::fs::write(
            dir.path().join("v7.json"),
            r#"{"schema":1,"url":"other","pin":"1","version":"7","fetched_at":0}"#,
        )
        .unwrap();
        assert!(read_entries(dir.path(), "u").is_empty());
        assert_eq!(read_entries(dir.path(), "other").len(), 1);
        // A missing directory is simply an empty cache.
        assert!(read_entries(&dir.path().join("nope"), "u").is_empty());
        assert!(list_cached(&dir.path().join("nope")).unwrap().is_empty());
        assert_eq!(clear_cached(&dir.path().join("nope")).unwrap(), 0);
    }

    #[test]
    fn clear_removes_entries_and_prunes_the_directory() {
        let dir = tempdir().unwrap();
        let plugin = dir.path().join("plug.yml");
        std::fs::write(&plugin, plugin_yaml("1", "r")).unwrap();
        let cache = tempdir().unwrap();
        let url = file_url(&plugin);
        load_remote(&url, &req("1"), &opts_with_cache(cache.path())).unwrap();
        assert_eq!(list_cached(cache.path()).unwrap().len(), 1);

        assert_eq!(clear_cached(cache.path()).unwrap(), 1);
        assert!(list_cached(cache.path()).unwrap().is_empty());
        assert!(!url_dir(cache.path(), &url).exists());
        // Clearing an already-empty cache is a no-op, not an error.
        assert_eq!(clear_cached(cache.path()).unwrap(), 0);
    }

    #[test]
    fn a_stale_entry_is_revalidated_and_its_clock_restarts() {
        let dir = tempdir().unwrap();
        let plugin = dir.path().join("plug.yml");
        std::fs::write(&plugin, plugin_yaml("1", "r")).unwrap();
        let cache = tempdir().unwrap();
        let url = file_url(&plugin);
        load_remote(&url, &req("1"), &opts_with_cache(cache.path())).unwrap();

        // Backdate the entry past the default window, then resolve again: the
        // origin is consulted and the confirmation restarts the entry's clock.
        let sub = url_dir(cache.path(), &url);
        let entries = read_entries(&sub, &url);
        let mut meta = entries[0].meta.clone();
        meta.fetched_at = 0;
        write_meta(&entries[0].meta_path, &meta).unwrap();
        assert!(is_stale(&meta, DEFAULT_TTL_SECS, now_secs()));

        let res = load_remote(&url, &req("1"), &opts_with_cache(cache.path())).unwrap();
        assert!(res.text.contains("name: r"));
        let after = read_entries(&sub, &url);
        assert_eq!(after.len(), 1, "an unchanged version stays one entry");
        assert!(
            !is_stale(&after[0].meta, DEFAULT_TTL_SECS, now_secs()),
            "revalidation must restart the clock: {:?}",
            after[0].meta
        );
    }

    #[test]
    fn an_http_origin_answering_not_modified_reuses_the_entry() {
        let origin = TestOrigin::serve(&plugin_yaml("1.2", "remote_rule"), "\"v1\"");
        let cache = tempdir().unwrap();
        let url = origin.url("/rules.yml");
        // Every resolution revalidates, so the second one is a conditional GET.
        let opts = always_revalidate(cache.path());

        let first = load_remote(&url, &req("1"), &opts).unwrap();
        assert_eq!(first.info.origin, Origin::Fetched);
        assert_eq!(origin.bodies(), 1);
        let sub = url_dir(cache.path(), &url);
        // The origin's validator was recorded beside the entry, and backdating
        // proves the 304 (not the window) is what refreshes the timestamp.
        let entries = read_entries(&sub, &url);
        assert_eq!(entries[0].meta.etag.as_deref(), Some("\"v1\""));
        let mut meta = entries[0].meta.clone();
        meta.fetched_at = 0;
        write_meta(&entries[0].meta_path, &meta).unwrap();

        let second = load_remote(&url, &req("1"), &opts).unwrap();
        assert_eq!(second.info.origin, Origin::Cache);
        assert!(second.text.contains("remote_rule"));
        assert_eq!(
            origin.bodies(),
            1,
            "an unchanged document must not be re-downloaded"
        );
        assert!(read_entries(&sub, &url)[0].meta.fetched_at > 0);
    }

    #[test]
    fn a_non_2xx_status_is_a_fetch_error() {
        let origin = TestOrigin::serve("", "\"v1\"");
        origin.set_status(500);
        let err = load_remote(&origin.url("/rules.yml"), &None, &no_cache()).unwrap_err();
        assert!(err.to_string().contains("HTTP status 500"), "got: {err}");
    }

    #[test]
    fn is_stale_tolerates_clock_skew() {
        let meta = CacheMeta {
            schema: CACHE_SCHEMA,
            url: "u".into(),
            pin: Some("1".into()),
            version: "1".into(),
            fetched_at: 100,
            etag: None,
            last_modified: None,
        };
        assert!(!is_stale(&meta, 10, 105));
        assert!(is_stale(&meta, 10, 110));
        // A future timestamp counts as fresh rather than underflowing.
        assert!(!is_stale(&meta, 10, 50));
        // A zero window always revalidates.
        assert!(is_stale(&meta, 0, 100));
    }

    #[test]
    fn unpinned_plugin_is_not_cached() {
        let dir = tempdir().unwrap();
        let plugin = dir.path().join("plug.yml");
        std::fs::write(&plugin, "rules: []\n").unwrap();
        let cache = tempdir().unwrap();
        let opts = opts_with_cache(cache.path());
        let res = load_remote(&file_url(&plugin), &None, &opts).unwrap();
        assert_eq!(res.info.version, None);
        assert_eq!(res.info.pin, None);
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
        let err = load_remote(&file_url(&v2), &req("1"), &opts).unwrap_err();
        assert!(matches!(err, Error::PluginVersionMismatch { .. }));

        let none = dir.path().join("none.yml");
        std::fs::write(&none, "rules: []\n").unwrap();
        let err = load_remote(&file_url(&none), &req("1"), &opts).unwrap_err();
        assert!(matches!(err, Error::PluginMissingVersion { .. }));
    }

    #[test]
    fn missing_file_url_is_a_fetch_error() {
        let err = load_remote("file:///no/such/plugin.yml", &None, &no_cache()).unwrap_err();
        assert!(matches!(err, Error::PluginFetch { .. }));
    }

    #[test]
    fn bundled_url_resolves_offline_and_validates_pin() {
        // Resolves from the embedded copy — no network, no cache.
        let res = load_remote(assets::CONFIG_LINT_URL, &req("1"), &no_cache()).unwrap();
        assert!(res.text.contains("name_describes_what_the_rule_checks"));
        assert_eq!(res.info.origin, Origin::Bundled);
        assert!(res.info.version.is_some());
        assert!(res.info.to_human().contains("from bundled"));
        // A pin the embedded version can't satisfy still errors.
        let err = load_remote(assets::CONFIG_LINT_URL, &req("2"), &no_cache()).unwrap_err();
        assert!(matches!(err, Error::PluginVersionMismatch { .. }));
    }

    #[test]
    fn resolution_renders_a_human_line() {
        let res = PluginResolution {
            url: "https://x/p.yml".into(),
            pin: Some("1".into()),
            version: Some("1.4".into()),
            origin: Origin::Cache,
        };
        assert_eq!(
            res.to_human(),
            "https://x/p.yml@1 -> version 1.4 (from cache)"
        );
        let unpinned = PluginResolution {
            url: "https://x/p.yml".into(),
            pin: None,
            version: None,
            origin: Origin::Fetched,
        };
        assert_eq!(
            unpinned.to_human(),
            "https://x/p.yml -> no declared version (from fetched)"
        );
    }

    #[test]
    fn http_connection_failure_is_a_fetch_error() {
        // Port 1 refuses immediately, exercising the transport-error branch of
        // the HTTPS client without any external network.
        let err = load_remote("http://127.0.0.1:1/nope.yml", &None, &no_cache()).unwrap_err();
        assert!(matches!(err, Error::PluginFetch { .. }));
    }

    #[test]
    fn unparseable_plugin_version_is_a_fetch_error() {
        let dir = tempdir().unwrap();
        let plugin = dir.path().join("bad.yml");
        // Invalid YAML so the version probe fails to parse.
        std::fs::write(&plugin, "version: : :\n  - oops\n").unwrap();
        let err = load_remote(&file_url(&plugin), &req("1"), &no_cache()).unwrap_err();
        assert!(matches!(err, Error::PluginFetch { .. }));
    }

    /// Set an env var for the duration of `f`, restoring the previous value.
    fn with_var<T>(name: &str, value: Option<&str>, f: impl FnOnce() -> T) -> T {
        let prev = std::env::var_os(name);
        match value {
            Some(v) => std::env::set_var(name, v),
            None => std::env::remove_var(name),
        }
        let out = f();
        match prev {
            Some(v) => std::env::set_var(name, v),
            None => std::env::remove_var(name),
        }
        out
    }

    #[test]
    fn from_env_reads_cache_dir_refresh_and_ttl() {
        with_var(CACHE_DIR_VAR, Some("/tmp/llmlint-cache-test"), || {
            with_var(TTL_VAR, None, || {
                let opts = ResolveOpts::from_env().unwrap();
                assert_eq!(
                    opts.cache_dir,
                    Some(PathBuf::from("/tmp/llmlint-cache-test"))
                );
                assert!(!opts.refresh);
                assert_eq!(opts.ttl_secs, DEFAULT_TTL_SECS);
            });
            with_var(TTL_VAR, Some("0"), || {
                assert_eq!(ResolveOpts::from_env().unwrap().ttl_secs, 0);
            });
            with_var(TTL_VAR, Some(" 90 "), || {
                assert_eq!(ResolveOpts::from_env().unwrap().ttl_secs, 90);
            });
            // A malformed window is located to the variable, never ignored.
            with_var(TTL_VAR, Some("soon"), || {
                let err = ResolveOpts::from_env().unwrap_err();
                let msg = err.to_string();
                assert!(msg.contains(TTL_VAR), "got: {msg}");
                assert!(msg.contains("whole number of seconds"), "got: {msg}");
            });
        });
    }
}
