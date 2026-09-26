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
        if let Some(category) = &self.category {
            if &product.category != category {
                return false;
            }
        }
        if let Some(max_price) = self.max_price_cents {
            if product.price_cents > max_price {
                return false;
            }
        }
        if let Some(min_rating) = self.min_rating {
            if product.rating < min_rating {
                return false;
            }
        }
        if self.in_stock_only && !product.in_stock {
            return false;
        }
        if let Some(needle) = &self.name_contains {
            let name = product.name.to_ascii_lowercase();
            if !name.contains(&needle.to_ascii_lowercase()) {
                return false;
            }
        }
        true
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
    let mut hits: Vec<&Product> = products.iter().filter(|product| filter.matches(product)).collect();
    hits.sort_by(|left, right| compare(key, left, right));
    hits
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

/// One page of ordered results, starting after the product with id
/// `after_id` (or from the beginning when `None`).
///
/// Returns at most `limit` products. When `after_id` is not in `ordered`, the
/// page is empty.
pub fn page<'a>(ordered: &[&'a Product], after_id: Option<u64>, limit: usize) -> Vec<&'a Product> {
    let start = match after_id {
        None => 0,
        Some(id) => match ordered.iter().position(|product| product.id == id) {
            Some(position) => position + 1,
            None => return Vec::new(),
        },
    };
    ordered.iter().skip(start).take(limit + 1).copied().collect()
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
