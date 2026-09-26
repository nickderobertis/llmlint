use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::error::{Error, Result};

/// Read the tax-rate table from `path`: `region,basis_points` rows.
pub fn load_rate_table(path: &Path) -> Result<HashMap<String, u64>> {
    let text = fs::read_to_string(path)
        .map_err(|e| Error::io(format!("reading tax rates {}", path.display()), e))?;
    let mut rates = HashMap::new();
    for (index, row) in text.lines().enumerate() {
        let Some((region, basis_points)) = row.split_once(',') else {
            continue;
        };
        let basis_points: u64 = basis_points.trim().parse().map_err(|parse_error| {
            Error::parse(format!("{}:{}: bad basis points: {parse_error}", path.display(), index + 1))
        })?;
        rates.insert(region.trim().to_string(), basis_points);
    }
    Ok(rates)
}
