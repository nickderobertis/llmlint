"""Persistence for coupons."""

from __future__ import annotations


def get_by_code(conn, code: str):
    """Return the coupon row for ``code``, or None."""
    return conn.execute(
        "SELECT coupon_id, code, percent_off, max_redemptions, redeemed "
        "FROM coupons WHERE code = %s",
        (code,),
    ).fetchone()


def try_increment_redemptions(conn, coupon_id: int) -> bool:
    """Count one redemption; return False if the coupon is already exhausted."""
    cur = conn.execute(
        "UPDATE coupons SET redeemed = redeemed + 1 "
        "WHERE coupon_id = %s AND redeemed < max_redemptions",
        (coupon_id,),
    )
    return cur.rowcount == 1
