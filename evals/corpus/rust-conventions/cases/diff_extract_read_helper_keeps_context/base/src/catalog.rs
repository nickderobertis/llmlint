use std::fs;
use std::path::Path;

use crate::error::{Error, Result};

/// Load SKU display names from `path`: `sku=name` per line.
pub fn load_names(path: &Path) -> Result<Vec<(String, String)>> {
    let text = fs::read_to_string(path)
        .map_err(|e| Error::io(format!("reading names {}", path.display()), e))?;
    Ok(text
        .lines()
        .filter_map(|row| row.split_once('='))
        .map(|(sku, name)| (sku.trim().to_string(), name.trim().to_string()))
        .collect())
}

/// Load SKU categories from `path`: `sku=category` per line.
pub fn load_categories(path: &Path) -> Result<Vec<(String, String)>> {
    let text = fs::read_to_string(path)
        .map_err(|e| Error::io(format!("reading categories {}", path.display()), e))?;
    Ok(text
        .lines()
        .filter_map(|row| row.split_once('='))
        .map(|(sku, category)| (sku.trim().to_string(), category.trim().to_string()))
        .collect())
}
