//! The `oneharness` subprocess client: build the `run` invocation, spawn it
//! with a wall-clock timeout, and extract the validated `structured` verdict.
//!
//! This is the one genuinely-external boundary in llmlint. oneharness enforces
//! and validates the JSON Schema itself (`--schema`), so the client only has to
//! pass the schema/system/prompt and read `results[0].structured`.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;
use wait_timeout::ChildExt;

use crate::domain::verdict::RuleVerdict;
use crate::errors::{io_err, Error, Result};

/// Default binary name, resolved on `PATH`.
pub const DEFAULT_BIN: &str = "oneharness";

/// A handle to the oneharness binary (existence is checked lazily on use).
pub struct Client {
    pub bin: PathBuf,
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
}

#[derive(Deserialize)]
struct RunResult {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    structured: Option<Value>,
    #[serde(default)]
    schema_valid: Option<bool>,
    #[serde(default)]
    schema_error: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

impl Client {
    /// Build a client for the given binary override, or the default on `PATH`.
    pub fn new(bin_override: Option<&str>) -> Client {
        Client {
            bin: PathBuf::from(bin_override.unwrap_or(DEFAULT_BIN)),
        }
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

        // Build the arg vector once, so the spawned command and the displayed
        // trace command can never drift apart.
        let mut args: Vec<OsString> = vec![
            "run".into(),
            "--system".into(),
            req.system.into(),
            "--prompt".into(),
            req.prompt.into(),
            "--schema".into(),
            schema_file.path().as_os_str().to_os_string(),
            "--cwd".into(),
            req.cwd.as_os_str().to_os_string(),
            "--timeout".into(),
            req.timeout_secs.to_string().into(),
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

    let result = report.results.into_iter().next().ok_or_else(|| {
        Error::Oneharness(format!(
            "oneharness returned no results for harness {harness}"
        ))
    })?;

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
        assert!(trace.command.contains("run --system sys"));
        assert!(trace.command.contains("--harness claude-code"));
        // No process ran, so there is no output and the run errored.
        assert!(trace.exit_code.is_none());
        assert!(trace.stdout.is_empty());
        assert!(matches!(result, Err(Error::OneharnessNotFound(_))));
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
}
