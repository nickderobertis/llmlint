"""Persistence for delivery windows and bookings."""

from __future__ import annotations

from dataclasses import dataclass
from datetime import date, time


@dataclass(frozen=True, slots=True)
class WindowRow:
    window_id: int
    day: date
    starts: time
    ends: time
    capacity: int
    booked: int


def query_open_windows(conn, postal_prefix: str, day: date) -> list[WindowRow]:
    """Return windows on ``day`` for ``postal_prefix`` with spare capacity."""
    rows = conn.execute(
        "SELECT window_id, day, starts, ends, capacity, booked FROM delivery_windows "
        "WHERE postal_prefix = %s AND day = %s AND booked < capacity ORDER BY starts",
        (postal_prefix, day),
    ).fetchall()
    return [WindowRow(*r) for r in rows]


def select_booking_for_update(conn, order_id: int) -> int | None:
    """Lock and return the order's booked window id, or None if unbooked."""
    row = conn.execute(
        "SELECT window_id FROM bookings WHERE order_id = %s FOR UPDATE", (order_id,)
    ).fetchone()
    return None if row is None else row[0]


def execute_booking(conn, order_id: int, window_id: int) -> bool:
    """Book ``window_id`` for ``order_id`` if it still has capacity."""
    cur = conn.execute(
        "UPDATE delivery_windows SET booked = booked + 1 "
        "WHERE window_id = %s AND booked < capacity",
        (window_id,),
    )
    if cur.rowcount != 1:
        return False
    conn.execute(
        "INSERT INTO bookings (order_id, window_id) VALUES (%s, %s)", (order_id, window_id)
    )
    return True
