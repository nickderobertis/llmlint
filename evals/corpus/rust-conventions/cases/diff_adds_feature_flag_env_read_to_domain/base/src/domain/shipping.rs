/// A parcel's weight in grams and destination zone (1 = domestic).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parcel {
    pub grams: u32,
    pub zone: u8,
}

/// Shipping cost in cents: a zone base rate plus 40 cents per started 500 g.
pub fn shipping_cents(parcel: &Parcel) -> u64 {
    let base = match parcel.zone {
        1 => 499,
        2 => 899,
        _ => 1_499,
    };
    let started_half_kilos = u64::from(parcel.grams).div_ceil(500);
    base + 40 * started_half_kilos
}

/// Whether the order qualifies for free shipping (a subtotal of at least
/// `threshold_cents`).
pub fn free_shipping(subtotal_cents: u64, threshold_cents: u64) -> bool {
    subtotal_cents >= threshold_cents
}
