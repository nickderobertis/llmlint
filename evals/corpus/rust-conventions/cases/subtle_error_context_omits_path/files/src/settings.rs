use std::fs;
use std::path::Path;

use serde::Deserialize;

use crate::error::{Error, Result};

/// Storefront settings loaded from a TOML file.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Settings {
    pub currency: String,
    pub tax_region: String,
}

/// Load settings from the TOML file at `path`.
pub fn load_settings(path: &Path) -> Result<Settings> {
    let text = fs::read_to_string(path)
        .map_err(|e| Error::io("reading settings file".to_string(), e))?;
    toml::from_str(&text)
        .map_err(|e| Error::parse(format!("parsing settings {}: {e}", path.display())))
}
