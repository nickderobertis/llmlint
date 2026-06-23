//! `llmlint doctor`: confirm oneharness (llmlint's runtime prerequisite) is
//! installed and reachable.

use crate::errors::Result;
use crate::io::oneharness;

pub fn run() -> Result<i32> {
    let bin = std::env::var("LLMLINT_ONEHARNESS_BIN")
        .ok()
        .filter(|s| !s.is_empty());
    let client = oneharness::Client::new(bin.as_deref());
    let version = client.version()?;
    println!("oneharness: {version} ({})", client.bin.display());
    Ok(0)
}
