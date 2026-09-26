use std::fs;
use std::path::Path;

use crate::error::{Error, Result};

/// Load the price table from `path`, one `sku,cents` pair per line.
///
/// Returns an error naming the path when the file cannot be read, and an error
/// naming the line when a row is malformed.
pub fn load_prices(path: &Path) -> Result<Vec<(String, u64)>> {
    let text = fs::read_to_string(path)
        .map_err(|e| Error::io(format!("reading price table {}", path.display()), e))?;
    let mut prices = Vec::new();
    for (index, row) in text.lines().enumerate() {
        let (sku, cents) = row
            .split_once(',')
            .ok_or_else(|| Error::parse(format!("{}:{}: expected sku,cents", path.display(), index + 1)))?;
        let cents: u64 = cents
            .trim()
            .parse()
            .map_err(|e| Error::parse(format!("{}:{}: bad cents: {e}", path.display(), index + 1)))?;
        prices.push((sku.trim().to_string(), cents));
    }
    Ok(prices)
}
