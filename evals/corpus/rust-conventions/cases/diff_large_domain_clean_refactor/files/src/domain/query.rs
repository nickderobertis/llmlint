//! Catalog search: filtering, ordering, and cursor pagination over an
//! in-memory product list.

use std::cmp::Ordering;

/// A product as the search layer sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct Product {
    pub id: u64,
    pub name: String,
    pub category: String,
    pub price_cents: u64,
    pub rating: f32,
    pub in_stock: bool,
}

/// How to order search results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    /// Cheapest first.
    PriceAscending,
    /// Most expensive first.
    PriceDescending,
    /// Highest rated first.
    Rating,
    /// Alphabetical by name.
    Name,
}

/// The filters a shopper can combine.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Filter {
    /// Only products in this category (exact match).
    pub category: Option<String>,
    /// Only products at or under this price.
    pub max_price_cents: Option<u64>,
    /// Only products rated at least this.
    pub min_rating: Option<f32>,
    /// Hide out-of-stock products.
    pub in_stock_only: bool,
    /// Only products whose name contains this text, ignoring ASCII case.
    pub name_contains: Option<String>,
}

impl Filter {
    /// Whether `product` passes every filter that is set.
    pub fn matches(&self, product: &Product) -> bool {
        let category_ok = self.category.as_ref().map_or(true, |category| &product.category == category);
        let price_ok = self.max_price_cents.map_or(true, |max_price| product.price_cents <= max_price);
        let rating_ok = self.min_rating.map_or(true, |min_rating| product.rating >= min_rating);
        let stock_ok = !self.in_stock_only || product.in_stock;
        let name_ok = self.name_contains.as_ref().map_or(true, |needle| {
            product.name.to_ascii_lowercase().contains(&needle.to_ascii_lowercase())
        });
        category_ok && price_ok && rating_ok && stock_ok && name_ok
    }
}

/// Compare two products under `key`, breaking ties by ascending id so the
/// order is total and stable across pages.
pub fn compare(key: SortKey, left: &Product, right: &Product) -> Ordering {
    let primary = match key {
        SortKey::PriceAscending => left.price_cents.cmp(&right.price_cents),
        SortKey::PriceDescending => right.price_cents.cmp(&left.price_cents),
        SortKey::Rating => right.rating.partial_cmp(&left.rating).unwrap_or(Ordering::Equal),
        SortKey::Name => left.name.cmp(&right.name),
    };
    primary.then_with(|| left.id.cmp(&right.id))
}

/// Every product passing `filter`, ordered by `key`.
pub fn search<'a>(products: &'a [Product], filter: &Filter, key: SortKey) -> Vec<&'a Product> {
    let mut matching: Vec<&Product> = products.iter().filter(|product| filter.matches(product)).collect();
    matching.sort_by(|left, right| compare(key, left, right));
    matching
}

/// Distinct categories among `products`, sorted alphabetically.
pub fn categories(products: &[Product]) -> Vec<&str> {
    let mut names: Vec<&str> = products.iter().map(|product| product.category.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    names
}

/// The average rating of `products`, or `None` for an empty slice.
pub fn average_rating(products: &[&Product]) -> Option<f32> {
    if products.is_empty() {
        return None;
    }
    let sum: f32 = products.iter().map(|product| product.rating).sum();
    Some(sum / products.len() as f32)
}

/// Where a page starting after `after_id` begins in `ordered`: 0 for `None`,
/// the index just past that product, or `None` when the id is not present.
fn start_index(ordered: &[&Product], after_id: Option<u64>) -> Option<usize> {
    match after_id {
        None => Some(0),
        Some(id) => ordered.iter().position(|product| product.id == id).map(|position| position + 1),
    }
}

/// One page of ordered results, starting after the product with id
/// `after_id` (or from the beginning when `None`).
///
/// Returns at most `limit` products. When `after_id` is not in `ordered`, the
/// page is empty.
pub fn page<'a>(ordered: &[&'a Product], after_id: Option<u64>, limit: usize) -> Vec<&'a Product> {
    match start_index(ordered, after_id) {
        Some(start) => ordered.iter().skip(start).take(limit).copied().collect(),
        None => Vec::new(),
    }
}

/// The cursor for the page after `current`: the id of its last product, or
/// `None` when the page is empty.
pub fn next_cursor(current: &[&Product]) -> Option<u64> {
    current.last().map(|product| product.id)
}

/// Price range (cheapest, dearest) across `products`, or `None` when empty.
pub fn price_range(products: &[&Product]) -> Option<(u64, u64)> {
    let cheapest = products.iter().map(|product| product.price_cents).min()?;
    let dearest = products.iter().map(|product| product.price_cents).max()?;
    Some((cheapest, dearest))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn product(id: u64, price_cents: u64) -> Product {
        Product {
            id,
            name: format!("product {id}"),
            category: "tools".into(),
            price_cents,
            rating: 4.0,
            in_stock: true,
        }
    }

    #[test]
    fn ties_break_by_id() {
        let catalog = [product(2, 100), product(1, 100)];
        let ordered = search(&catalog, &Filter::default(), SortKey::PriceAscending);
        assert_eq!(ordered[0].id, 1);
    }

    #[test]
    fn unknown_cursor_is_an_empty_page() {
        let catalog = [product(1, 100)];
        let ordered = search(&catalog, &Filter::default(), SortKey::Name);
        assert!(page(&ordered, Some(99), 10).is_empty());
    }
}
