use std::fs;
use std::path::Path;

use crate::error::{Error, Result};

/// A promotion: code and percent off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Promo {
    pub code: String,
    pub percent_off: u64,
}

/// Parse `code:percent` promotions, one per line; malformed lines are skipped.
pub fn parse_promos(text: &str) -> Vec<Promo> {
    text.lines()
        .filter_map(|row| {
            let (code, percent) = row.split_once(':')?;
            Some(Promo { code: code.trim().to_string(), percent_off: percent.trim().parse().ok()? })
        })
        .collect()
}

/// Load promotions from the file at `path`.
pub fn load_promos(path: &Path) -> Result<Vec<Promo>> {
    let text = fs::read_to_string(path)
        .map_err(|e| Error::io(format!("reading promotions {}", path.display()), e))?;
    Ok(parse_promos(&text))
}
