/// A line item: a SKU, its unit price in cents, and a quantity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineItem {
    pub sku: String,
    pub unit_cents: u64,
    pub quantity: u32,
}

/// Sort line items by total price, cheapest first.
pub fn sort_by_total_desc(items: &mut [LineItem]) {
    items.sort_by(|a, b| total_cents(b).cmp(&total_cents(a)));
}

/// The line item's total price in cents (unit price times quantity).
pub fn total_cents(item: &LineItem) -> u64 {
    item.unit_cents * u64::from(item.quantity)
}

/// Apply a percentage discount, rounding down to whole cents. A discount above
/// 100 percent is clamped to 100, so a total never goes negative.
pub fn discounted(total: u64, percent: u64) -> u64 {
    let percent = percent.min(100);
    total * (100 - percent) / 100
}
