"""Persistence for orders. All SQL for the ``orders`` table lives here."""

from __future__ import annotations

import logging
from dataclasses import dataclass
from datetime import datetime
from decimal import Decimal

import psycopg

logger = logging.getLogger(__name__)


@dataclass(frozen=True, slots=True)
class OrderRow:
    order_id: int
    customer_id: int
    status: str
    total: Decimal
    created_at: datetime


class OrderNotFound(LookupError):
    """Raised when no order exists with the requested id."""


def get_order(conn: psycopg.Connection, order_id: int) -> OrderRow:
    """Return the order with ``order_id``.

    Raises:
        OrderNotFound: if no such order exists.
    """
    row = conn.execute(
        "SELECT order_id, customer_id, status, total, created_at "
        "FROM orders WHERE order_id = %s",
        (order_id,),
    ).fetchone()
    if row is None:
        raise OrderNotFound(f"order {order_id} does not exist")
    return OrderRow(*row)


def list_open_orders(conn: psycopg.Connection, customer_id: int) -> list[OrderRow]:
    """Return the customer's open orders, newest first."""
    rows = conn.execute(
        "SELECT order_id, customer_id, status, total, created_at "
        "FROM orders WHERE customer_id = %s AND status = 'open' "
        "ORDER BY created_at DESC",
        (customer_id,),
    ).fetchall()
    return [OrderRow(*row) for row in rows]


def mark_cancelled(conn: psycopg.Connection, order_id: int) -> bool:
    """Set the order's status to ``cancelled``.

    Returns True if a row was updated, False if the order was already closed.
    """
    cur = conn.execute(
        "UPDATE orders SET status = 'cancelled' "
        "WHERE order_id = %s AND status = 'open'",
        (order_id,),
    )
    return cur.rowcount == 1
