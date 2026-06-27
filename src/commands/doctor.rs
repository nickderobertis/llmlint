//! `llmlint doctor`: confirm oneharness (llmlint's runtime prerequisite) is
//! installed, reachable, and recent enough to run in read-only mode.

use crate::errors::Result;
use crate::io::oneharness;

pub fn run() -> Result<i32> {
    let bin = std::env::var("LLMLINT_ONEHARNESS_BIN")
        .ok()
        .filter(|s| !s.is_empty());
    let client = oneharness::Client::new(bin.as_deref());
    // Reports the version and fails clearly when oneharness is missing or older
    // than the minimum required for read-only mode.
    let version = client.check_min_version()?;
    println!("oneharness: {version} ({})", client.bin.display());
    Ok(0)
}
