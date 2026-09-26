/// Render each SKU and its total as a `sku: $d.cc` line.
pub fn do_it(data: &[(String, u64)]) -> String {
    let mut tmp = String::new();
    for (sku, cents) in data {
        let val = format!("{sku}: ${}.{:02}\n", cents / 100, cents % 100);
        tmp.push_str(&val);
    }
    tmp
}
