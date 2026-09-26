/// Render orders as CSV: `order_id,customer,total_cents` with a header row.
pub fn orders_csv(orders: &[(u64, String, u64)]) -> String {
    let mut csv = String::from("order_id,customer,total_cents\n");
    for (order_id, customer, total_cents) in orders {
        let tmp = customer.replace('"', "\"\"");
        csv.push_str(&format!("{order_id},\"{tmp}\",{total_cents}\n"));
    }
    csv
}
