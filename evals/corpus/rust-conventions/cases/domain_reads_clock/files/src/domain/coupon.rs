use std::time::{SystemTime, UNIX_EPOCH};

/// A coupon code and the Unix time (seconds) after which it is invalid.
pub struct Coupon {
    pub code: String,
    pub expires_at: u64,
}

/// Whether the coupon has expired.
pub fn is_expired(coupon: &Coupon) -> bool {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    now > coupon.expires_at
}
