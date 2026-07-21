//! The `oneharness` subprocess client: build the `run` invocation, spawn it
//! with a wall-clock timeout, and extract the validated `structured` verdict.
//!
//! This is the one genuinely-external boundary in llmlint. oneharness enforces
//! and validates the JSON Schema itself (`--schema`), so the client only has to
//! pass the schema/system/prompt and read the winning result's `structured`
//! verdict — for a fallback run, the harness named in `fallback.ran`, not
//! blindly `results[0]` (which may be a harness skipped as unavailable).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use wait_timeout::ChildExt;

use crate::domain::verdict::RuleVerdict;
use crate::errors::{io_err, Error, Result};

/// Default binary name, resolved on `PATH`.
pub const DEFAULT_BIN: &str = "oneharness";

/// Minimum oneharness version llmlint requires, as `(major, minor, patch)`.
/// `--system-file` — which lets llmlint pass its (potentially large) rendered
/// system prompt by file path instead of as an argv string that could trip the
/// OS `Argument list too long` limit — landed in oneharness 0.3.12. (Read-only
/// mode, `--mode read-only`, has been required since 0.3.0.) The named
/// `failure_kind: "tool_deferred"` that lets llmlint give a specific diagnostic
/// when a bridged/managed harness defers a builtin tool instead of running it
/// (issue #142) landed in 0.3.21 — the current floor. An older binary lacks
/// these, so it is rejected up front.
pub const MIN_VERSION: (u64, u64, u64) = (0, 3, 21);

const HISTORY_LABELS_ENV: &str = "ONEHARNESS_HISTORY_LABELS";

/// Add llmlint's role to oneharness's comma-separated `key=value` environment
/// format, replacing an inherited role while retaining every other label.
///
/// Contract source (oneharness README and parser at commit 23393fe):
/// <https://github.com/nickderobertis/oneharness/blob/23393fefc7873c57d09c4fa0f05ee50b8e250583/README.md#L1226-L1236>
/// and
/// <https://github.com/nickderobertis/oneharness/blob/23393fefc7873c57d09c4fa0f05ee50b8e250583/crates/oneharness-core/src/domain/config.rs#L339-L341>.
fn llmlint_history_labels(inherited: Option<String>) -> String {
    let mut labels: Vec<&str> = inherited
        .as_deref()
        .into_iter()
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|label| {
            !label.is_empty()
                && label
                    .split_once('=')
                    .is_none_or(|(key, _)| key.trim() != "role")
        })
        .collect();
    labels.push("role=llmlint");
    labels.join(",")
}

/// Render a `(major, minor, patch)` version as `major.minor.patch`.
fn format_version((major, minor, patch): (u64, u64, u64)) -> String {
    format!("{major}.{minor}.{patch}")
}

/// Extract `(major, minor, patch)` from a `oneharness --version` line such as
/// `oneharness 0.3.0` or `oneharness 0.3.1 (abc)`. The first whitespace token
/// that starts with a `major.minor[.patch]` numeric run wins; a missing patch
/// defaults to 0 and any pre-release/build suffix is ignored. Returns `None`
/// when no such token is present.
fn parse_semver(version_line: &str) -> Option<(u64, u64, u64)> {
    for token in version_line.split_whitespace() {
        // Tolerate a leading `v` (e.g. `v0.3.0`).
        let token = token.strip_prefix('v').unwrap_or(token);
        // Take the leading numeric-dotted run, dropping any suffix like `-rc1`.
        let core: String = token
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        let parts: Vec<&str> = core.split('.').filter(|p| !p.is_empty()).collect();
        // Need at least `major.minor` to call it a version (so the bare program
        // name and stray single integers don't masquerade as one).
        if parts.len() < 2 {
            continue;
        }
        let nums: Option<Vec<u64>> = parts.iter().map(|p| p.parse().ok()).collect();
        if let Some(nums) = nums {
            return Some((nums[0], nums[1], nums.get(2).copied().unwrap_or(0)));
        }
    }
    None
}

/// A handle to the oneharness binary (existence is checked lazily on use).
pub struct Client {
    pub bin: PathBuf,
}

/// True when `name` resolves in one of `paths`' directories, mirroring the
/// lookup `Command::new` does for a bare program name. On Windows, also probe
/// `name.exe` — the one extension our release archives and wheels ship.
fn found_in_paths(paths: &std::ffi::OsStr, name: &str) -> bool {
    std::env::split_paths(paths).any(|dir| {
        !dir.as_os_str().is_empty()
            && (dir.join(name).is_file()
                || (cfg!(windows) && dir.join(format!("{name}.exe")).is_file()))
    })
}

/// The oneharness binary sitting in `dir`, if any.
fn sibling_in(dir: &Path) -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "oneharness.exe"
    } else {
        "oneharness"
    };
    let candidate = dir.join(name);
    candidate.is_file().then_some(candidate)
}

/// Resolve a `oneharness` living NEXT TO the running llmlint executable.
/// Tool-isolating installers (`uv tool install`, `pipx`) install the
/// llmlint-cli wheel and its oneharness-cli dependency into one private venv
/// but link only llmlint's own executable onto PATH — oneharness ends up
/// beside the real llmlint binary, invisible to a PATH lookup. Probing that
/// sibling makes those installs work with zero flags. `current_exe` is
/// canonicalized so the probe happens in the real venv `bin/`, not next to the
/// launcher symlink.
fn sibling_oneharness() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe = exe.canonicalize().unwrap_or(exe);
    sibling_in(exe.parent()?)
}

/// A record of one oneharness invocation, for the `-v` debug view: the exact
/// command line and the raw subprocess result. Empty/`None` fields mean that
/// stage wasn't reached (e.g. the binary was not found, so there is no output).
#[derive(Debug, Default, Clone)]
pub struct RunTrace {
    /// The exact command line (program + args), shell-quoted for copy/paste.
    pub command: String,
    /// Process exit code, if the child ran to completion.
    pub exit_code: Option<i32>,
    /// Raw stdout (the oneharness JSON report).
    pub stdout: String,
    /// Raw stderr.
    pub stderr: String,
}

/// One judge invocation request.
pub struct RunRequest<'a> {
    /// Harness id to select, or `None` to omit `--harness` and let oneharness
    /// use its own configured default harness.
    pub harness: Option<&'a str>,
    pub model: Option<&'a str>,
    pub system: &'a str,
    pub prompt: &'a str,
    pub schema: &'a Value,
    pub schema_max_retries: Option<u32>,
    pub cwd: &'a Path,
    pub timeout_secs: u64,
    /// Single oneharness config to forward via `--config` (replaces discovery).
    pub oneharness_config: Option<&'a Path>,
    /// Pass `--no-config` so oneharness ignores its own config discovery.
    pub no_config: bool,
}

#[derive(Deserialize)]
struct Report {
    #[serde(default)]
    results: Vec<RunResult>,
    /// Present only when oneharness ran in **fallback** mode: it names the
    /// harness that actually produced the verdict (`ran`) and lists those that
    /// fell through before it. In fallback mode `results` holds every *attempted*
    /// harness in priority order, so `results[0]` may be one skipped as
    /// unavailable — this block is the authority on which entry is the winner.
    #[serde(default)]
    fallback: Option<Fallback>,
}

#[derive(Deserialize)]
struct Fallback {
    /// The harness oneharness fell through to and ran (`None` if the whole chain
    /// failed and nothing ran).
    #[serde(default)]
    ran: Option<String>,
}

#[derive(Deserialize)]
struct RunResult {
    /// The harness this result is for; used to match the fallback winner.
    #[serde(default)]
    harness: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    structured: Option<Value>,
    #[serde(default)]
    schema_valid: Option<bool>,
    #[serde(default)]
    schema_error: Option<String>,
    /// A coarse, named reason a run failed (oneharness >= 0.3.21). The one kind
    /// llmlint acts on is `tool_deferred`: the harness exited cleanly but only
    /// *proposed* a builtin tool (Read/Bash/…) for a controller to run instead
    /// of executing it, so it produced no verdict — the bridged/managed-session
    /// trap (issue #142).
    #[serde(default)]
    failure_kind: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

impl RunResult {
    /// True when this result produced a non-null structured verdict — i.e. the
    /// harness actually ran and answered, not skipped/timed-out.
    fn produced_output(&self) -> bool {
        self.structured.as_ref().is_some_and(|v| !v.is_null())
    }
}

impl Client {
    /// Build a client for the given binary override, or the default on `PATH`,
    /// or — when neither resolves — a `oneharness` sitting beside the llmlint
    /// executable (how `uv tool install` / `pipx` lay out the wheels). An
    /// explicit override is always taken as-is; with no override, `PATH` wins
    /// over the sibling so an environment's chosen oneharness is never shadowed
    /// by a bundled one. When nothing resolves, keep the bare default so the
    /// "oneharness not found" error reads the same as before.
    pub fn new(bin_override: Option<&str>) -> Client {
        let bin = match bin_override {
            Some(b) => PathBuf::from(b),
            None => {
                let on_path = std::env::var_os("PATH")
                    .is_some_and(|paths| found_in_paths(&paths, DEFAULT_BIN));
                if on_path {
                    PathBuf::from(DEFAULT_BIN)
                } else {
                    sibling_oneharness().unwrap_or_else(|| PathBuf::from(DEFAULT_BIN))
                }
            }
        };
        Client { bin }
    }

    /// Run `oneharness --version`, mapping a missing binary to a clear error.
    pub fn version(&self) -> Result<String> {
        let output = match Command::new(&self.bin).arg("--version").output() {
            Ok(o) => o,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(Error::OneharnessNotFound(self.bin.display().to_string()))
            }
            Err(e) => return Err(io_err("running oneharness --version", e)),
        };
        if !output.status.success() {
            return Err(Error::Oneharness(format!(
                "`{} --version` failed: {}",
                self.bin.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Confirm the installed oneharness satisfies [`MIN_VERSION`], returning its
    /// raw `--version` string on success. A binary older than the minimum (or
    /// one whose version can't be parsed) is rejected, since read-only mode —
    /// llmlint's guarantee that the harness never edits files — requires it.
    pub fn check_min_version(&self) -> Result<String> {
        let raw = self.version()?;
        match parse_semver(&raw) {
            Some(v) if v >= MIN_VERSION => Ok(raw),
            Some(_) => Err(Error::OneharnessTooOld {
                found: raw,
                required: format_version(MIN_VERSION),
            }),
            None => Err(Error::Oneharness(format!(
                "could not determine the oneharness version from {raw:?}; llmlint \
                 requires oneharness >= {} for read-only mode",
                format_version(MIN_VERSION)
            ))),
        }
    }

    /// Run one judge and return its per-rule verdicts. Convenience wrapper over
    /// [`Client::run_with_trace`] that discards the debug trace.
    pub fn run(&self, req: &RunRequest) -> Result<BTreeMap<String, RuleVerdict>> {
        self.run_with_trace(req).1
    }

    /// Run one judge, returning both a [`RunTrace`] (the exact command + raw
    /// result, for `-v` debug output) and the parsed per-rule verdicts. The
    /// trace is always returned — even when the run errors — so a failure can
    /// be inspected; its fields are best-effort and empty before the relevant
    /// stage is reached.
    pub fn run_with_trace(
        &self,
        req: &RunRequest,
    ) -> (RunTrace, Result<BTreeMap<String, RuleVerdict>>) {
        // A human-readable harness label for error messages; when unset, the
        // harness is whichever default oneharness resolves from its own config.
        let harness = req.harness.unwrap_or("oneharness default");
        let mut trace = RunTrace::default();

        let mut schema_file = match tempfile::Builder::new()
            .prefix("llmlint-schema-")
            .suffix(".json")
            .tempfile()
        {
            Ok(f) => f,
            Err(e) => return (trace, Err(io_err("creating schema temp file", e))),
        };
        match serde_json::to_vec(req.schema)
            .map_err(|e| Error::Io(e.to_string()))
            .and_then(|bytes| {
                schema_file
                    .write_all(&bytes)
                    .and_then(|_| schema_file.flush())
                    .map_err(|e| io_err("writing schema temp file", e))
            }) {
            Ok(()) => {}
            Err(e) => return (trace, Err(e)),
        }

        // The rendered system prompt carries the whole judge briefing — rules,
        // per-file applicability, and every changed file's inlined diff — so it
        // can be large. Passed inline as `--system <TEXT>` it trips the OS
        // single-argument limit and fails at spawn with `Argument list too long`
        // (E2BIG). Write it to a temp file and hand oneharness `--system-file`
        // instead, exactly as the schema is passed by path (requires oneharness
        // >= MIN_VERSION; checked up front).
        let mut system_file = match tempfile::Builder::new()
            .prefix("llmlint-system-")
            .suffix(".txt")
            .tempfile()
        {
            Ok(f) => f,
            Err(e) => return (trace, Err(io_err("creating system temp file", e))),
        };
        match system_file
            .write_all(req.system.as_bytes())
            .and_then(|_| system_file.flush())
            .map_err(|e| io_err("writing system temp file", e))
        {
            Ok(()) => {}
            Err(e) => return (trace, Err(e)),
        }

        // Build the arg vector once, so the spawned command and the displayed
        // trace command can never drift apart.
        let mut args: Vec<OsString> = vec![
            "run".into(),
            "--system-file".into(),
            system_file.path().as_os_str().to_os_string(),
            "--prompt".into(),
            req.prompt.into(),
            "--schema".into(),
            schema_file.path().as_os_str().to_os_string(),
            "--cwd".into(),
            req.cwd.as_os_str().to_os_string(),
            "--timeout".into(),
            req.timeout_secs.to_string().into(),
            // llmlint is a judge, never an editor: run the harness in read-only
            // mode so it may read target files but can't edit them or run
            // commands. (Requires oneharness >= MIN_VERSION; checked up front.)
            "--mode".into(),
            "read-only".into(),
            "--require-available".into(),
            "--compact".into(),
        ];
        if let Some(h) = req.harness {
            args.push("--harness".into());
            args.push(h.into());
        }
        if let Some(m) = req.model {
            args.push("--model".into());
            args.push(m.into());
        }
        if let Some(n) = req.schema_max_retries {
            args.push("--schema-max-retries".into());
            args.push(n.to_string().into());
        }
        if req.no_config {
            args.push("--no-config".into());
        } else if let Some(c) = req.oneharness_config {
            args.push("--config".into());
            args.push(c.as_os_str().to_os_string());
        }
        trace.command = render_command(&self.bin, &args);

        let mut cmd = Command::new(&self.bin);
        cmd.args(&args)
            .env(
                HISTORY_LABELS_ENV,
                llmlint_history_labels(std::env::var(HISTORY_LABELS_ENV).ok()),
            )
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return (
                    trace,
                    Err(Error::OneharnessNotFound(self.bin.display().to_string())),
                )
            }
            Err(e) => return (trace, Err(io_err("spawning oneharness", e))),
        };

        // Give oneharness its own timeout plus a margin before we hard-kill it,
        // so a clean per-harness `timeout` result can still come back as JSON.
        let wall = Duration::from_secs(req.timeout_secs.saturating_add(30));
        let capture = match wait_capture(child, wall) {
            Ok(Some(c)) => c,
            Ok(None) => {
                return (
                    trace,
                    Err(Error::Oneharness(format!(
                        "oneharness did not exit within {}s (harness {})",
                        wall.as_secs(),
                        harness
                    ))),
                )
            }
            Err(e) => return (trace, Err(e)),
        };
        trace.exit_code = capture.status.code();
        trace.stdout = String::from_utf8_lossy(&capture.stdout).into_owned();
        trace.stderr = String::from_utf8_lossy(&capture.stderr).into_owned();

        let verdicts = parse_verdicts(&capture, harness);
        (trace, verdicts)
    }

    /// Confirm the harness actually *executes* tools, not merely that its binary
    /// answers — the gap `doctor`'s version check can't see (issue #142). Writes
    /// a marker to a temp file and asks the harness to read it back with its
    /// file-reading tool; a deployment that runs tools inline returns a verdict
    /// ([`ProbeOutcome::Executed`]), while a bridged/managed one *defers* the
    /// Read to a controller and oneharness reports `tool_deferred`
    /// ([`ProbeOutcome::Deferred`]). Read is chosen because it is permitted in
    /// read-only mode and mirrors how the judge reads the files it reviews.
    ///
    /// Makes a real, billed model call, so it is opt-in (`doctor --probe`) and
    /// never on llmlint's default paths. Any error other than a deferral (auth,
    /// missing harness, timeout) propagates so the probe can't mask it.
    pub fn probe(
        &self,
        harness: Option<&str>,
        model: Option<&str>,
        timeout_secs: u64,
    ) -> Result<ProbeOutcome> {
        // A marker the harness can only report by actually reading the file, so
        // a deferring deployment is forced to defer the Read rather than guess.
        const MARKER: &str = "LLMLINT_PROBE_OK";
        let mut probe_file = tempfile::Builder::new()
            .prefix("llmlint-probe-")
            .suffix(".txt")
            .tempfile()
            .map_err(|e| io_err("creating probe temp file", e))?;
        probe_file
            .write_all(MARKER.as_bytes())
            .and_then(|_| probe_file.flush())
            .map_err(|e| io_err("writing probe temp file", e))?;
        let path = probe_file.path();
        let cwd = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();

        let prompt = format!(
            "Use your file-reading tool to read the file at {} and report whether \
             its entire contents are exactly `{MARKER}`.",
            path.display()
        );
        let schema = json!({
            "type": "object",
            "additionalProperties": false,
            "properties": { "read_ok": { "type": "boolean" } },
            "required": ["read_ok"],
        });
        let req = RunRequest {
            harness,
            model,
            system: "You are a probe. Use your tools to answer; never guess.",
            prompt: &prompt,
            schema: &schema,
            schema_max_retries: None,
            cwd: &cwd,
            timeout_secs,
            oneharness_config: None,
            no_config: false,
        };
        match self.run(&req) {
            Ok(_) => Ok(ProbeOutcome::Executed),
            Err(Error::ToolDeferred { detail, .. }) => Ok(ProbeOutcome::Deferred(detail)),
            Err(e) => Err(e),
        }
    }
}

/// The outcome of [`Client::probe`]: whether the harness runs tools inline.
#[derive(Debug)]
pub enum ProbeOutcome {
    /// The harness executed a tool and answered — tool-using runs work here.
    Executed,
    /// The harness deferred the tool instead of running it (a bridged/managed
    /// deployment). The string is oneharness's actionable detail (it names the
    /// deferred tool).
    Deferred(String),
}

/// Parse one captured oneharness run into its per-rule verdicts (the verdict
/// extraction split out so `run_with_trace` can keep the trace on every path).
fn parse_verdicts(capture: &Capture, harness: &str) -> Result<BTreeMap<String, RuleVerdict>> {
    let report: Report = serde_json::from_slice(&capture.stdout).map_err(|e| {
        Error::Oneharness(format!(
            "could not parse oneharness output ({e}); exit {:?}; stderr: {}",
            capture.status.code(),
            String::from_utf8_lossy(&capture.stderr).trim()
        ))
    })?;

    if report.results.is_empty() {
        return Err(Error::Oneharness(format!(
            "oneharness returned no results for harness {harness}"
        )));
    }

    // Pick which result carries the verdict. For a single-harness run that is
    // `results[0]`. In **fallback** mode oneharness runs harnesses in priority
    // order and names the one that actually ran in `fallback.ran`, while
    // `results` still lists every *attempted* harness (including any skipped as
    // unavailable) in that order — so `results[0]` can be a skipped entry, not
    // the winner (issue #146). Select the named winner; when the whole chain
    // failed with nothing to select, report the entire chain rather than a
    // single skipped harness's "no structured output".
    let winner = select_winner_index(&report);
    let result = match winner {
        Some(i) => report.results.into_iter().nth(i).expect("index in range"),
        None => return Err(fallback_chain_error(&report, harness)),
    };

    // A deferred builtin tool is a *named* failure (oneharness >= 0.3.21), not a
    // schema/output error: the harness proposed a tool (Read/Bash/…) for an
    // external controller to run and stopped, so there is no verdict. Check it
    // first — without this it would fall through to the schema-invalid or
    // no-structured-output branch below and read like a config bug, which is the
    // whole wall issue #142 describes. Surface oneharness's actionable `error`
    // (it names the tool) inside a specific, pointed diagnostic.
    if result.failure_kind.as_deref() == Some("tool_deferred") {
        return Err(Error::ToolDeferred {
            harness: harness.to_string(),
            detail: result
                .error
                .filter(|e| !e.trim().is_empty())
                .unwrap_or_else(|| {
                    "The harness deferred a builtin tool call to a controller.".into()
                }),
        });
    }

    if result.schema_valid == Some(false) {
        return Err(Error::Oneharness(format!(
            "harness {} produced output that failed schema validation: {}",
            harness,
            result
                .schema_error
                .unwrap_or_else(|| "unknown error".into())
        )));
    }

    let structured = match result.structured {
        Some(v) if !v.is_null() => v,
        _ => {
            return Err(Error::Oneharness(format!(
                "harness {} returned no structured output (status {:?}): {}",
                harness,
                result.status.as_deref().unwrap_or("?"),
                result.error.unwrap_or_else(|| "no error reported".into())
            )))
        }
    };

    serde_json::from_value(structured).map_err(|e| {
        Error::Oneharness(format!("invalid verdict shape from harness {harness}: {e}"))
    })
}

/// Choose the index of the `results` entry that carries the run's verdict.
///
/// - **Non-fallback run** (no `fallback` block): a single result, so index 0.
/// - **Fallback run:** oneharness names the harness it fell through to and ran
///   in `fallback.ran`; select that entry. If the name can't be matched (or is
///   absent), fall back to the first entry that actually produced structured
///   output — the equivalent signal. Returns `None` only when a fallback chain
///   left no successful harness, so the caller can report the whole chain.
fn select_winner_index(report: &Report) -> Option<usize> {
    let Some(fallback) = &report.fallback else {
        // Single-harness run: the sole result is the verdict.
        return (!report.results.is_empty()).then_some(0);
    };
    if let Some(ran) = fallback.ran.as_deref() {
        if let Some(i) = report
            .results
            .iter()
            .position(|r| r.harness.as_deref() == Some(ran))
        {
            return Some(i);
        }
    }
    // No usable `fallback.ran`: the winner is the first harness that answered.
    report.results.iter().position(RunResult::produced_output)
}

/// Build the error for a fallback run where no harness produced a verdict,
/// naming every attempted harness and why it failed (status + error) so the
/// message reflects the whole chain instead of a single skipped harness.
fn fallback_chain_error(report: &Report, harness: &str) -> Error {
    let chain: Vec<String> = report
        .results
        .iter()
        .map(|r| {
            let name = r.harness.as_deref().unwrap_or("?");
            let status = r.status.as_deref().unwrap_or("?");
            match r.error.as_deref() {
                Some(e) if !e.is_empty() => format!("{name} ({status}: {e})"),
                _ => format!("{name} ({status})"),
            }
        })
        .collect();
    Error::Oneharness(format!(
        "all harnesses in the fallback chain failed for {harness}: {}",
        chain.join(", ")
    ))
}

/// Render `bin` + `args` as a single shell-quoted command line for display.
fn render_command(bin: &Path, args: &[OsString]) -> String {
    let mut parts = vec![shell_quote(&bin.to_string_lossy())];
    parts.extend(args.iter().map(|a| shell_quote(&a.to_string_lossy())));
    parts.join(" ")
}

/// Quote a single argument for copy/paste into a POSIX shell. Bare when it is
/// safe (common path/flag characters), single-quoted otherwise.
fn shell_quote(s: &str) -> String {
    let safe = !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./:=@,+".contains(&b));
    if safe {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

struct Capture {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// Wait for `child` up to `wall`, draining stdout/stderr in threads so a large
/// stream can't deadlock the wait. `Ok(None)` means it timed out (and was
/// killed); `Ok(Some(_))` carries the exit status and captured output.
fn wait_capture(mut child: Child, wall: Duration) -> Result<Option<Capture>> {
    let mut out = child.stdout.take().expect("piped stdout");
    let mut err = child.stderr.take().expect("piped stderr");
    let out_h = thread::spawn(move || {
        let mut b = Vec::new();
        let _ = out.read_to_end(&mut b);
        b
    });
    let err_h = thread::spawn(move || {
        let mut b = Vec::new();
        let _ = err.read_to_end(&mut b);
        b
    });

    let status = match child
        .wait_timeout(wall)
        .map_err(|e| io_err("waiting for subprocess", e))?
    {
        Some(s) => s,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
    };
    Ok(Some(Capture {
        status,
        stdout: out_h.join().unwrap_or_default(),
        stderr: err_h.join().unwrap_or_default(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::Path;

    fn req<'a>(schema: &'a Value, cwd: &'a Path) -> RunRequest<'a> {
        RunRequest {
            harness: Some("claude-code"),
            model: None,
            system: "sys",
            prompt: "go",
            schema,
            schema_max_retries: None,
            cwd,
            timeout_secs: 5,
            oneharness_config: None,
            no_config: true,
        }
    }

    #[test]
    fn missing_binary_is_not_found_error() {
        let client = Client::new(Some("definitely-not-a-real-binary-xyz"));
        assert!(matches!(
            client.version(),
            Err(Error::OneharnessNotFound(_))
        ));
        let schema = json!({"type": "object"});
        let cwd = std::env::temp_dir();
        assert!(matches!(
            client.run(&req(&schema, &cwd)),
            Err(Error::OneharnessNotFound(_))
        ));
    }

    #[test]
    fn trace_records_the_command_even_when_the_run_fails() {
        let client = Client::new(Some("definitely-not-a-real-binary-xyz"));
        let schema = json!({"type": "object"});
        let cwd = std::env::temp_dir();
        let (trace, result) = client.run_with_trace(&req(&schema, &cwd));
        // The exact command is captured for `-v` even though spawning failed.
        assert!(trace.command.contains("definitely-not-a-real-binary-xyz"));
        // The large system prompt is passed by file, not inline, so the traced
        // command shows `--system-file <path>` rather than the system text.
        assert!(trace.command.contains("run --system-file"));
        assert!(trace.command.contains("--harness claude-code"));
        // No process ran, so there is no output and the run errored.
        assert!(trace.exit_code.is_none());
        assert!(trace.stdout.is_empty());
        assert!(matches!(result, Err(Error::OneharnessNotFound(_))));
    }

    #[test]
    fn parse_semver_reads_major_minor_patch() {
        assert_eq!(parse_semver("oneharness 0.3.0"), Some((0, 3, 0)));
        assert_eq!(parse_semver("oneharness 0.3.1 (abc)"), Some((0, 3, 1)));
        assert_eq!(parse_semver("oneharness 0.2.529 (mock)"), Some((0, 2, 529)));
        assert_eq!(parse_semver("oneharness 1.2.3"), Some((1, 2, 3)));
        // A missing patch defaults to 0; a leading `v` and a pre-release suffix
        // are tolerated.
        assert_eq!(parse_semver("oneharness 0.4"), Some((0, 4, 0)));
        assert_eq!(parse_semver("v0.5.0"), Some((0, 5, 0)));
        assert_eq!(parse_semver("oneharness 0.3.0-rc1"), Some((0, 3, 0)));
    }

    #[test]
    fn parse_semver_rejects_non_versions() {
        assert_eq!(parse_semver("oneharness"), None);
        assert_eq!(parse_semver(""), None);
        // A bare integer is not a version (needs at least major.minor).
        assert_eq!(parse_semver("oneharness 7"), None);
    }

    #[test]
    fn min_version_comparison_uses_tuple_order() {
        // Sanity-check the ordering the `check_min_version` gate relies on.
        assert!((0, 3, 21) >= MIN_VERSION);
        assert!((0, 4, 0) >= MIN_VERSION);
        assert!((1, 0, 0) >= MIN_VERSION);
        assert!((0, 3, 20) < MIN_VERSION);
        assert!((0, 3, 0) < MIN_VERSION);
    }

    #[test]
    fn check_min_version_errors_when_binary_missing() {
        let client = Client::new(Some("definitely-not-a-real-binary-xyz"));
        assert!(matches!(
            client.check_min_version(),
            Err(Error::OneharnessNotFound(_))
        ));
    }

    /// Parse a report body and run it through the same extraction `run` uses.
    fn verdicts_from(body: &Value) -> Result<BTreeMap<String, RuleVerdict>> {
        let capture = Capture {
            status: fake_status(0),
            stdout: serde_json::to_vec(body).unwrap(),
            stderr: Vec::new(),
        };
        parse_verdicts(&capture, "oneharness default")
    }

    /// A dummy successful exit status (any real process; we only use `.code()`).
    fn fake_status(_code: i32) -> ExitStatus {
        // `ExitStatus` has no public constructor; take a trivially-succeeding
        // command's status. Portable across unix/windows.
        #[cfg(unix)]
        {
            Command::new("true").status().unwrap()
        }
        #[cfg(windows)]
        {
            Command::new("cmd").args(["/C", "exit 0"]).status().unwrap()
        }
    }

    fn ok_result(harness: &str) -> Value {
        json!({
            "harness": harness,
            "status": "ok",
            "exit_code": 0,
            "structured": { "some_rule": { "holds": true } },
            "schema_valid": true,
        })
    }

    fn skipped_result(harness: &str) -> Value {
        json!({
            "harness": harness,
            "status": "skipped",
            "available": false,
            "exit_code": null,
            "structured": null,
            "error": format!("`{harness}` not found on PATH; harness skipped."),
        })
    }

    #[test]
    fn fallback_run_reads_the_ran_winner_not_results_zero() {
        // Issue #146: codex is skipped (results[0]) and claude-code ran. The
        // top-level `fallback.ran` names the winner; its verdict must be used,
        // not the skipped first entry.
        let body = json!({
            "schema_version": "0.1",
            "oneharness_version": "mock",
            "fallback": { "ran": "claude-code",
                          "fell_through": [{ "harness": "codex", "reason": "not-installed" }] },
            "results": [skipped_result("codex"), ok_result("claude-code")],
        });
        let verdicts = verdicts_from(&body).expect("winner's verdict is used");
        assert!(verdicts.contains_key("some_rule"));
    }

    #[test]
    fn fallback_run_all_failed_reports_the_whole_chain() {
        // Every harness fell through: the error names the chain, not a single
        // skipped harness's "no structured output".
        let body = json!({
            "schema_version": "0.1",
            "oneharness_version": "mock",
            "fallback": { "ran": null,
                          "fell_through": [{ "harness": "codex", "reason": "not-installed" }] },
            "results": [skipped_result("codex"), skipped_result("claude-code")],
        });
        let err = verdicts_from(&body).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("fallback chain"), "chain-aware message: {msg}");
        assert!(
            msg.contains("codex") && msg.contains("claude-code"),
            "{msg}"
        );
    }

    #[test]
    fn non_fallback_run_still_uses_results_zero() {
        // No `fallback` block: the single result is the verdict, unchanged.
        let body = json!({
            "schema_version": "0.1",
            "oneharness_version": "mock",
            "results": [ok_result("claude-code")],
        });
        assert!(verdicts_from(&body).unwrap().contains_key("some_rule"));
    }

    #[test]
    fn deferred_tool_is_a_specific_error_not_a_schema_error() {
        // Issue #142: oneharness >= 0.3.21 reports `failure_kind: "tool_deferred"`
        // (status ok, null structured). llmlint must map it to the pointed
        // `ToolDeferred` diagnostic surfacing oneharness's `error`, never the
        // generic no-structured/schema-invalid message.
        let body = json!({
            "schema_version": "0.1",
            "oneharness_version": "mock",
            "results": [{
                "harness": "claude-code",
                "status": "ok",
                "exit_code": 0,
                "structured": null,
                "schema_valid": null,
                "failure_kind": "tool_deferred",
                "error": "harness claude-code deferred a tool call (`Read`).",
            }],
        });
        let err = verdicts_from(&body).unwrap_err();
        assert!(
            matches!(err, Error::ToolDeferred { .. }),
            "expected ToolDeferred, got {err:?}"
        );
        let msg = err.to_string();
        assert!(msg.contains("deferred a tool call"), "{msg}");
        // oneharness's detail (naming the tool) is carried through.
        assert!(msg.contains("`Read`"), "{msg}");
    }

    #[test]
    fn deferred_tool_without_detail_still_diagnoses() {
        // Even if oneharness omits the `error` detail, the diagnostic stands on
        // its own (a default detail) rather than falling through.
        let body = json!({
            "schema_version": "0.1",
            "oneharness_version": "mock",
            "results": [{
                "harness": "claude-code",
                "status": "ok",
                "structured": null,
                "failure_kind": "tool_deferred",
            }],
        });
        let err = verdicts_from(&body).unwrap_err();
        assert!(matches!(err, Error::ToolDeferred { .. }), "{err:?}");
    }

    #[test]
    fn fallback_without_ran_name_picks_first_harness_that_answered() {
        // A defensive path: the `ran` name is absent, so the winner is the first
        // entry that produced structured output.
        let body = json!({
            "schema_version": "0.1",
            "oneharness_version": "mock",
            "fallback": { "fell_through": [{ "harness": "codex", "reason": "not-installed" }] },
            "results": [skipped_result("codex"), ok_result("claude-code")],
        });
        assert!(verdicts_from(&body).unwrap().contains_key("some_rule"));
    }

    #[test]
    fn shell_quote_is_bare_when_safe_and_quoted_otherwise() {
        assert_eq!(shell_quote("run"), "run");
        assert_eq!(shell_quote("--harness"), "--harness");
        assert_eq!(shell_quote("/tmp/a.json"), "/tmp/a.json");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn render_command_joins_program_and_args() {
        let args: Vec<OsString> = vec!["run".into(), "--system".into(), "hi there".into()];
        let rendered = render_command(Path::new("oneharness"), &args);
        assert_eq!(rendered, "oneharness run --system 'hi there'");
    }

    #[cfg(unix)]
    #[test]
    fn wait_capture_collects_output() {
        let child = Command::new("sh")
            .arg("-c")
            .arg("printf hello; printf oops 1>&2")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let cap = wait_capture(child, Duration::from_secs(5))
            .unwrap()
            .unwrap();
        assert!(cap.status.success());
        assert_eq!(cap.stdout, b"hello");
        assert_eq!(cap.stderr, b"oops");
    }

    #[cfg(unix)]
    #[test]
    fn wait_capture_times_out_and_kills() {
        let child = Command::new("sh")
            .arg("-c")
            .arg("sleep 30")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let result = wait_capture(child, Duration::from_millis(200)).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn found_in_paths_sees_the_binary_and_skips_empty_entries() {
        let dir = tempfile::tempdir().unwrap();
        let name = if cfg!(windows) {
            "oneharness.exe"
        } else {
            "oneharness"
        };
        std::fs::write(dir.path().join(name), b"").unwrap();
        // An empty entry (a leading `:` in PATH) must not match, and the real
        // directory must.
        let paths =
            std::env::join_paths([Path::new(""), dir.path(), Path::new("/nonexistent-xyz")])
                .unwrap();
        assert!(found_in_paths(&paths, DEFAULT_BIN));

        let empty = tempfile::tempdir().unwrap();
        let paths = std::env::join_paths([empty.path()]).unwrap();
        assert!(!found_in_paths(&paths, DEFAULT_BIN));
    }

    #[test]
    fn sibling_in_finds_only_a_real_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(sibling_in(dir.path()).is_none());
        let name = if cfg!(windows) {
            "oneharness.exe"
        } else {
            "oneharness"
        };
        std::fs::write(dir.path().join(name), b"").unwrap();
        assert_eq!(sibling_in(dir.path()), Some(dir.path().join(name)));
        // A directory named oneharness is not a binary.
        let dir2 = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir2.path().join(name)).unwrap();
        assert!(sibling_in(dir2.path()).is_none());
    }
}
