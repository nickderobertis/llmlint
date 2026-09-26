"""Issue refunds through the card gateway."""

from __future__ import annotations

import logging
from dataclasses import dataclass
from decimal import Decimal

import httpx

logger = logging.getLogger(__name__)


class RefundDeclined(Exception):
    """The gateway refused the refund; the message carries its reason code."""


@dataclass(frozen=True, slots=True)
class RefundRequest:
    refund_id: str
    payment_id: str
    amount: Decimal
    reason: str


def issue_refund(client: httpx.Client, request: RefundRequest) -> str:
    """Refund ``request.amount`` against ``request.payment_id``.

    Returns the gateway's refund reference.

    Raises:
        RefundDeclined: if the gateway answers 402 or 409.
        httpx.HTTPStatusError: for any other non-2xx response.
    """
    response = client.post(
        f"/payments/{request.payment_id}/refunds",
        json={
            "amount": str(request.amount),
            "reason": request.reason,
            "idempotency_key": request.refund_id,
        },
    )
    if response.status_code in (402, 409):
        code = response.json().get("decline_code", "unknown")
        logger.error("refund declined by gateway (decline code %s)", code)
        raise RefundDeclined(f"refund {request.refund_id} declined: {code}")
    response.raise_for_status()
    reference = response.json()["refund_reference"]
    logger.info(
        "refund %s for payment %s issued as %s",
        request.refund_id,
        request.payment_id,
        reference,
    )
    return reference
