use std::fs;
use std::path::Path;

use crate::error::{Error, Result};

/// Load the store's currency code from `path` (the first line, trimmed), or
/// `USD` when the file is empty.
pub fn load_currency(path: &Path) -> Result<String> {
    let text = fs::read_to_string(path)
        .map_err(|e| Error::io(format!("reading currency {}", path.display()), e))?;
    Ok(text.lines().next().unwrap_or("USD").trim().to_string())
}

/// Load the store's display name from `path` (the whole file, trimmed).
pub fn load_store_name(path: &Path) -> Result<String> {
    let text = fs::read_to_string(path)
        .map_err(|e| Error::io(format!("reading store name {}", path.display()), e))?;
    Ok(text.trim().to_string())
}
