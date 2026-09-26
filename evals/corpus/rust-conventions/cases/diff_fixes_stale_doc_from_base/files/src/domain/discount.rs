/// Loyalty tiers, lowest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    Bronze,
    Silver,
    Gold,
}

/// The loyalty discount in percent: 2 for Bronze, 5 for Silver, and 10 for
/// Gold.
pub fn tier_discount(tier: Tier) -> u64 {
    match tier {
        Tier::Bronze => 2,
        Tier::Silver => 5,
        Tier::Gold => 10,
    }
}
