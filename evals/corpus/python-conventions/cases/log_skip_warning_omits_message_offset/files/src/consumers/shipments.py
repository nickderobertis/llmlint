"""Consume shipment status events and update tracking records."""

from __future__ import annotations

import json
import logging
from dataclasses import dataclass
from datetime import datetime

logger = logging.getLogger(__name__)

KNOWN_STATUSES = frozenset({"label_created", "in_transit", "out_for_delivery", "delivered"})


@dataclass(frozen=True, slots=True)
class ShipmentEvent:
    shipment_id: str
    status: str
    occurred_at: datetime


def decode(raw: bytes) -> ShipmentEvent:
    """Decode one Kafka message value.

    Raises:
        ValueError: if the value is not JSON, lacks a field, or names an
            unknown status.
    """
    try:
        body = json.loads(raw)
        event = ShipmentEvent(
            shipment_id=body["shipment_id"],
            status=body["status"],
            occurred_at=datetime.fromisoformat(body["occurred_at"]),
        )
    except (json.JSONDecodeError, KeyError) as exc:
        raise ValueError(f"malformed shipment event: {exc}") from exc
    if event.status not in KNOWN_STATUSES:
        raise ValueError(f"shipment {event.shipment_id}: unknown status {event.status!r}")
    return event


def handle_batch(messages, tracking_store) -> int:
    """Apply each message in ``messages`` to ``tracking_store``.

    Malformed messages are skipped with a warning so one bad producer cannot
    stall the partition. Returns the number of events applied.
    """
    applied = 0
    for message in messages:
        try:
            event = decode(message.value)
        except ValueError:
            logger.warning("could not process message, skipping")
            continue
        tracking_store.update_status(event.shipment_id, event.status, event.occurred_at)
        applied += 1
    return applied
