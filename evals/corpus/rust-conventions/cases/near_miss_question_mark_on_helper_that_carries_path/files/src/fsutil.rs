use std::fs;
use std::path::Path;

use crate::error::ReadError;

/// Read `path` into a string. On failure the error carries `path`, so callers
/// can propagate it with a bare `?` and still report which file failed.
pub fn read_text(path: &Path) -> Result<String, ReadError> {
    fs::read_to_string(path).map_err(|source| ReadError { path: path.to_path_buf(), source })
}
