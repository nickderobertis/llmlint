"""Verify and decode inbound payment-provider webhooks."""

from __future__ import annotations

import hashlib
import hmac
import json
import time
from dataclasses import dataclass
from typing import Any

TOLERANCE_SECONDS = 300


class InvalidSignature(Exception):
    """The webhook signature header is missing, malformed, or does not match."""


class StaleWebhook(Exception):
    """The webhook timestamp is outside the replay tolerance window."""


@dataclass(frozen=True, slots=True)
class WebhookEvent:
    event_id: str
    event_type: str
    data: dict[str, Any]


def _parse_header(header: str) -> tuple[int, str]:
    parts = dict(p.split("=", 1) for p in header.split(",") if "=" in p)
    if "t" not in parts or "v1" not in parts:
        raise InvalidSignature("signature header lacks t= or v1=")
    try:
        return int(parts["t"]), parts["v1"]
    except ValueError:
        raise InvalidSignature(f"signature timestamp {parts['t']!r} is not an integer") from None


def _check_timestamp(timestamp: int, now: float) -> None:
    if abs(now - timestamp) > TOLERANCE_SECONDS:
        raise StaleWebhook(f"timestamp {timestamp} is {int(now - timestamp)}s from now")


def _check_signature(secret: bytes, timestamp: int, body: bytes, expected: str) -> None:
    signed = f"{timestamp}.".encode() + body
    digest = hmac.new(secret, signed, hashlib.sha256).hexdigest()
    if not hmac.compare_digest(digest, expected):
        raise InvalidSignature("signature does not match the payload")


def verify_webhook(
    secret: bytes, header: str, body: bytes, *, now: float | None = None
) -> WebhookEvent:
    """Verify ``body`` against its signature ``header`` and decode it.

    Raises:
        InvalidSignature: if the header is malformed or the HMAC does not match.
        StaleWebhook: if the signed timestamp is more than five minutes away
            from ``now``.
    """
    timestamp, expected = _parse_header(header)
    _check_timestamp(timestamp, time.time() if now is None else now)
    _check_signature(secret, timestamp, body, expected)
    payload = json.loads(body)
    return WebhookEvent(payload["id"], payload["type"], payload.get("data", {}))
