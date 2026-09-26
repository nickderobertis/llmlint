/// A promotion: code and percent off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Promo {
    pub code: String,
    pub percent_off: u64,
}

/// Parse `code:percent` promotions, one per line; malformed lines are skipped.
pub fn parse_promos(text: &str) -> Vec<Promo> {
    text.lines()
        .filter_map(|row| {
            let (code, percent) = row.split_once(':')?;
            Some(Promo { code: code.trim().to_string(), percent_off: percent.trim().parse().ok()? })
        })
        .collect()
}
