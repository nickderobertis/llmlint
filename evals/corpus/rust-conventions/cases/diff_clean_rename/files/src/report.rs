/// Render each SKU and its total as a `sku: $d.cc` line.
pub fn render_totals(totals: &[(String, u64)]) -> String {
    let mut rendered = String::new();
    for (sku, cents) in totals {
        rendered.push_str(&format!("{sku}: ${}.{:02}\n", cents / 100, cents % 100));
    }
    rendered
}
