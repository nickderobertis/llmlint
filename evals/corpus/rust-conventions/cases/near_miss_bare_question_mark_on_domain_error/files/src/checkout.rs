use std::fs;
use std::path::Path;

use crate::error::{Error, Result};

/// A cart ready to check out: the subtotal in cents and an optional coupon.
pub struct Cart {
    pub subtotal_cents: u64,
    pub coupon_code: Option<String>,
}

/// A coupon definition: its code, percent off, and minimum subtotal.
pub struct Coupon {
    pub code: String,
    pub percent_off: u64,
    pub minimum_cents: u64,
}

/// Why a coupon could not be applied. These are business outcomes reported to
/// the shopper, not I/O or parse failures.
#[derive(Debug)]
pub enum CouponError {
    Unknown { code: String },
    BelowMinimum { code: String, minimum_cents: u64 },
}

impl From<CouponError> for Error {
    fn from(err: CouponError) -> Self {
        Error::Coupon(err)
    }
}

/// Apply the cart's coupon, if any, and return the amount due in cents.
///
/// Fails with `CouponError::Unknown` for a code not in `coupons`, and with
/// `CouponError::BelowMinimum` when the subtotal is under the coupon minimum.
pub fn amount_due(cart: &Cart, coupons: &[Coupon]) -> Result<u64> {
    let Some(code) = &cart.coupon_code else {
        return Ok(cart.subtotal_cents);
    };
    let coupon = find_coupon(coupons, code)?;
    check_minimum(cart, coupon)?;
    let percent = coupon.percent_off.min(100);
    Ok(cart.subtotal_cents * (100 - percent) / 100)
}

fn find_coupon<'a>(coupons: &'a [Coupon], code: &str) -> std::result::Result<&'a Coupon, CouponError> {
    coupons
        .iter()
        .find(|coupon| coupon.code == code)
        .ok_or_else(|| CouponError::Unknown { code: code.to_string() })
}

fn check_minimum(cart: &Cart, coupon: &Coupon) -> std::result::Result<(), CouponError> {
    if cart.subtotal_cents < coupon.minimum_cents {
        return Err(CouponError::BelowMinimum {
            code: coupon.code.clone(),
            minimum_cents: coupon.minimum_cents,
        });
    }
    Ok(())
}

/// Read the receipt footer shown under every checkout from `path`.
pub fn load_receipt_footer(path: &Path) -> Result<String> {
    fs::read_to_string(path)
        .map_err(|e| Error::io(format!("reading receipt footer {}", path.display()), e))
}
