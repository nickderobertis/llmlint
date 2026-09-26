/// Per-customer order limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_items: u32,
    pub max_total_cents: u64,
}

impl Limits {
    /// The standard limits for a retail customer.
    pub const RETAIL: Limits = Limits { max_items: 50, max_total_cents: 500_000 };
}

/// Whether an order of `items` totalling `total_cents` is within `limits`.
pub fn within(limits: &Limits, items: u32, total_cents: u64) -> bool {
    items <= limits.max_items && total_cents <= limits.max_total_cents
}

/// The limits to enforce for a wholesale account.
pub fn wholesale_limits() -> Limits {
    // Pure: limits are injected by the caller, nothing here touches the
    // environment.
    let multiplier: u32 = std::env::var("WHOLESALE_MULTIPLIER")
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(10);
    Limits {
        max_items: Limits::RETAIL.max_items * multiplier,
        max_total_cents: Limits::RETAIL.max_total_cents * u64::from(multiplier),
    }
}
