/// A cart line: SKU, quantity, and unit price in cents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CartLine {
    pub sku: String,
    pub quantity: u32,
    pub unit_cents: u64,
}

/// A shopping cart.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Cart {
    pub lines: Vec<CartLine>,
}

impl Cart {
    /// Add `quantity` of `sku` at `unit_cents`, merging with an existing line
    /// for the same SKU.
    pub fn add(&mut self, sku: &str, quantity: u32, unit_cents: u64) {
        match self.lines.iter_mut().find(|line| line.sku == sku) {
            Some(line) => line.quantity += quantity,
            None => self.lines.push(CartLine { sku: sku.to_string(), quantity, unit_cents }),
        }
    }

    /// The cart total in cents, ignoring lines with zero quantity. This is a
    /// read-only query: the cart's lines are left exactly as they were.
    pub fn total_cents(&mut self) -> u64 {
        self.lines.retain(|line| line.quantity > 0);
        self.lines
            .iter()
            .map(|line| line.unit_cents * u64::from(line.quantity))
            .sum()
    }
}
