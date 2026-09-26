//! Plain-text invoice rendering: column layout, money formatting, tax lines,
//! and the totals block.

use std::fmt::Write as _;

/// Width of the description column, in characters.
pub const DESCRIPTION_WIDTH: usize = 32;
/// Width of each numeric column, in characters.
pub const AMOUNT_WIDTH: usize = 12;

/// One billed line on an invoice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvoiceLine {
    pub description: String,
    pub quantity: u32,
    pub unit_cents: u64,
}

impl InvoiceLine {
    /// Quantity times unit price, in cents.
    pub fn extended_cents(&self) -> u64 {
        self.unit_cents * u64::from(self.quantity)
    }
}

/// A tax applied to the subtotal, in basis points (1/100 of a percent).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaxRate {
    pub label: String,
    pub basis_points: u64,
}

/// Everything needed to render one invoice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invoice {
    pub number: String,
    pub customer: String,
    pub lines: Vec<InvoiceLine>,
    pub taxes: Vec<TaxRate>,
}

/// Format cents as dollars with a thousands separator, e.g. `$1,234.05`.
pub fn format_money(cents: u64) -> String {
    let dollars = cents / 100;
    let digits = dollars.to_string();
    let mut grouped = String::new();
    for (position, digit) in digits.chars().enumerate() {
        if position > 0 && (digits.len() - position) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    format!("${}.{:02}", grouped, cents % 100)
}

/// Truncate `text` to at most `width` characters, ending with `…` when cut.
pub fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let kept: String = text.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}…")
}

/// Pad `text` on the right to `width` characters.
fn pad_right(text: &str, width: usize) -> String {
    format!("{text:<width$}")
}

/// Pad `text` on the left to `width` characters.
fn pad_left(text: &str, width: usize) -> String {
    format!("{text:>width$}")
}

/// The column header row followed by a rule line of dashes.
pub fn header_row() -> String {
    let mut header = String::new();
    header.push_str(&pad_right("Description", DESCRIPTION_WIDTH));
    header.push_str(&pad_left("Qty", AMOUNT_WIDTH));
    header.push_str(&pad_left("Unit", AMOUNT_WIDTH));
    header.push_str(&pad_left("Amount", AMOUNT_WIDTH));
    header.push('\n');
    header.push_str(&"-".repeat(DESCRIPTION_WIDTH + 3 * AMOUNT_WIDTH));
    header.push('\n');
    header
}

/// Render one invoice line as a fixed-width row, truncating a long
/// description.
pub fn line_row(line: &InvoiceLine) -> String {
    format!(
        "{}{}{}{}\n",
        pad_right(&truncate(&line.description, DESCRIPTION_WIDTH), DESCRIPTION_WIDTH),
        pad_left(&line.quantity.to_string(), AMOUNT_WIDTH),
        pad_left(&format_money(line.unit_cents), AMOUNT_WIDTH),
        pad_left(&format_money(line.extended_cents()), AMOUNT_WIDTH),
    )
}

/// The invoice subtotal in cents: the sum of every line's extended amount.
pub fn subtotal_cents(invoice: &Invoice) -> u64 {
    invoice.lines.iter().map(InvoiceLine::extended_cents).sum()
}

/// The tax owed for `rate` on `subtotal`, rounded half up to the nearest cent.
pub fn tax_cents(subtotal: u64, rate: &TaxRate) -> u64 {
    (subtotal * rate.basis_points + 5_000) / 10_000
}

/// Every tax line's amount, in the invoice's tax order.
pub fn tax_amounts(invoice: &Invoice) -> Vec<(String, u64)> {
    let subtotal = subtotal_cents(invoice);
    invoice
        .taxes
        .iter()
        .map(|rate| (rate.label.clone(), tax_cents(subtotal, rate)))
        .collect()
}

/// The grand total in cents: subtotal plus every tax.
pub fn total_cents(invoice: &Invoice) -> u64 {
    subtotal_cents(invoice) + tax_amounts(invoice).iter().map(|(_, cents)| cents).sum::<u64>()
}

/// A right-aligned `label  amount` row for the totals block.
fn totals_row(label: &str, cents: u64) -> String {
    let label_width = DESCRIPTION_WIDTH + 2 * AMOUNT_WIDTH;
    format!("{}{}\n", pad_left(label, label_width), pad_left(&format_money(cents), AMOUNT_WIDTH))
}

/// The totals block: subtotal, one row per tax, then the grand total.
pub fn totals_block(invoice: &Invoice) -> String {
    let mut block = totals_row("Subtotal", subtotal_cents(invoice));
    for (label, cents) in tax_amounts(invoice) {
        block.push_str(&totals_row(&label, cents));
    }
    block.push_str(&totals_row("Total", total_cents(invoice)));
    block
}

/// Render the full invoice: a title line, the customer, the line table, and
/// the totals block. An invoice with no lines renders a `(no charges)` row in
/// place of the table body.
pub fn render_invoice(invoice: &Invoice) -> String {
    let mut document = String::new();
    let _ = writeln!(document, "INVOICE {}", invoice.number);
    let _ = writeln!(document, "Bill to: {}", invoice.customer);
    document.push('\n');
    document.push_str(&header_row());
    if invoice.lines.is_empty() {
        document.push_str("(no charges)\n");
    }
    for line in &invoice.lines {
        document.push_str(&line_row(line));
    }
    document.push('\n');
    document.push_str(&totals_block(invoice));
    document
}

/// Render a compact one-line summary per invoice: `number  customer  total`.
pub fn render_summary(invoices: &[Invoice]) -> String {
    let mut summary = String::new();
    for invoice in invoices {
        let tmp = format!(
            "{}  {}  {}\n",
            pad_right(&invoice.number, 12),
            pad_right(&truncate(&invoice.customer, 24), 24),
            pad_left(&format_money(total_cents(invoice)), AMOUNT_WIDTH),
        );
        summary.push_str(&tmp);
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money_groups_thousands() {
        assert_eq!(format_money(123_456_78), "$123,456.78");
        assert_eq!(format_money(5), "$0.05");
    }

    #[test]
    fn truncation_marks_the_cut() {
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("abc", 4), "abc");
    }

    #[test]
    fn tax_rounds_half_up() {
        let rate = TaxRate { label: "VAT".into(), basis_points: 1_250 };
        assert_eq!(tax_cents(1_000, &rate), 125);
        assert_eq!(tax_cents(4, &rate), 1);
    }
}
