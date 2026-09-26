use std::fs;
use std::path::Path;

use crate::error::{Error, Result};

/// Write `contents` to `path` atomically: write a sibling `.tmp` file, then
/// rename it over `path`.
pub fn write_snapshot(path: &Path, contents: &str) -> Result<()> {
    let staging_path = path.with_extension("tmp");
    fs::write(&staging_path, contents)
        .map_err(|e| Error::io(format!("writing staging file {}", staging_path.display()), e))?;
    fs::rename(&staging_path, path).map_err(Error::Io)?;
    Ok(())
}

/// Read the snapshot at `path`, or `None` when no snapshot has been written.
pub fn read_snapshot(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(read_error) if read_error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(read_error) => Err(Error::io(format!("reading snapshot {}", path.display()), read_error)),
    }
}
