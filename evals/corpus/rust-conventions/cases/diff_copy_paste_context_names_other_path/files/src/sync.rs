use std::fs;
use std::path::Path;

use crate::error::{Error, Result};

/// Copy the price table from `source_path` to `mirror_path`, returning the
/// number of bytes copied.
pub fn mirror_prices(source_path: &Path, mirror_path: &Path) -> Result<u64> {
    fs::copy(source_path, mirror_path).map_err(|copy_error| {
        Error::io(
            format!("copying prices {} -> {}", source_path.display(), mirror_path.display()),
            copy_error,
        )
    })
}

/// Remove a stale mirror at `mirror_path`, then copy a fresh one from
/// `source_path`. Returns the number of bytes copied.
pub fn refresh_mirror(source_path: &Path, mirror_path: &Path) -> Result<u64> {
    if mirror_path.exists() {
        fs::remove_file(mirror_path)
            .map_err(|e| Error::io(format!("removing stale mirror {}", source_path.display()), e))?;
    }
    mirror_prices(source_path, mirror_path)
}
