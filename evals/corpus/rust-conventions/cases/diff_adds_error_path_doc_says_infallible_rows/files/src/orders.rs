use std::fs;
use std::path::Path;

use crate::error::{Error, Result};

/// An order row: id and quantity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderRow {
    pub id: String,
    pub quantity: u32,
}

/// Load order rows from `path`: `id,quantity` per line.
///
/// # Errors
///
/// Fails only when the file cannot be read. Malformed rows are skipped, never
/// reported.
pub fn load_orders(path: &Path) -> Result<Vec<OrderRow>> {
    let text = fs::read_to_string(path)
        .map_err(|e| Error::io(format!("reading orders {}", path.display()), e))?;
    let mut orders = Vec::new();
    for (index, row) in text.lines().enumerate() {
        let Some((id, quantity)) = row.split_once(',') else { continue };
        let Ok(quantity) = quantity.trim().parse::<u32>() else { continue };
        if quantity == 0 {
            return Err(Error::parse(format!(
                "{}:{}: order {} has zero quantity",
                path.display(),
                index + 1,
                id.trim()
            )));
        }
        orders.push(OrderRow { id: id.trim().to_string(), quantity });
    }
    Ok(orders)
}
