//! Sales-tax computation. Rates arrive as a `RateTable` built by the caller;
//! nothing here decides where rates come from.

use std::collections::HashMap;
use std::path::Path;

/// Tax rates per region, in basis points (1/100 of a percent).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RateTable {
    rates: HashMap<String, u64>,
}

impl RateTable {
    /// Build a table from `(region, basis_points)` pairs; a later pair for the
    /// same region replaces an earlier one.
    pub fn from_pairs<I>(pairs: I) -> RateTable
    where
        I: IntoIterator<Item = (String, u64)>,
    {
        RateTable { rates: pairs.into_iter().collect() }
    }

    /// The rate for `region`, or `None` when the region is not taxed here.
    pub fn rate(&self, region: &str) -> Option<u64> {
        self.rates.get(region).copied()
    }

    /// How many regions have a rate.
    pub fn len(&self) -> usize {
        self.rates.len()
    }

    /// Whether the table has no rates.
    pub fn is_empty(&self) -> bool {
        self.rates.is_empty()
    }
}

/// A taxable line: its amount in cents and whether it is exempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaxableLine {
    pub amount_cents: u64,
    pub exempt: bool,
}

/// Tax on `amount_cents` at `basis_points`, rounded half up to a cent.
pub fn tax_on(amount_cents: u64, basis_points: u64) -> u64 {
    (amount_cents * basis_points + 5_000) / 10_000
}

/// The taxable base: the sum of every non-exempt line.
pub fn taxable_base(lines: &[TaxableLine]) -> u64 {
    lines
        .iter()
        .filter(|line| !line.exempt)
        .map(|line| line.amount_cents)
        .sum()
}

/// Tax owed on `lines` in `region` under `table`; 0 when the region has no
/// rate.
pub fn tax_owed(lines: &[TaxableLine], region: &str, table: &RateTable) -> u64 {
    match table.rate(region) {
        Some(basis_points) => tax_on(taxable_base(lines), basis_points),
        None => 0,
    }
}

/// A per-region breakdown of tax owed on the same lines, sorted by region.
pub fn breakdown(lines: &[TaxableLine], regions: &[&str], table: &RateTable) -> Vec<(String, u64)> {
    let mut owed: Vec<(String, u64)> = regions
        .iter()
        .map(|region| (region.to_string(), tax_owed(lines, region, table)))
        .collect();
    owed.sort();
    owed
}

/// Format basis points as a percentage string, e.g. `825` → `8.25%`.
pub fn format_rate(basis_points: u64) -> String {
    format!("{}.{:02}%", basis_points / 100, basis_points % 100)
}

/// The region with the highest rate in `table`, or `None` when empty. Ties go
/// to the alphabetically first region.
pub fn highest_rate_region(table: &RateTable) -> Option<(&str, u64)> {
    table
        .rates
        .iter()
        .map(|(region, rate)| (region.as_str(), *rate))
        .max_by(|left, right| left.1.cmp(&right.1).then_with(|| right.0.cmp(left.0)))
}

/// Tax owed on `lines` in `region`, using the rate table shipped with the
/// deployment when the caller has none of its own.
pub fn tax_owed_with_default_rates(lines: &[TaxableLine], region: &str) -> u64 {
    let shipped = crate::io::rates::load_rate_table(Path::new("config/tax_rates.csv"))
        .map(RateTable::from_pairs)
        .unwrap_or_default();
    tax_owed(lines, region, &shipped)
}

/// Whether a line of `amount_cents` should be flagged for review: large lines
/// in a high-rate region.
pub fn needs_review(amount_cents: u64, basis_points: u64) -> bool {
    amount_cents >= 1_000_000 && basis_points >= 1_000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exempt_lines_are_not_taxed() {
        let table = RateTable::from_pairs([("CA".to_string(), 825)]);
        let lines = [
            TaxableLine { amount_cents: 10_000, exempt: false },
            TaxableLine { amount_cents: 5_000, exempt: true },
        ];
        assert_eq!(tax_owed(&lines, "CA", &table), 825);
    }

    #[test]
    fn rates_format_as_percentages() {
        assert_eq!(format_rate(825), "8.25%");
        assert_eq!(format_rate(5), "0.05%");
    }
}
