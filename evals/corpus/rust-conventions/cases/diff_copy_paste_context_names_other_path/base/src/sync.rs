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
