//! Results logging: persist each `lint`/`lint-config` run's *full* results to
//! disk so a caller can retrieve everything the terminal report omits.
//!
//! The terminal report is deliberately terse (failing rules + a summary). When
//! results logging is on (the default), every run is also written as one JSON
//! record — the complete per-rule verdicts, votes, per-judge breakdown,
//! violations, rationales, and run errors — under an auto-generated, time-sortable
//! id. `llmlint history <id>` reads it back and drills into it. Only the newest
//! `max_runs` records are kept; older ones are pruned after each run.
//!
//! This module owns the I/O (the clock, the filesystem, the environment). The
//! record *shape* is assembled from the pure [`Report`](crate::domain::report)
//! JSON plus run metadata, so it can never drift from what the report exposes.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::{json, Map, Value};

use crate::domain::config::Config;
use crate::domain::report::Report;
use crate::errors::{io_err, Error, Result};

/// The resolved, effective results-logging settings for a run: whether to log,
/// where, and how many records to keep. Built by [`resolve`] from the config and
/// environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// Whether to write a record for this run at all.
    pub enabled: bool,
    /// Where records live. `None` when no directory could be determined (no
    /// config `history.dir`, no `LLMLINT_HISTORY_DIR`, and no home/data dir) —
    /// logging then no-ops even if `enabled`.
    pub dir: Option<PathBuf>,
    /// How many of the most recent records to keep (older ones are pruned).
    pub max_runs: usize,
}

/// Resolve the effective results-logging settings from the merged `config` and
/// the environment. `force_off` (a `--no-history` flag) hard-disables logging.
///
/// Precedence — **enabled:** `force_off` wins, then the env layer. The canonical
/// `LLMLINT_HISTORY_ENABLED` is already folded into `config.history.enabled` by
/// [`crate::io::env::apply_overrides`] (so it reaches here through the config);
/// the legacy `LLMLINT_NO_HISTORY=1` is honored as an off-switch **only when the
/// canonical variable is unset**, so `LLMLINT_HISTORY_ENABLED` supersedes it.
/// Otherwise the config `history.enabled` (default on) decides. **dir:**
/// `LLMLINT_HISTORY_DIR` wins over a config `history.dir`, which wins over the
/// platform default data dir.
pub fn resolve(config: &Config, force_off: bool) -> Settings {
    // The legacy off-switch defers to the canonical `LLMLINT_HISTORY_ENABLED`
    // when that is set (whichever way), so the new scheme takes the same slot.
    let legacy_off =
        env_flag("LLMLINT_NO_HISTORY") && std::env::var_os("LLMLINT_HISTORY_ENABLED").is_none();
    let enabled = resolve_enabled(force_off, legacy_off, config.history_enabled());
    let env_dir = std::env::var_os("LLMLINT_HISTORY_DIR")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);
    let dir = resolve_dir(env_dir, config.history.dir.as_deref(), default_data_dir());
    Settings {
        enabled,
        dir,
        max_runs: config.history_max_runs(),
    }
}

/// Pure enabled-precedence: an explicit off switch (the flag or the env var)
/// wins, otherwise the config decides.
fn resolve_enabled(force_off: bool, env_off: bool, config_enabled: bool) -> bool {
    !force_off && !env_off && config_enabled
}

/// Pure dir-precedence: the env override, then the config value, then the
/// platform default.
fn resolve_dir(
    env_dir: Option<PathBuf>,
    config_dir: Option<&str>,
    default: Option<PathBuf>,
) -> Option<PathBuf> {
    env_dir
        .or_else(|| config_dir.map(PathBuf::from))
        .or(default)
}

/// The platform per-user data directory for llmlint run history, or `None` when
/// no home/data directory can be determined. Mirrors the plugin cache-dir logic
/// (see [`crate::io::plugins`]) but targets the *data* dir — run history is
/// generated data, not configuration.
fn default_data_dir() -> Option<PathBuf> {
    if let Some(x) = non_empty_var("XDG_DATA_HOME") {
        return Some(PathBuf::from(x).join("llmlint").join("history"));
    }
    #[cfg(windows)]
    if let Some(a) = non_empty_var("LOCALAPPDATA") {
        return Some(
            PathBuf::from(a)
                .join("llmlint")
                .join("data")
                .join("history"),
        );
    }
    if let Some(h) = non_empty_var("HOME") {
        return Some(
            PathBuf::from(h)
                .join(".local")
                .join("share")
                .join("llmlint")
                .join("history"),
        );
    }
    None
}

fn non_empty_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|s| !s.is_empty())
}

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false)
}

/// An auto-generated run id from the wall clock: a compact, lexicographically
/// time-sortable UTC stamp plus a sub-second suffix for uniqueness, e.g.
/// `20260704T153000Z-1a2b3`. Sortable by string means listing/pruning by
/// filename is chronological.
pub fn generate_id(now: SystemTime) -> String {
    let (secs, nanos) = epoch_parts(now);
    let (y, mo, d, h, mi, s) = civil(secs);
    // Low 20 bits of the nanosecond fraction distinguish two runs in the same
    // second without a random-number dependency.
    let suffix = nanos & 0xF_FFFF;
    format!("{y:04}{mo:02}{d:02}T{h:02}{mi:02}{s:02}Z-{suffix:05x}")
}

/// A human/machine-readable RFC 3339 UTC timestamp for the record body, e.g.
/// `2026-07-04T15:30:00Z`.
pub fn format_timestamp(now: SystemTime) -> String {
    let (secs, _) = epoch_parts(now);
    let (y, mo, d, h, mi, s) = civil(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// Seconds and nanoseconds since the Unix epoch. A clock before 1970 (or read
/// error) clamps to 0 rather than panicking — an id/timestamp is best-effort
/// metadata, never worth failing a run over.
fn epoch_parts(now: SystemTime) -> (i64, u32) {
    match now.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => (d.as_secs() as i64, d.subsec_nanos()),
        Err(_) => (0, 0),
    }
}

/// Break epoch seconds into UTC `(year, month, day, hour, minute, second)` with
/// no calendar dependency (Howard Hinnant's `civil_from_days`).
fn civil(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    (year, m as u32, d as u32, h as u32, mi as u32, s as u32)
}

/// Assemble the full run record: the run metadata followed by the pure report
/// JSON (`summary`/`rules`/`errors`), so the persisted record carries every field
/// the report exposes and can never drift from it. `config_files` is the ordered
/// source list (files + plugin URLs) that produced the run.
#[allow(clippy::too_many_arguments)]
pub fn build_record(
    id: &str,
    timestamp: &str,
    command: &str,
    cwd: &Path,
    exit_code: i32,
    config_files: &[String],
    report: &Report,
) -> Value {
    let report_json = report.to_json();
    let mut obj = Map::new();
    obj.insert("id".into(), json!(id));
    obj.insert("timestamp".into(), json!(timestamp));
    obj.insert("llmlint_version".into(), json!(env!("CARGO_PKG_VERSION")));
    obj.insert("command".into(), json!(command));
    obj.insert("cwd".into(), json!(cwd.display().to_string()));
    obj.insert("exit_code".into(), json!(exit_code));
    obj.insert("config_files".into(), json!(config_files));
    // Fold in the report's own top-level keys (summary, rules, errors) in order.
    if let Value::Object(report_map) = report_json {
        for (k, v) in report_map {
            obj.insert(k, v);
        }
    }
    Value::Object(obj)
}

/// Write `record` under `dir` as `<id>.json`, then prune the oldest records
/// beyond `max_runs`. Returns the written path. Creates `dir` if needed.
pub fn write_record(dir: &Path, id: &str, record: &Value, max_runs: usize) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)
        .map_err(|e| io_err(format!("creating history dir {}", dir.display()), e))?;
    let path = dir.join(format!("{id}.json"));
    let mut body = serde_json::to_string_pretty(record).map_err(|e| Error::Io(e.to_string()))?;
    body.push('\n');
    std::fs::write(&path, body)
        .map_err(|e| io_err(format!("writing history record {}", path.display()), e))?;
    prune(dir, max_runs)?;
    Ok(path)
}

/// Delete the oldest records so at most `max_runs` remain. Ids sort
/// chronologically as strings, so "oldest" is the lexicographically smallest
/// filename. Individual deletions are best-effort (a failure to remove one stale
/// record must not fail the run).
pub fn prune(dir: &Path, max_runs: usize) -> Result<()> {
    let mut files = record_files(dir)?;
    if files.len() <= max_runs {
        return Ok(());
    }
    files.sort_by(|a, b| a.0.cmp(&b.0)); // oldest first
    let remove = files.len() - max_runs;
    for (_, path) in files.into_iter().take(remove) {
        let _ = std::fs::remove_file(path);
    }
    Ok(())
}

/// A stored run record: its id, file path, and parsed JSON body.
#[derive(Debug, Clone)]
pub struct Record {
    pub id: String,
    pub path: PathBuf,
    pub value: Value,
}

/// Every stored record under `dir`, newest first (ids are time-sortable, so this
/// is reverse filename order). Files that don't parse as JSON are skipped rather
/// than failing the whole listing. An absent directory is an empty list.
pub fn all(dir: &Path) -> Result<Vec<Record>> {
    let mut files = record_files(dir)?;
    files.sort_by(|a, b| b.0.cmp(&a.0)); // newest first
    let mut out = Vec::with_capacity(files.len());
    for (id, path) in files {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        out.push(Record { id, path, value });
    }
    Ok(out)
}

/// Load one record by id. `latest` resolves to the most recent run. An unknown id
/// (or empty history for `latest`) is a clear [`Error::History`].
pub fn load(dir: &Path, id: &str) -> Result<Record> {
    if id == "latest" {
        return all(dir)?
            .into_iter()
            .next()
            .ok_or_else(|| Error::History(format!("no runs recorded yet in {}", dir.display())));
    }
    let path = dir.join(format!("{id}.json"));
    let text = std::fs::read_to_string(&path).map_err(|_| {
        Error::History(format!(
            "no run with id {id:?} in {} (run `llmlint history` to list recorded runs)",
            dir.display()
        ))
    })?;
    let value = serde_json::from_str(&text)
        .map_err(|e| Error::History(format!("run {id}: corrupt record: {e}")))?;
    Ok(Record {
        id: id.to_string(),
        path,
        value,
    })
}

/// `(id, path)` for every `*.json` file directly under `dir`. A missing dir is an
/// empty list (nothing has been recorded yet); other read errors surface.
fn record_files(dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io_err(format!("reading history dir {}", dir.display()), e)),
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            out.push((stem.to_string(), path));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tempfile::tempdir;

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn timestamp_and_id_are_utc_and_sortable() {
        // Epoch 0 -> the Unix epoch itself.
        assert_eq!(format_timestamp(at(0)), "1970-01-01T00:00:00Z");
        assert!(generate_id(at(0)).starts_with("19700101T000000Z-"));
        // A known later instant (2023-11-14T22:13:20Z).
        assert_eq!(format_timestamp(at(1_700_000_000)), "2023-11-14T22:13:20Z");
        assert!(generate_id(at(1_700_000_000)).starts_with("20231114T221320Z-"));
        // Ids sort chronologically as strings.
        assert!(generate_id(at(0)) < generate_id(at(1_700_000_000)));
    }

    #[test]
    fn id_suffix_reflects_subsecond_nanos() {
        // Two ids in the same second differ by their nanosecond suffix. The
        // offsets are 0.1ms/0.2ms apart, well above the coarsest `SystemTime`
        // granularity (Windows FILETIME ticks at 100ns), so the difference
        // survives the platform clock rounding a constructed instant.
        let a = generate_id(SystemTime::UNIX_EPOCH + Duration::new(10, 100_000));
        let b = generate_id(SystemTime::UNIX_EPOCH + Duration::new(10, 200_000));
        assert_ne!(a, b);
        assert!(a.starts_with("19700101T000010Z-"));
    }

    #[test]
    fn resolve_precedence_is_pure_and_ordered() {
        // enabled: an explicit off switch wins; otherwise the config decides.
        assert!(!resolve_enabled(true, false, true)); // --no-history
        assert!(!resolve_enabled(false, true, true)); // env off
        assert!(!resolve_enabled(false, false, false)); // config off
        assert!(resolve_enabled(false, false, true)); // default on

        // dir: env > config > default.
        assert_eq!(
            resolve_dir(Some("/env".into()), Some("/cfg"), Some("/def".into())),
            Some(PathBuf::from("/env"))
        );
        assert_eq!(
            resolve_dir(None, Some("/cfg"), Some("/def".into())),
            Some(PathBuf::from("/cfg"))
        );
        assert_eq!(
            resolve_dir(None, None, Some("/def".into())),
            Some(PathBuf::from("/def"))
        );
        assert_eq!(resolve_dir(None, None, None), None);
    }

    #[test]
    fn write_read_and_prune_round_trip() {
        let dir = tempdir().unwrap();
        let rec = |id: &str| json!({"id": id, "exit_code": 0});
        // Three records with time-sortable ids.
        for id in ["20260101T000000Z-00001", "20260102T000000Z-00002"] {
            write_record(dir.path(), id, &rec(id), 10).unwrap();
        }
        // Newest-first listing.
        let listed = all(dir.path()).unwrap();
        assert_eq!(listed[0].id, "20260102T000000Z-00002");
        assert_eq!(listed[1].id, "20260101T000000Z-00001");
        // `latest` resolves to the newest; an exact id loads its body.
        assert_eq!(load(dir.path(), "latest").unwrap().id, listed[0].id);
        assert_eq!(
            load(dir.path(), "20260101T000000Z-00001").unwrap().value["id"],
            "20260101T000000Z-00001"
        );
        // An unknown id is a clear error.
        assert!(load(dir.path(), "nope").is_err());

        // A third write with max_runs=2 prunes the oldest.
        let id3 = "20260103T000000Z-00003";
        write_record(dir.path(), id3, &rec(id3), 2).unwrap();
        let ids: Vec<String> = all(dir.path()).unwrap().into_iter().map(|r| r.id).collect();
        assert_eq!(
            ids,
            vec![id3.to_string(), "20260102T000000Z-00002".to_string()]
        );
        assert!(load(dir.path(), "20260101T000000Z-00001").is_err());
    }

    #[test]
    fn build_record_carries_metadata_and_report_fields() {
        use crate::domain::verdict::{Outcome, RuleOutcome};
        let report = Report::new(
            vec![RuleOutcome {
                name: "r".into(),
                rationale: None,
                outcome: Outcome::Pass,
                votes_total: 1,
                votes_hold: 1,
                judges: vec![],
                violations: vec![],
            }],
            vec![],
        );
        let rec = build_record(
            "id1",
            "2026-07-04T00:00:00Z",
            "lint",
            Path::new("/proj"),
            0,
            &["llmlint.yml".to_string()],
            &report,
        );
        assert_eq!(rec["id"], "id1");
        assert_eq!(rec["command"], "lint");
        assert_eq!(rec["cwd"], "/proj");
        assert_eq!(rec["exit_code"], 0);
        assert_eq!(rec["config_files"][0], "llmlint.yml");
        // The report's own keys are folded in.
        assert_eq!(rec["summary"]["passed"], 1);
        assert_eq!(rec["rules"][0]["name"], "r");
        assert!(rec["errors"].as_array().unwrap().is_empty());
    }

    #[test]
    fn resolve_reads_env_and_config() {
        // Drive the env-composing `resolve` end to end, restoring globals after.
        let prev_dir = std::env::var_os("LLMLINT_HISTORY_DIR");
        let prev_off = std::env::var_os("LLMLINT_NO_HISTORY");
        std::env::set_var("LLMLINT_HISTORY_DIR", "/tmp/llmlint-history-test");
        std::env::remove_var("LLMLINT_NO_HISTORY");

        let mut config = Config {
            history: crate::domain::config::HistoryCfg {
                max_runs: Some(3),
                ..Default::default()
            },
            ..Default::default()
        };
        let s = resolve(&config, false);
        assert!(s.enabled); // default on
        assert_eq!(s.dir, Some(PathBuf::from("/tmp/llmlint-history-test"))); // env wins
        assert_eq!(s.max_runs, 3); // from config

        // `force_off` hard-disables regardless of config.
        config.history.enabled = Some(true);
        assert!(!resolve(&config, true).enabled);

        // The env off-switch also disables.
        std::env::set_var("LLMLINT_NO_HISTORY", "1");
        assert!(!resolve(&config, false).enabled);

        match prev_dir {
            Some(v) => std::env::set_var("LLMLINT_HISTORY_DIR", v),
            None => std::env::remove_var("LLMLINT_HISTORY_DIR"),
        }
        match prev_off {
            Some(v) => std::env::set_var("LLMLINT_NO_HISTORY", v),
            None => std::env::remove_var("LLMLINT_NO_HISTORY"),
        }
    }

    #[test]
    fn latest_of_empty_and_corrupt_records_error() {
        let dir = tempdir().unwrap();
        // `latest` with nothing recorded is a clear error, not a panic.
        assert!(load(dir.path(), "latest").is_err());
        // A file that exists but isn't valid JSON errors when loaded by exact id.
        std::fs::write(dir.path().join("bad.json"), "{ not json").unwrap();
        assert!(load(dir.path(), "bad").is_err());
    }

    #[test]
    fn all_of_a_missing_dir_is_empty() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("nope");
        assert!(all(&missing).unwrap().is_empty());
    }

    #[test]
    fn non_json_files_are_skipped() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("notes.txt"), "hi").unwrap();
        std::fs::write(dir.path().join("bad.json"), "{ not json").unwrap();
        write_record(
            dir.path(),
            "20260101T000000Z-00001",
            &json!({"id": "x"}),
            10,
        )
        .unwrap();
        // The text file and the unparseable json are both skipped; the valid one
        // remains.
        let listed = all(dir.path()).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "20260101T000000Z-00001");
    }
}
