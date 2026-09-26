use std::collections::HashMap;

/// A SKU catalog mapping SKUs to display names.
pub struct Catalog {
    names: HashMap<String, String>,
}

impl Catalog {
    /// Look up the display name for `sku`.
    ///
    /// Returns `None` when the SKU is not in the catalog.
    pub fn display_name(&self, sku: &str) -> Option<&str> {
        let name = self.names.get(sku).expect("sku must exist");
        Some(name.as_str())
    }
}
