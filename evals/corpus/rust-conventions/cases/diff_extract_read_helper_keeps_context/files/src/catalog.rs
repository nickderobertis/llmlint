use std::fs;
use std::path::Path;

use crate::error::{Error, Result};

/// Read `path` and split each `key=value` line into a trimmed pair; lines
/// without `=` are skipped. `what` names the table in the read error.
fn load_pairs(path: &Path, what: &str) -> Result<Vec<(String, String)>> {
    let text = fs::read_to_string(path)
        .map_err(|e| Error::io(format!("reading {what} {}", path.display()), e))?;
    Ok(text
        .lines()
        .filter_map(|row| row.split_once('='))
        .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
        .collect())
}

/// Load SKU display names from `path`: `sku=name` per line.
pub fn load_names(path: &Path) -> Result<Vec<(String, String)>> {
    load_pairs(path, "names")
}

/// Load SKU categories from `path`: `sku=category` per line.
pub fn load_categories(path: &Path) -> Result<Vec<(String, String)>> {
    load_pairs(path, "categories")
}
