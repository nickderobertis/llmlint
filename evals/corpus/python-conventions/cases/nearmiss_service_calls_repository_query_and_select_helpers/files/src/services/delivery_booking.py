"""Let shoppers select a delivery window at checkout."""

from __future__ import annotations

import logging
from datetime import date

from src.repositories import deliveries as deliveries_repo

logger = logging.getLogger(__name__)

SELECT_WINDOW_PROMPT = "Select a delivery window"
NO_WINDOWS_MESSAGE = "No delivery windows are left for this day. Please select another date."


class WindowUnavailable(Exception):
    """The chosen window filled up before the booking was made."""


def offer_windows(conn, postal_code: str, day: date) -> dict[str, object]:
    """Return the prompt and the open windows to render for ``postal_code``."""
    windows = deliveries_repo.query_open_windows(conn, postal_code[:3], day)
    return {
        "prompt": SELECT_WINDOW_PROMPT if windows else NO_WINDOWS_MESSAGE,
        "windows": [
            {"id": w.window_id, "label": f"{w.starts:%H:%M}-{w.ends:%H:%M}"} for w in windows
        ],
    }


def book(conn, order_id: int, window_id: int) -> None:
    """Book ``window_id`` for ``order_id``; rebooking the same window is a no-op.

    Raises:
        WindowUnavailable: if the window has no capacity left.
    """
    if deliveries_repo.select_booking_for_update(conn, order_id) == window_id:
        return
    if not deliveries_repo.execute_booking(conn, order_id, window_id):
        logger.warning("order %s could not book full window %s", order_id, window_id)
        raise WindowUnavailable(f"window {window_id} is full")
