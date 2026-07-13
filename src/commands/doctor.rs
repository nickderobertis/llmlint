//! `llmlint doctor`: confirm oneharness (llmlint's runtime prerequisite) is
//! installed, reachable, and recent enough to run in read-only mode. With
//! `--probe`, also confirm the harness *executes* tools inline instead of
//! deferring them to a controller — the bridged/managed-session trap (issue
//! #142) that a version check alone can't catch.

use crate::cli::DoctorArgs;
use crate::errors::{Error, Result};
use crate::io::oneharness::{self, ProbeOutcome};

pub fn run(args: DoctorArgs) -> Result<i32> {
    // Flag wins over env var (like the other commands), then PATH / sibling.
    let bin = args.oneharness_bin.clone().or_else(|| {
        std::env::var("LLMLINT_ONEHARNESS_BIN")
            .ok()
            .filter(|s| !s.is_empty())
    });
    let client = oneharness::Client::new(bin.as_deref());
    // Reports the version and fails clearly when oneharness is missing or older
    // than the minimum required for read-only mode.
    let version = client.check_min_version()?;
    println!("oneharness: {version} ({})", client.bin.display());

    if !args.probe {
        return Ok(0);
    }

    // Opt-in, billed tool-execution probe: prove the harness runs a tool inline
    // rather than deferring it. A deferral surfaces the same actionable error a
    // real lint run would hit, so this catches the wall up front instead of
    // after a full run.
    let harness = args.harness.as_deref();
    println!(
        "probing tool execution via {} (this makes a real model call)…",
        harness.unwrap_or("the default harness")
    );
    match client.probe(harness, args.model.as_deref(), args.timeout)? {
        ProbeOutcome::Executed => {
            println!("tool execution: ok — the harness ran a tool inline");
            Ok(0)
        }
        ProbeOutcome::Deferred(detail) => Err(Error::ToolDeferred {
            // Match the client's own label for an unspecified harness so the
            // message reads cleanly ("harness oneharness default deferred …").
            harness: harness.unwrap_or("oneharness default").to_string(),
            detail,
        }),
    }
}
