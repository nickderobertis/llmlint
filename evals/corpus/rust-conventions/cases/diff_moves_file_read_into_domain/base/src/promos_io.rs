use std::fs;
use std::path::Path;

use crate::domain::promo::{parse_promos, Promo};
use crate::error::{Error, Result};

/// Load promotions from the file at `path`.
pub fn load_promos(path: &Path) -> Result<Vec<Promo>> {
    let text = fs::read_to_string(path)
        .map_err(|e| Error::io(format!("reading promotions {}", path.display()), e))?;
    Ok(parse_promos(&text))
}
