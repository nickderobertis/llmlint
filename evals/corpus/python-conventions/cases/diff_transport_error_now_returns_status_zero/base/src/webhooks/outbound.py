"""Deliver outbound webhooks to customer endpoints."""

from __future__ import annotations

import hashlib
import hmac
import json
import logging
from dataclasses import dataclass

import httpx

logger = logging.getLogger(__name__)


@dataclass(frozen=True, slots=True)
class Delivery:
    delivery_id: str
    endpoint_url: str
    secret: bytes
    event: dict


def _signature(secret: bytes, body: bytes) -> str:
    return hmac.new(secret, body, hashlib.sha256).hexdigest()


def deliver(client: httpx.Client, delivery: Delivery) -> int:
    """POST the event to the endpoint and return the response status code.

    Raises:
        httpx.TransportError: if the endpoint cannot be reached; the caller's
            retry queue re-schedules the delivery.
    """
    body = json.dumps(delivery.event, separators=(",", ":")).encode()
    response = client.post(
        delivery.endpoint_url,
        content=body,
        headers={
            "Content-Type": "application/json",
            "X-Signature": _signature(delivery.secret, body),
            "X-Delivery-Id": delivery.delivery_id,
        },
        timeout=10.0,
    )
    logger.info("delivery %s answered %s", delivery.delivery_id, response.status_code)
    return response.status_code
