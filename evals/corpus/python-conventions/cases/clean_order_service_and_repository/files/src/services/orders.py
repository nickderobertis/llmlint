"""Order workflows: cancellation and open-order summaries."""

from __future__ import annotations

import logging
from decimal import Decimal

import psycopg

from src.repositories import orders as orders_repo
from src.services.notifications import notify_customer

logger = logging.getLogger(__name__)


class CancellationRejected(Exception):
    """Raised when an order cannot be cancelled because it is no longer open."""


def cancel_order(conn: psycopg.Connection, order_id: int) -> None:
    """Cancel an open order and notify its customer.

    Raises:
        OrderNotFound: if the order does not exist.
        CancellationRejected: if the order is no longer open.
    """
    order = orders_repo.get_order(conn, order_id)
    if not orders_repo.mark_cancelled(conn, order_id):
        logger.warning(
            "cancel rejected for order %s: status is %s", order_id, order.status
        )
        raise CancellationRejected(f"order {order_id} is {order.status}")
    try:
        notify_customer(order.customer_id, f"Order {order_id} was cancelled.")
    except ConnectionError:
        logger.exception(
            "cancellation notice for order %s to customer %s not sent",
            order_id,
            order.customer_id,
        )


def open_order_total(conn: psycopg.Connection, customer_id: int) -> Decimal:
    """Return the summed total of the customer's open orders (0 if none)."""
    orders = orders_repo.list_open_orders(conn, customer_id)
    return sum((o.total for o in orders), start=Decimal("0"))
