"""Checkout orchestration: turn a cart into a paid order.

The flow is: validate the cart, price it (promotions, shipping, tax), reserve
stock, authorize payment, then create the order and capture. Any failure after
stock is reserved releases the reservation; any failure after authorization
voids it. Persistence goes through the repositories; payment goes through the
gateway client; this module holds only the ordering of steps and the business
rules between them.
"""

from __future__ import annotations

import logging
import uuid
from dataclasses import dataclass, field
from decimal import ROUND_HALF_UP, Decimal
from typing import Protocol

from src.repositories import carts as carts_repo
from src.repositories import inventory as inventory_repo
from src.repositories import orders as orders_repo
from src.repositories import promotions as promotions_repo

logger = logging.getLogger(__name__)

CENT = Decimal("0.01")
FREE_SHIPPING_THRESHOLD = Decimal("75.00")
FLAT_SHIPPING = Decimal("6.95")


class CheckoutError(Exception):
    """Base class for failures shown to the shopper."""


class EmptyCart(CheckoutError):
    """The cart has no line items."""


class OutOfStock(CheckoutError):
    """At least one SKU cannot be reserved in the requested quantity."""


class PaymentDeclined(CheckoutError):
    """The gateway declined the authorization."""


@dataclass(frozen=True, slots=True)
class CartLine:
    sku: str
    quantity: int
    unit_price: Decimal


@dataclass(frozen=True, slots=True)
class PricedCart:
    cart_id: str
    lines: tuple[CartLine, ...]
    subtotal: Decimal
    discount: Decimal
    shipping: Decimal
    tax: Decimal

    @property
    def total(self) -> Decimal:
        return self.subtotal - self.discount + self.shipping + self.tax


@dataclass
class CheckoutResult:
    order_id: str
    total: Decimal
    authorization_id: str
    warnings: list[str] = field(default_factory=list)


class Gateway(Protocol):
    def authorize(self, amount: Decimal, token: str, idempotency_key: str) -> str: ...

    def capture(self, authorization_id: str) -> None: ...

    def void(self, authorization_id: str) -> None: ...


class TaxService(Protocol):
    def tax_for(self, subtotal: Decimal, postal_code: str) -> Decimal: ...


def _money(value: Decimal) -> Decimal:
    return value.quantize(CENT, ROUND_HALF_UP)


def load_cart(conn, cart_id: str) -> tuple[CartLine, ...]:
    """Return the cart's lines.

    Raises:
        EmptyCart: if the cart has no lines.
    """
    lines = tuple(
        CartLine(row.sku, row.quantity, row.unit_price)
        for row in carts_repo.lines_for(conn, cart_id)
    )
    if not lines:
        raise EmptyCart(f"cart {cart_id} is empty")
    return lines


def price_cart(
    conn, cart_id: str, lines: tuple[CartLine, ...], postal_code: str, tax: TaxService
) -> PricedCart:
    """Price ``lines``: subtotal, best single promotion, shipping, and tax.

    Shipping is free at or above ``FREE_SHIPPING_THRESHOLD`` after discount.
    """
    subtotal = _money(sum((l.unit_price * l.quantity for l in lines), start=Decimal("0")))
    promotions = promotions_repo.active_for_skus(conn, [l.sku for l in lines])
    discount = max(
        (_money(subtotal * p.percent_off / 100) for p in promotions if subtotal >= p.minimum),
        default=Decimal("0"),
    )
    after_discount = subtotal - discount
    shipping = Decimal("0") if after_discount >= FREE_SHIPPING_THRESHOLD else FLAT_SHIPPING
    return PricedCart(
        cart_id,
        lines,
        subtotal,
        discount,
        shipping,
        _money(tax.tax_for(after_discount, postal_code)),
    )


def reserve_stock(conn, cart: PricedCart) -> str:
    """Reserve every line's quantity and return the reservation id.

    Reservation is all-or-nothing: rows are locked in SKU order to avoid
    deadlocks between concurrent checkouts.

    Raises:
        OutOfStock: naming the first SKU that is short.
    """
    reservation_id = str(uuid.uuid4())
    skus = sorted(line.sku for line in cart.lines)
    locked = conn.execute(
        "SELECT sku, on_hand - reserved FROM stock WHERE sku = ANY(%s) "
        "ORDER BY sku FOR UPDATE",
        (skus,),
    ).fetchall()
    available = dict(locked)
    for line in cart.lines:
        if available.get(line.sku, 0) < line.quantity:
            raise OutOfStock(f"{line.sku}: {available.get(line.sku, 0)} left")
    inventory_repo.create_reservation(
        conn, reservation_id, [(l.sku, l.quantity) for l in cart.lines]
    )
    return reservation_id


def checkout(
    conn,
    gateway: Gateway,
    tax: TaxService,
    *,
    cart_id: str,
    payment_token: str,
    postal_code: str,
) -> CheckoutResult:
    """Run the full checkout for ``cart_id``.

    Raises:
        EmptyCart, OutOfStock, PaymentDeclined: shopper-facing failures; no
            stock stays reserved and no authorization stays open.
    """
    lines = load_cart(conn, cart_id)
    cart = price_cart(conn, cart_id, lines, postal_code, tax)
    reservation_id = reserve_stock(conn, cart)
    try:
        authorization_id = gateway.authorize(cart.total, payment_token, idempotency_key=cart_id)
    except PaymentDeclined:
        inventory_repo.release_reservation(conn, reservation_id)
        logger.info("payment declined for cart %s; released %s", cart_id, reservation_id)
        raise
    except Exception:
        inventory_repo.release_reservation(conn, reservation_id)
        logger.exception(
            "authorization for cart %s (%s) failed; released reservation %s",
            cart_id,
            cart.total,
            reservation_id,
        )
        raise

    try:
        order_id = orders_repo.create_from_cart(conn, cart_id, reservation_id, cart.total)
        gateway.capture(authorization_id)
    except Exception:
        gateway.void(authorization_id)
        inventory_repo.release_reservation(conn, reservation_id)
        logger.exception(
            "order creation/capture for cart %s failed; voided authorization %s",
            cart_id,
            authorization_id,
        )
        raise

    carts_repo.mark_checked_out(conn, cart_id, order_id)
    result = CheckoutResult(order_id, cart.total, authorization_id)
    if cart.shipping and cart.subtotal - cart.discount > FREE_SHIPPING_THRESHOLD - Decimal("10"):
        result.warnings.append("Add a little more to get free shipping next time.")
    logger.info("cart %s checked out as order %s for %s", cart_id, order_id, cart.total)
    return result
