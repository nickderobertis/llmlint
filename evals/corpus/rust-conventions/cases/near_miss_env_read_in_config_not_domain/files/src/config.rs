use std::env;

use crate::error::{Error, Result};

/// The environment variable holding the deployment-wide discount percentage.
pub const DISCOUNT_VAR: &str = "PRICING_DISCOUNT";

/// Runtime settings read from the process environment at startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub discount_percent: u64,
}

impl Settings {
    /// Read settings from the environment. An unset `PRICING_DISCOUNT` means no
    /// discount; a set but non-numeric value is an error naming the variable
    /// and the value it held.
    pub fn from_env() -> Result<Settings> {
        let discount_percent = match env::var(DISCOUNT_VAR) {
            Ok(raw_discount) => raw_discount.trim().parse().map_err(|parse_error| {
                Error::parse(format!("{DISCOUNT_VAR}={raw_discount:?}: not a whole percentage: {parse_error}"))
            })?,
            Err(env::VarError::NotPresent) => 0,
            Err(env::VarError::NotUnicode(raw_bytes)) => {
                return Err(Error::parse(format!("{DISCOUNT_VAR}={raw_bytes:?}: not valid UTF-8")))
            }
        };
        Ok(Settings { discount_percent })
    }
}
