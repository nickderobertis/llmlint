"""Coupon redemption at checkout."""

from __future__ import annotations

import logging

from src.repositories import coupons as coupons_repo

logger = logging.getLogger(__name__)


class CouponRejected(Exception):
    """The coupon code is unknown or fully redeemed."""


def redeem(conn, code: str, order_id: int) -> int:
    """Redeem ``code`` for ``order_id`` and return its percent off.

    Raises:
        CouponRejected: if the code is unknown or has no redemptions left.
    """
    coupon = coupons_repo.get_by_code(conn, code)
    if coupon is None:
        raise CouponRejected(f"unknown coupon {code!r}")
    cur = conn.execute(
        "UPDATE coupons SET redeemed = redeemed + 1 "
        "WHERE coupon_id = %s AND redeemed < max_redemptions",
        (coupon.coupon_id,),
    )
    if cur.rowcount != 1:
        logger.warning("coupon %s exhausted before order %s could redeem it", code, order_id)
        raise CouponRejected(f"coupon {code!r} has no redemptions left")
    return coupon.percent_off
