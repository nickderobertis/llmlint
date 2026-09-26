use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A coupon code and the Unix time (seconds) after which it is invalid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coupon {
    pub code: String,
    pub expires_at: u64,
}

/// Seconds since the Unix epoch for `instant`; instants before the epoch map
/// to 0.
fn unix_seconds(instant: SystemTime) -> u64 {
    instant
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// Whether the coupon has expired as of `now`. The caller supplies the clock,
/// so this stays deterministic under test.
pub fn is_expired(coupon: &Coupon, now: SystemTime) -> bool {
    unix_seconds(now) > coupon.expires_at
}

/// How long until the coupon expires as of `now`, or `None` when it already
/// has.
pub fn time_remaining(coupon: &Coupon, now: SystemTime) -> Option<Duration> {
    coupon
        .expires_at
        .checked_sub(unix_seconds(now))
        .map(Duration::from_secs)
}

/// The coupons from `coupons` still valid as of `now`, in their original order.
pub fn still_valid<'a>(coupons: &'a [Coupon], now: SystemTime) -> Vec<&'a Coupon> {
    coupons.iter().filter(|coupon| !is_expired(coupon, now)).collect()
}
