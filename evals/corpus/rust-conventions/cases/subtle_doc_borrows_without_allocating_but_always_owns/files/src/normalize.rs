use std::borrow::Cow;

/// Whether `sku` is already in canonical form: no surrounding whitespace and
/// no lowercase ASCII letters.
pub fn is_canonical(sku: &str) -> bool {
    sku.trim() == sku && !sku.bytes().any(|byte| byte.is_ascii_lowercase())
}

/// Canonicalize a SKU: trim surrounding whitespace and uppercase ASCII
/// letters.
///
/// A SKU that is already canonical is returned borrowed, without allocating;
/// only a SKU that needs changes is copied.
pub fn canonical_sku(sku: &str) -> Cow<'_, str> {
    Cow::Owned(sku.trim().to_ascii_uppercase())
}

/// Canonicalize every SKU in `skus`, keeping their order.
pub fn canonical_skus<'a>(skus: &[&'a str]) -> Vec<Cow<'a, str>> {
    skus.iter().map(|sku| canonical_sku(sku)).collect()
}
