/// A line item: a SKU, its unit price in cents, and a quantity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineItem {
    pub sku: String,
    pub unit_cents: u64,
    pub quantity: u32,
}

/// The line item's total price in cents (unit price times quantity).
pub fn total_cents(item: &LineItem) -> u64 {
    item.unit_cents * u64::from(item.quantity)
}

/// The order total in cents after applying `discount_percent`, rounding down to
/// whole cents. A discount above 100 percent is clamped to 100.
///
/// The discount is passed in by the caller (see `config::Settings`), so this
/// function never consults the environment itself.
pub fn order_total(items: &[LineItem], discount_percent: u64) -> u64 {
    let subtotal: u64 = items.iter().map(total_cents).sum();
    let percent = discount_percent.min(100);
    subtotal * (100 - percent) / 100
}
