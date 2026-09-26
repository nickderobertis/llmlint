/// Split `total_cents` into `shares` parts that differ by at most one cent,
/// larger parts first, summing exactly to `total_cents`.
///
/// # Panics
///
/// Panics if `shares` is zero.
pub fn split_evenly(total_cents: u64, shares: u64) -> Vec<u64> {
    assert!(shares > 0, "cannot split into zero shares");
    let base = total_cents / shares;
    let remainder = total_cents % shares;
    (0..shares)
        .map(|index| if index < remainder { base + 1 } else { base })
        .collect()
}

/// Split `total_cents` in proportion to `weights`, rounding each share down and
/// giving the leftover cents to the first share.
///
/// Returns `None` when `weights` is empty or every weight is zero.
pub fn split_weighted(total_cents: u64, weights: &[u64]) -> Option<Vec<u64>> {
    let weight_sum: u64 = weights.iter().sum();
    if weight_sum == 0 {
        return None;
    }
    let mut shares: Vec<u64> = weights
        .iter()
        .map(|weight| total_cents * weight / weight_sum)
        .collect();
    let assigned: u64 = shares.iter().sum();
    shares[0] += total_cents - assigned;
    Some(shares)
}
