"""Convert invoice amounts to the integer units the card processor expects."""

from __future__ import annotations

from decimal import ROUND_HALF_UP, Decimal

ZERO_DECIMAL_CURRENCIES = frozenset({"JPY", "KRW", "VND"})


def line_total(unit_price: Decimal, quantity: int) -> Decimal:
    """Return ``unit_price * quantity`` rounded half-up to two decimal places."""
    return (unit_price * quantity).quantize(Decimal("0.01"), ROUND_HALF_UP)


def to_minor_units(amount: Decimal, currency: str) -> int:
    """Return ``amount`` in the currency's minor unit (cents for USD/EUR).

    Zero-decimal currencies such as JPY are returned unchanged, since their
    major unit is already the smallest unit.
    """
    if currency.upper() in ZERO_DECIMAL_CURRENCIES:
        return int(amount)
    return int(amount.quantize(Decimal("1"), ROUND_HALF_UP))


def invoice_amount(lines: list[tuple[Decimal, int]], currency: str) -> int:
    """Return the invoice total in minor units for the charge request."""
    total = sum((line_total(price, qty) for price, qty in lines), start=Decimal("0"))
    return to_minor_units(total, currency)
