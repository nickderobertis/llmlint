"""Persistence for coupons."""

from __future__ import annotations


def get_by_code(conn, code: str):
    """Return the coupon row for ``code``, or None."""
    return conn.execute(
        "SELECT coupon_id, code, percent_off, max_redemptions, redeemed "
        "FROM coupons WHERE code = %s",
        (code,),
    ).fetchone()
