"""Apply promotional discounts to a cart payload from the storefront API."""

from __future__ import annotations

from decimal import ROUND_HALF_UP, Decimal
from typing import Any

CENT = Decimal("0.01")


def _discounted(price: Decimal, percent: int) -> Decimal:
    return (price * (Decimal(100 - percent) / 100)).quantize(CENT, ROUND_HALF_UP)


def apply_discounts(cart: dict[str, Any], percent_by_sku: dict[str, int]) -> dict[str, Any]:
    """Return a copy of ``cart`` with per-SKU percentage discounts applied.

    The input ``cart`` is left unmodified, so callers may keep it as the
    pre-discount snapshot for the receipt.
    """
    result = dict(cart)
    for item in result["items"]:
        percent = percent_by_sku.get(item["sku"])
        if percent:
            item["unit_price"] = _discounted(Decimal(item["unit_price"]), percent)
    result["subtotal"] = sum(
        (Decimal(i["unit_price"]) * i["quantity"] for i in result["items"]),
        start=Decimal("0"),
    )
    return result


def discount_total(before: dict[str, Any], after: dict[str, Any]) -> Decimal:
    """Return how much the discounts saved: ``before`` subtotal minus ``after``."""
    return Decimal(before["subtotal"]) - Decimal(after["subtotal"])
