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
use crate::io::{assets, env};

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

/// The `schema` field of [`CacheMeta`]: the one shape this release can read.
/// Deserializing rejects any other number, so metadata from a future (or
/// previous) layout cannot inhabit the type at all — there is no
/// "wrong-schema `CacheMeta`" to check for afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CacheSchema;

impl Serialize for CacheSchema {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_u32(CACHE_SCHEMA)
    }
}

impl<'de> Deserialize<'de> for CacheSchema {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let n = u32::deserialize(d)?;
        if n == CACHE_SCHEMA {
            Ok(CacheSchema)
        } else {
            Err(serde::de::Error::custom(format!(
                "cache entry schema {n}, but this llmlint reads schema {CACHE_SCHEMA}"
            )))
        }
    }
}

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
    /// dir), [`REFRESH_VAR`], and [`TTL_VAR`]. A malformed window or switch is an
    /// exit-2 [`Error::Env`] located to the variable — validated at the boundary,
    /// never silently ignored, and the switch reads the same bool grammar every
    /// other `LLMLINT_*` setting does.
    pub fn from_env() -> Result<Self> {
        let cache_dir = std::env::var_os(CACHE_DIR_VAR)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .or_else(default_cache_dir);
        let refresh = match non_empty_var(REFRESH_VAR) {
            Some(v) => env::parse_bool(REFRESH_VAR, &v)?,
            None => false,
        };
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
    /// What was asked for and what answered.
    pub resolved: Resolved,
    pub origin: Origin,
}

/// What a `plugins:` URL resolved to. The pin and the version that satisfied it
/// are one value rather than two independently-optional fields, because a pinned
/// plugin whose document declares no version in the pin's range never resolves
/// at all — resolution raises that as an error before building this value, so
/// "pinned, but nothing satisfied it" is not a state this type can hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// Requested under a pin, and answered by a document declaring `version`.
    Pinned { pin: VersionReq, version: Version },
    /// Requested with no pin, which a document need not declare a version for.
    Unpinned { version: Option<Version> },
}

impl Resolved {
    /// The pin this URL was requested under, if any.
    pub fn pin(&self) -> Option<&VersionReq> {
        match self {
            Resolved::Pinned { pin, .. } => Some(pin),
            Resolved::Unpinned { .. } => None,
        }
    }

    /// The version the resolved document declares, if it declares one.
    pub fn version(&self) -> Option<&Version> {
        match self {
            Resolved::Pinned { version, .. } => Some(version),
            Resolved::Unpinned { version } => version.as_ref(),
        }
    }
}

impl PluginResolution {
    /// One human line: `url@pin -> version 1.4 (from cache)`.
    pub fn to_human(&self) -> String {
        let pin = match self.resolved.pin() {
            Some(p) => format!("@{p}"),
            None => String::new(),
        };
        let version = match self.resolved.version() {
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
        let resolved = resolve_version(url, req.as_ref(), content)?;
        return Ok(resolution(
            url,
            resolved,
            content.to_string(),
            Origin::Bundled,
        ));
    }

    match (req, &opts.cache_dir) {
        // Only a pinned fetch has a stable identity worth caching.
        (Some(r), Some(dir)) => load_cached(url, r, &url_dir(dir, url), opts),
        _ => {
            let body = fetch_body(url)?;
            let resolved = resolve_version(url, req.as_ref(), &body.text)?;
            Ok(resolution(url, resolved, body.text, Origin::Fetched))
        }
    }
}

/// The cached half of [`load_remote`]: pick the newest entry satisfying the pin,
/// revalidate it when stale, and fall back to fetching when the cache holds
/// nothing usable (or `--refresh` forces it).
fn load_cached(url: &str, req: &VersionReq, dir: &Path, opts: &ResolveOpts) -> Result<Resolution> {
    let entries = if opts.refresh {
        Vec::new()
    } else {
        read_entries(dir, url)
    };
    if let Some((entry, text)) = newest_usable(&entries, req, url) {
        let cached = |text: String| {
            let resolved = Resolved::Pinned {
                pin: req.clone(),
                version: entry.version().clone(),
            };
            resolution(url, resolved, text, Origin::Cache)
        };
        let now = now_confirmed();
        if !is_stale(&entry.meta, opts.ttl_secs, now) {
            return Ok(cached(text));
        }
        match revalidate(url, &entry.meta) {
            // Unchanged: the origin confirmed the entry, so its clock restarts.
            Ok(None) => {
                touch(entry, now)?;
                return Ok(cached(text));
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
                        let resolved = Resolved::Pinned {
                            pin: req.clone(),
                            version: v,
                        };
                        return Ok(resolution(url, resolved, body.text, Origin::Fetched));
                    }
                }
                return Ok(cached(text));
            }
            // Offline, a transport failure, a refusal: a cache is a speed-up, not
            // a network dependency, so the run keeps working from what it has.
            // The timestamp is deliberately *not* refreshed, so the next run
            // tries the origin again.
            Err(_) => return Ok(cached(text)),
        }
    }

    let body = fetch_body(url)?;
    let resolved = resolve_version(url, Some(req), &body.text)?;
    if let Some(v) = resolved.version() {
        store(dir, url, req, v, &body, now_confirmed())?;
    }
    Ok(resolution(url, resolved, body.text, Origin::Fetched))
}

fn resolution(url: &str, resolved: Resolved, text: String, origin: Origin) -> Resolution {
    Resolution {
        text,
        info: PluginResolution {
            url: url.to_string(),
            resolved,
            origin,
        },
    }
}

/// The newest cached entry satisfying `req` whose **document** still backs its
/// metadata, together with that document's text.
///
/// The cache directory is an input like any other — a file on disk, editable,
/// truncatable, and written by an older release — so the sidecar is not taken
/// on trust: an entry counts only when the document it names parses and still
/// declares the version the metadata claims (which, having been matched against
/// the pin, is what makes the text safe to hand to the caller). Anything else is
/// passed over for the next-newest match, and an empty result simply fetches —
/// never an error, and never a document the pin did not ask for.
fn newest_usable<'a>(
    entries: &'a [Entry],
    req: &VersionReq,
    url: &str,
) -> Option<(&'a Entry, String)> {
    let mut matching: Vec<&Entry> = entries
        .iter()
        .filter(|e| req.matches(e.version()))
        .collect();
    matching.sort_by(|a, b| b.version().cmp(a.version()));
    matching
        .into_iter()
        .find_map(|e| load_entry(e, url).map(|text| (e, text)))
}

/// A cache entry's document, if it still declares the version its metadata does.
fn load_entry(entry: &Entry, url: &str) -> Option<String> {
    let text = std::fs::read_to_string(&entry.data).ok()?;
    let declared = declared_version(url, &text).ok()??;
    (&declared == entry.version()).then_some(text)
}

/// A cache entry's confirmation time as persisted: seconds since the Unix epoch,
/// and only a count some clock can hold. Deserializing rejects anything else, so
/// a hand-edited number cannot inhabit [`CacheMeta`] at all and the addition that
/// would panic on it is unreachable rather than guarded. Serde-transparent: on
/// the wire it is the bare number, exactly as the golden holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ConfirmedAt(u64);

impl ConfirmedAt {
    /// The confirmation time `secs` names, or `None` when no `SystemTime` can
    /// hold it.
    pub fn new(secs: u64) -> Option<Self> {
        Self::to_instant(secs).map(|_| ConfirmedAt(secs))
    }

    /// Seconds since the Unix epoch, for arithmetic against the freshness window.
    pub fn secs(self) -> u64 {
        self.0
    }

    /// The instant this names. Total by construction — the value was checked
    /// before the type existed — so there is no failure for a caller to handle.
    pub fn instant(self) -> SystemTime {
        Self::to_instant(self.0).unwrap_or(UNIX_EPOCH)
    }

    fn to_instant(secs: u64) -> Option<SystemTime> {
        UNIX_EPOCH.checked_add(std::time::Duration::from_secs(secs))
    }
}

impl Serialize for ConfirmedAt {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_u64(self.0)
    }
}

impl<'de> Deserialize<'de> for ConfirmedAt {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let secs = u64::deserialize(d)?;
        ConfirmedAt::new(secs).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "confirmation time {secs} is past any representable instant"
            ))
        })
    }
}

/// The longest revalidation validator worth persisting. Real `ETag`s and HTTP
/// dates are tens of bytes; the bound keeps a bloated cache file from being sent
/// back to an origin verbatim.
const MAX_VALIDATOR_LEN: usize = 512;

/// A revalidation validator as persisted (`ETag` / `Last-Modified`): a legal,
/// bounded HTTP header value, because going back out as a request header is the
/// only thing it is for. Deserializing rejects anything else — a control
/// character would split the request line — so an edited cache file cannot put
/// one into a request. Serde-transparent: on the wire it is the bare string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderValue(String);

impl HeaderValue {
    /// The validator `v` names, or `None` when it is not one a request could
    /// carry (empty, over-long, or holding a byte outside visible ASCII).
    pub fn new(v: impl Into<String>) -> Option<Self> {
        let v = v.into();
        let legal = !v.is_empty()
            && v.len() <= MAX_VALIDATOR_LEN
            && v.bytes().all(|b| b == b'\t' || (0x20..0x7f).contains(&b));
        legal.then_some(HeaderValue(v))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Serialize for HeaderValue {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for HeaderValue {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        HeaderValue::new(raw.clone()).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "{raw:?} is not a legal HTTP header value (visible ASCII, 1 to \
                 {MAX_VALIDATOR_LEN} bytes)"
            ))
        })
    }
}

/// Metadata stored beside each cache entry. Written as `v<version>.json` next to
/// the `v<version>.yml` document it describes; an entry without readable
/// metadata of the current [`CACHE_SCHEMA`] is not an entry. Every field is the
/// value it means, so metadata this release did not write fails to deserialize
/// rather than arriving as something a later check has to catch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheMeta {
    /// On-disk shape version — only the one this release reads.
    pub schema: CacheSchema,
    /// The URL this document was fetched from.
    pub url: String,
    /// The pin the fetch was made under (`@1`), which is a *range*, not this
    /// entry's identity. Only a pinned fetch is ever cached, so an entry always
    /// records one; metadata without a pin (or naming a malformed one) is not
    /// metadata this release wrote, and is passed over.
    pub pin: VersionReq,
    /// The version the fetched document declares — the entry's key.
    pub version: Version,
    /// When the origin last **confirmed** this entry. A `304` refreshes it
    /// without downloading anything, so this is a confirmation time, not a
    /// download time.
    pub confirmed_at: ConfirmedAt,
    /// The `ETag` the response carried, for the next conditional request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<HeaderValue>,
    /// The `Last-Modified` the response carried.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<HeaderValue>,
}

/// One cache entry: its metadata and the paths of the two files that hold it.
#[derive(Debug, Clone)]
struct Entry {
    meta: CacheMeta,
    data: PathBuf,
    meta_path: PathBuf,
}

impl Entry {
    /// The version this entry is keyed by.
    fn version(&self) -> &Version {
        &self.meta.version
    }
}

/// A cached plugin as [`list_cached`] reports it. This is the single source of
/// the reporting verb's machine-readable shape too: `llmlint plugins --format
/// json` serializes these values rather than restating the fields, so the two
/// cannot drift.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CachedPlugin {
    pub url: String,
    /// The pin recorded in the entry's metadata.
    pub pin: VersionReq,
    pub version: Version,
    /// When the origin last confirmed this entry; serialized (and printed) as
    /// the RFC 3339 UTC stamp. An instant rather than a raw count, so the
    /// metadata's arbitrary number is converted once — where it is validated —
    /// instead of at each rendering.
    #[serde(serialize_with = "as_utc_stamp")]
    pub confirmed_at: SystemTime,
    /// The newest *other* cached version of the same URL that also satisfies
    /// this entry's pin, if any — i.e. this entry is no longer what the pin
    /// resolves to.
    pub newer: Option<Version>,
}

impl CachedPlugin {
    /// When the origin last confirmed this entry, as the same RFC 3339 UTC stamp
    /// the history records use. The human report and the JSON one share it.
    pub fn confirmed_at_utc(&self) -> String {
        crate::io::history::format_timestamp(self.confirmed_at)
    }
}

fn as_utc_stamp<S: serde::Serializer>(
    at: &SystemTime,
    s: S,
) -> std::result::Result<S::Ok, S::Error> {
    s.serialize_str(&crate::io::history::format_timestamp(*at))
}

/// The current confirmation time. A real clock is always representable, and a
/// clock read that fails clamps to the epoch rather than failing a run.
fn now_confirmed() -> ConfirmedAt {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    ConfirmedAt::new(secs).unwrap_or(ConfirmedAt(0))
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
        // Every persisted value is validated by this one parse: a timestamp no
        // clock can hold, a validator no request could carry, a malformed
        // version or pin, a schema this release does not read.
        let Ok(meta) = serde_json::from_str::<CacheMeta>(&text) else {
            continue;
        };
        if !url.is_empty() && meta.url != url {
            continue;
        }
        // The sidecar must be the one this entry's version implies. Without
        // that, any `.json` declaring a version would adopt the document keyed
        // by it — a stray or forged file aliasing an entry it does not name, and
        // a `touch` writing back to the wrong sidecar.
        let (data, expected_meta) = entry_paths(dir, &meta.version);
        if meta_path != expected_meta || !data.is_file() {
            continue;
        }
        out.push(Entry {
            meta,
            data,
            meta_path,
        });
    }
    out
}

/// Whether an entry is older than the freshness window. A `confirmed_at` in the
/// future (clock skew) counts as fresh.
fn is_stale(meta: &CacheMeta, ttl_secs: u64, now: ConfirmedAt) -> bool {
    now.secs().saturating_sub(meta.confirmed_at.secs()) >= ttl_secs
}

/// Write an entry (document + metadata) under its **resolved** version.
fn store(
    dir: &Path,
    url: &str,
    req: &VersionReq,
    version: &Version,
    body: &Body,
    now: ConfirmedAt,
) -> Result<()> {
    std::fs::create_dir_all(dir)
        .map_err(|e| io_err(format!("creating plugin cache dir {}", dir.display()), e))?;
    let (data, meta_path) = entry_paths(dir, version);
    std::fs::write(&data, &body.text)
        .map_err(|e| io_err(format!("writing plugin cache {}", data.display()), e))?;
    write_meta(
        &meta_path,
        &CacheMeta {
            schema: CacheSchema,
            url: url.to_string(),
            pin: req.clone(),
            version: version.clone(),
            confirmed_at: now,
            etag: body.etag.clone(),
            last_modified: body.last_modified.clone(),
        },
    )
}

/// Record that the origin confirmed an entry is still current.
fn touch(entry: &Entry, now: ConfirmedAt) -> Result<()> {
    let mut meta = entry.meta.clone();
    meta.confirmed_at = now;
    write_meta(&entry.meta_path, &meta)
}

fn write_meta(path: &Path, meta: &CacheMeta) -> Result<()> {
    let json = serde_json::to_string_pretty(meta).map_err(|e| Error::Io(e.to_string()))?;
    std::fs::write(path, format!("{json}\n"))
        .map_err(|e| io_err(format!("writing plugin cache {}", path.display()), e))
}

/// The per-URL subdirectories of a cache directory, for the two reporting verbs.
///
/// A cache directory is created on first store, so one that is not there yet is
/// an empty cache. Every *other* read failure — a file where a directory should
/// be, a directory this process may not read — is reported: those verbs answer a
/// question about a directory the caller named, and "no plugins cached" is the
/// wrong answer to a `--dir` that could not be read. Resolution
/// ([`read_entries`]) still swallows both, because a run must not fail over a
/// cache it can simply refetch past.
fn cache_subdirs(dir: &Path) -> Result<Vec<PathBuf>> {
    match std::fs::read_dir(dir) {
        Ok(rd) => Ok(rd.flatten().map(|e| e.path()).collect()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(io_err(format!("reading plugin cache {}", dir.display()), e)),
    }
}

/// Every cached plugin entry under `dir`, sorted by URL then version, each
/// carrying whether a newer cached version satisfying its pin is known.
pub fn list_cached(dir: &Path) -> Result<Vec<CachedPlugin>> {
    let mut entries: Vec<Entry> = Vec::new();
    for sub in cache_subdirs(dir)? {
        // An empty `url` filter accepts any entry: listing has no URL in hand,
        // and each entry's metadata names its own.
        entries.extend(read_entries(&sub, ""));
    }
    entries.sort_by(|a, b| {
        a.meta
            .url
            .cmp(&b.meta.url)
            .then_with(|| a.version().cmp(b.version()))
    });
    let mut out = Vec::with_capacity(entries.len());
    for e in &entries {
        let newer = entries
            .iter()
            .filter(|o| {
                o.meta.url == e.meta.url
                    && o.version() > e.version()
                    && e.meta.pin.matches(o.version())
            })
            .map(|o| o.version().clone())
            .max();
        out.push(CachedPlugin {
            url: e.meta.url.clone(),
            pin: e.meta.pin.clone(),
            version: e.version().clone(),
            confirmed_at: e.meta.confirmed_at.instant(),
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
    let mut removed = 0;
    for sub in cache_subdirs(dir)? {
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

/// A fetched document plus whatever revalidation validators the origin supplied.
#[derive(Debug, Clone, Default)]
struct Body {
    text: String,
    etag: Option<HeaderValue>,
    last_modified: Option<HeaderValue>,
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

/// Check a fetched plugin's declared version against the requested pin, and
/// yield the [`Resolved`] it establishes. An unpinned plugin accepts any (or no)
/// declared version; a pinned one that declares none, or one outside the range,
/// is the error raised here — which is what makes `Resolved::Pinned` the only
/// answer a successful pinned call can give.
fn resolve_version(url: &str, req: Option<&VersionReq>, text: &str) -> Result<Resolved> {
    let declared = declared_version(url, text)?;
    let Some(req) = req else {
        return Ok(Resolved::Unpinned { version: declared });
    };
    match declared {
        Some(version) if req.matches(&version) => Ok(Resolved::Pinned {
            pin: req.clone(),
            version,
        }),
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
            req = req.header("If-None-Match", etag.as_str());
        }
        if let Some(lm) = &meta.last_modified {
            req = req.header("If-Modified-Since", lm.as_str());
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
    // A validator is kept only if it is one a later conditional request could
    // carry, so nothing unusable is ever persisted.
    let header = |name: &str| {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .and_then(HeaderValue::new)
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

    /// The committed goldens for the on-disk metadata shape (`CACHE_SCHEMA` 1).
    /// They are byte-for-byte what [`write_meta`] produces, so asserting against
    /// them pins the field names, their order, the schema value, and which
    /// fields are omitted — see `tests/fixtures/plugin_cache/README.md`.
    const GOLDEN_V1: &str = include_str!("../../tests/fixtures/plugin_cache/v1.json");
    const GOLDEN_V1_NO_VALIDATORS: &str =
        include_str!("../../tests/fixtures/plugin_cache/v1-no-validators.json");

    const GOLDEN_URL: &str = "https://example.com/org-rules.yml";
    const GOLDEN_ETAG: &str = "\"a1b2c3\"";
    const GOLDEN_LAST_MODIFIED: &str = "Tue, 01 Sep 2026 09:12:44 GMT";
    const GOLDEN_CONFIRMED_AT: u64 = 1_700_000_000;

    fn plugin_yaml(version: &str, rule: &str) -> String {
        format!("version: {version}\nrules:\n  - {{name: {rule}, description: d}}\n")
    }

    fn req(s: &str) -> Option<VersionReq> {
        Some(VersionReq::parse(s).unwrap())
    }

    fn ver(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    fn validator(s: &str) -> HeaderValue {
        HeaderValue::new(s).unwrap()
    }

    fn at(secs: u64) -> ConfirmedAt {
        ConfirmedAt::new(secs).unwrap()
    }

    /// The resolved version as text, for a compact assertion.
    fn resolved(res: &Resolution) -> Option<String> {
        res.info.resolved.version().map(ToString::to_string)
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

    /// The freshness window's default is a user-facing number, so it is stated
    /// in the README and in `AGENTS.md` as well as here. Nothing generates those
    /// documents, so this is the gate that keeps the three from drifting: change
    /// the constant and the two prose statements move with it, or this fails.
    #[test]
    fn the_documented_freshness_default_matches_the_constant() {
        let default = DEFAULT_TTL_SECS.to_string();
        for (name, text) in [
            ("README.md", include_str!("../../README.md")),
            ("AGENTS.md", include_str!("../../AGENTS.md")),
        ] {
            let line = text
                .lines()
                .find(|l| l.contains(TTL_VAR) && l.contains("default"))
                .or_else(|| {
                    // The statement may wrap, so fall back to the line that
                    // carries the default itself.
                    text.lines()
                        .find(|l| l.contains("default") && l.contains(&default))
                })
                .unwrap_or_else(|| panic!("{name} states no {TTL_VAR} default"));
            assert!(
                line.contains(&default),
                "{name} documents a freshness default that is not {default}: {line}"
            );
        }
    }

    #[test]
    fn cache_metadata_v1_matches_the_committed_golden() {
        // The golden's own text fixes the persisted names, their order and the
        // schema value, so a renamed or reordered field fails here rather than
        // silently making every host's cache unreadable.
        let raw: serde_json::Value = serde_json::from_str(GOLDEN_V1).unwrap();
        let keys: Vec<&str> = raw
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            [
                "schema",
                "url",
                "pin",
                "version",
                "confirmed_at",
                "etag",
                "last_modified"
            ]
        );
        assert_eq!(raw["schema"], 1);

        // The real deserializer reads every field back as its parsed type.
        let meta: CacheMeta = serde_json::from_str(GOLDEN_V1).unwrap();
        assert_eq!(meta.schema, CacheSchema);
        assert_eq!(meta.url, GOLDEN_URL);
        assert_eq!(meta.pin, VersionReq::parse("1").unwrap());
        assert_eq!(meta.version, ver("1.4"));
        assert_eq!(meta.confirmed_at, at(GOLDEN_CONFIRMED_AT));
        assert_eq!(meta.etag, Some(validator(GOLDEN_ETAG)));
        assert_eq!(meta.last_modified, Some(validator(GOLDEN_LAST_MODIFIED)));

        // …and the real writer puts it back byte for byte, so the golden is the
        // file a host actually holds rather than an idealized rendering of it.
        let dir = tempdir().unwrap();
        let path = dir.path().join("v1.4.json");
        write_meta(&path, &meta).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), GOLDEN_V1);
    }

    #[test]
    fn a_persisted_timestamp_no_clock_can_hold_is_rejected_when_read() {
        // The value cannot inhabit `CacheMeta` at all, so nothing downstream has
        // to guard the arithmetic that would panic on it.
        let meta = GOLDEN_V1.replace("1700000000", &u64::MAX.to_string());
        let err = serde_json::from_str::<CacheMeta>(&meta).unwrap_err();
        assert!(
            err.to_string().contains("representable instant"),
            "got: {err}"
        );
        assert_eq!(ConfirmedAt::new(u64::MAX), None);

        // …and a value a clock can hold round-trips as the bare number the
        // golden holds, keeping the wire shape a plain integer.
        let ok = at(GOLDEN_CONFIRMED_AT);
        assert_eq!(ok.secs(), GOLDEN_CONFIRMED_AT);
        assert_eq!(
            serde_json::to_string(&ok).unwrap(),
            GOLDEN_CONFIRMED_AT.to_string()
        );
        assert_eq!(serde_json::from_str::<ConfirmedAt>("0").unwrap(), at(0));
        assert_eq!(
            ok.instant(),
            UNIX_EPOCH + std::time::Duration::from_secs(ok.secs())
        );
    }

    #[test]
    fn a_persisted_validator_no_request_could_carry_is_rejected_when_read() {
        // Each of these would either split the request line, send nothing, or
        // ship a bloated cache file back to the origin verbatim.
        for bad in [
            "\"v1\"\r\nX-Injected: 1",
            "\"v1\"\n",
            "tab\u{7f}del",
            "",
            &"x".repeat(MAX_VALIDATOR_LEN + 1),
        ] {
            assert_eq!(HeaderValue::new(bad), None, "accepted {bad:?}");
            let json = serde_json::to_string(bad).unwrap();
            let err = serde_json::from_str::<HeaderValue>(&json).unwrap_err();
            assert!(
                err.to_string().contains("legal HTTP header value"),
                "got: {err}"
            );
        }

        // A real validator round-trips as the bare string the golden holds.
        for good in [
            GOLDEN_ETAG,
            GOLDEN_LAST_MODIFIED,
            "W/\"weak\"",
            &"x".repeat(MAX_VALIDATOR_LEN),
        ] {
            let v = validator(good);
            assert_eq!(v.as_str(), good);
            let json = serde_json::to_string(&v).unwrap();
            assert_eq!(json, serde_json::to_string(good).unwrap());
            assert_eq!(serde_json::from_str::<HeaderValue>(&json).unwrap(), v);
        }
    }

    #[test]
    fn absent_validators_are_omitted_and_read_back_as_none() {
        // An origin that supplied neither validator writes neither field —
        // omitted, not null — and an entry written that way still reads.
        let meta: CacheMeta = serde_json::from_str(GOLDEN_V1_NO_VALIDATORS).unwrap();
        assert_eq!(meta.etag, None);
        assert_eq!(meta.last_modified, None);

        let dir = tempdir().unwrap();
        let path = dir.path().join("v1.4.json");
        write_meta(&path, &meta).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(written, GOLDEN_V1_NO_VALIDATORS);
        assert!(!written.contains("etag"), "got: {written}");
        assert!(!written.contains("last_modified"), "got: {written}");
        assert!(!written.contains("null"), "got: {written}");
    }

    #[test]
    fn a_stored_entry_is_the_golden_on_disk() {
        // The path a real fetch takes, with the clock pinned: what `store`
        // leaves beside the document is exactly the committed golden, and
        // `read_entries` accepts it.
        let dir = tempdir().unwrap();
        let body = Body {
            text: plugin_yaml("1.4", "org_rule"),
            etag: Some(validator(GOLDEN_ETAG)),
            last_modified: Some(validator(GOLDEN_LAST_MODIFIED)),
        };
        store(
            dir.path(),
            GOLDEN_URL,
            &VersionReq::parse("1").unwrap(),
            &ver("1.4"),
            &body,
            at(GOLDEN_CONFIRMED_AT),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("v1.4.json")).unwrap(),
            GOLDEN_V1
        );
        let entries = read_entries(dir.path(), GOLDEN_URL);
        assert_eq!(entries.len(), 1, "{entries:?}");
        assert_eq!(entries[0].version(), &ver("1.4"));
        assert_eq!(entries[0].meta.etag, Some(validator(GOLDEN_ETAG)));
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
        assert_eq!(resolved(&res).as_deref(), Some("1"));

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
        assert_eq!(resolved(&first).as_deref(), Some("1.2"));

        // The plugin author publishes 1.4 — the consumer changes nothing.
        std::fs::write(&plugin, plugin_yaml("1.4", "new_rule")).unwrap();
        let second = load_remote(&url, &req("1"), &opts).unwrap();
        assert!(second.text.contains("new_rule"), "got: {}", second.text);
        assert_eq!(resolved(&second).as_deref(), Some("1.4"));

        // Both versions are now cached as separate entries, and the pin resolves
        // to the newer one even without reaching the origin again.
        let cached = load_remote(&url, &req("1"), &opts_with_cache(cache.path())).unwrap();
        assert_eq!(cached.info.origin, Origin::Cache);
        assert_eq!(resolved(&cached).as_deref(), Some("1.4"));
        let listed = list_cached(cache.path()).unwrap();
        assert_eq!(listed.len(), 2, "{listed:?}");
        assert_eq!(listed[0].version, ver("1.2"));
        assert_eq!(listed[0].newer, Some(ver("1.4")));
        assert_eq!(listed[1].version, ver("1.4"));
        assert_eq!(listed[1].newer, None);
        // A more specific pin still resolves to its own range.
        let pinned = load_remote(&url, &req("1.2"), &opts_with_cache(cache.path())).unwrap();
        assert_eq!(resolved(&pinned).as_deref(), Some("1.2"));
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
        assert_eq!(resolved(&res).as_deref(), Some("1.4"));
        // It is also invisible to the reporting verb.
        let listed = list_cached(cache.path()).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].version, ver("1.4"));
        // …and the stale file is left alone by a clear (it is not ours to read).
        assert_eq!(clear_cached(cache.path()).unwrap(), 1);
        assert!(sub.join("1.yml").is_file());
    }

    #[test]
    fn a_cached_document_that_no_longer_backs_its_metadata_is_not_used() {
        let dir = tempdir().unwrap();
        let plugin = dir.path().join("plug.yml");
        std::fs::write(&plugin, plugin_yaml("1.2", "genuine")).unwrap();
        let cache = tempdir().unwrap();
        let url = file_url(&plugin);
        let opts = opts_with_cache(cache.path());
        load_remote(&url, &req("1"), &opts).unwrap();

        // Tamper with the cached document: it now declares a version its own
        // metadata does not claim, though one the pin would still accept.
        let sub = url_dir(cache.path(), &url);
        let entry = &read_entries(&sub, &url)[0];
        std::fs::write(&entry.data, plugin_yaml("1.9", "tampered")).unwrap();

        // The entry is passed over, so the origin answers instead — the run
        // never judges against a document nothing vouches for.
        let res = load_remote(&url, &req("1"), &opts).unwrap();
        assert!(res.text.contains("genuine"), "got: {}", res.text);
        assert_eq!(res.info.origin, Origin::Fetched);

        // …and with the origin gone there is nothing to fall back to, so it is
        // an error rather than the mismatched document.
        std::fs::write(&entry.data, plugin_yaml("1.9", "tampered")).unwrap();
        std::fs::remove_file(&plugin).unwrap();
        let err = load_remote(&url, &req("1"), &opts).unwrap_err();
        assert!(matches!(err, Error::PluginFetch { .. }), "got: {err}");

        // An unparseable cached document is passed over the same way.
        std::fs::write(&entry.data, "version: : :\n  - oops\n").unwrap();
        let err = load_remote(&url, &req("1"), &opts).unwrap_err();
        assert!(matches!(err, Error::PluginFetch { .. }), "got: {err}");
    }

    #[test]
    fn unreadable_or_foreign_cache_files_are_skipped() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("notjson.txt"), "x").unwrap();
        std::fs::write(dir.path().join("bad.json"), "{not json").unwrap();
        // Right shape, wrong schema version.
        std::fs::write(
            dir.path().join("v9.json"),
            r#"{"schema":99,"url":"u","pin":"1","version":"9","confirmed_at":0}"#,
        )
        .unwrap();
        // Current schema, but the document beside it is missing.
        std::fs::write(
            dir.path().join("v8.json"),
            r#"{"schema":1,"url":"u","pin":"1","version":"8","confirmed_at":0}"#,
        )
        .unwrap();
        // Current schema, unparseable version.
        std::fs::write(
            dir.path().join("vx.json"),
            r#"{"schema":1,"url":"u","pin":"1","version":"x","confirmed_at":0}"#,
        )
        .unwrap();
        // Current schema, no pin — only a pinned fetch is ever cached.
        std::fs::write(dir.path().join("v6.yml"), "version: 6\n").unwrap();
        std::fs::write(
            dir.path().join("v6.json"),
            r#"{"schema":1,"url":"u","version":"6","confirmed_at":0}"#,
        )
        .unwrap();
        // Current schema, a confirmation time no clock can represent: it must be
        // passed over rather than reach the arithmetic that would panic on it.
        std::fs::write(dir.path().join("v5.yml"), "version: 5\n").unwrap();
        std::fs::write(
            dir.path().join("v5.json"),
            r#"{"schema":1,"url":"u","pin":"1","version":"5","confirmed_at":18446744073709551615}"#,
        )
        .unwrap();
        // Current schema, a validator no request could carry: the metadata does
        // not parse, so there is no entry — a header this release would never
        // have written is not repaired into one.
        std::fs::write(dir.path().join("v4.yml"), "version: 4\n").unwrap();
        std::fs::write(
            dir.path().join("v4.json"),
            "{\"schema\":1,\"url\":\"u\",\"pin\":\"1\",\"version\":\"4\",\
             \"confirmed_at\":0,\"etag\":\"v1\\r\\nX-Injected: 1\"}",
        )
        .unwrap();
        assert!(read_entries(dir.path(), "u").is_empty());
        assert!(list_cached(dir.path()).unwrap().is_empty());
        // Metadata under a filename that does not name the version it declares
        // never adopts that version's document.
        std::fs::write(dir.path().join("v2.yml"), "version: 2\n").unwrap();
        std::fs::write(
            dir.path().join("stray.json"),
            r#"{"schema":1,"url":"u","pin":"1","version":"2","confirmed_at":0}"#,
        )
        .unwrap();
        assert!(read_entries(dir.path(), "u").is_empty());
        std::fs::remove_file(dir.path().join("stray.json")).unwrap();
        // Metadata for another URL never answers for this one (hash collision).
        std::fs::write(dir.path().join("v7.yml"), "version: 7\n").unwrap();
        std::fs::write(
            dir.path().join("v7.json"),
            r#"{"schema":1,"url":"other","pin":"1","version":"7","confirmed_at":0}"#,
        )
        .unwrap();
        assert!(read_entries(dir.path(), "u").is_empty());
        assert_eq!(read_entries(dir.path(), "other").len(), 1);
        // A missing directory is simply an empty cache — it is created on the
        // first store.
        assert!(read_entries(&dir.path().join("nope"), "u").is_empty());
        assert!(list_cached(&dir.path().join("nope")).unwrap().is_empty());
        assert_eq!(clear_cached(&dir.path().join("nope")).unwrap(), 0);
        // A cache directory that is not a directory is a reported fault, not an
        // empty cache: the reporting verbs answer about the path they were
        // given. Resolution still shrugs and refetches.
        let not_a_dir = dir.path().join("file.txt");
        std::fs::write(&not_a_dir, "x").unwrap();
        assert!(matches!(list_cached(&not_a_dir), Err(Error::Io(_))));
        assert!(matches!(clear_cached(&not_a_dir), Err(Error::Io(_))));
        assert!(read_entries(&not_a_dir, "u").is_empty());
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
        meta.confirmed_at = at(0);
        write_meta(&entries[0].meta_path, &meta).unwrap();
        assert!(is_stale(&meta, DEFAULT_TTL_SECS, now_confirmed()));

        let res = load_remote(&url, &req("1"), &opts_with_cache(cache.path())).unwrap();
        assert!(res.text.contains("name: r"));
        let after = read_entries(&sub, &url);
        assert_eq!(after.len(), 1, "an unchanged version stays one entry");
        assert!(
            !is_stale(&after[0].meta, DEFAULT_TTL_SECS, now_confirmed()),
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
        assert_eq!(entries[0].meta.etag, Some(validator("\"v1\"")));
        let mut meta = entries[0].meta.clone();
        meta.confirmed_at = at(0);
        write_meta(&entries[0].meta_path, &meta).unwrap();

        let second = load_remote(&url, &req("1"), &opts).unwrap();
        assert_eq!(second.info.origin, Origin::Cache);
        assert!(second.text.contains("remote_rule"));
        assert_eq!(
            origin.bodies(),
            1,
            "an unchanged document must not be re-downloaded"
        );
        assert!(read_entries(&sub, &url)[0].meta.confirmed_at.secs() > 0);
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
            schema: CacheSchema,
            url: "u".into(),
            pin: VersionReq::parse("1").unwrap(),
            version: ver("1"),
            confirmed_at: at(100),
            etag: None,
            last_modified: None,
        };
        assert!(!is_stale(&meta, 10, at(105)));
        assert!(is_stale(&meta, 10, at(110)));
        // A future timestamp counts as fresh rather than underflowing.
        assert!(!is_stale(&meta, 10, at(50)));
        // A zero window always revalidates.
        assert!(is_stale(&meta, 0, at(100)));
    }

    #[test]
    fn unpinned_plugin_is_not_cached() {
        let dir = tempdir().unwrap();
        let plugin = dir.path().join("plug.yml");
        std::fs::write(&plugin, "rules: []\n").unwrap();
        let cache = tempdir().unwrap();
        let opts = opts_with_cache(cache.path());
        let res = load_remote(&file_url(&plugin), &None, &opts).unwrap();
        assert_eq!(res.info.resolved, Resolved::Unpinned { version: None });
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
        assert!(res.info.resolved.version().is_some());
        assert!(res.info.resolved.pin().is_some());
        assert!(res.info.to_human().contains("from bundled"));
        // A pin the embedded version can't satisfy still errors.
        let err = load_remote(assets::CONFIG_LINT_URL, &req("2"), &no_cache()).unwrap_err();
        assert!(matches!(err, Error::PluginVersionMismatch { .. }));
    }

    #[test]
    fn resolution_renders_a_human_line() {
        let res = PluginResolution {
            url: "https://x/p.yml".into(),
            resolved: Resolved::Pinned {
                pin: VersionReq::parse("1").unwrap(),
                version: ver("1.4"),
            },
            origin: Origin::Cache,
        };
        assert_eq!(
            res.to_human(),
            "https://x/p.yml@1 -> version 1.4 (from cache)"
        );
        let unpinned = PluginResolution {
            url: "https://x/p.yml".into(),
            resolved: Resolved::Unpinned { version: None },
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
