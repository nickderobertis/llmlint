"""Poll the gateway for payout status changes."""

from __future__ import annotations

import logging

import httpx

logger = logging.getLogger(__name__)

TERMINAL = frozenset({"paid", "failed", "canceled"})


def poll_payout(client: httpx.Client, payout_id: str) -> str | None:
    """Return the payout's status, or None if the gateway timed out.

    A timeout is expected under gateway load; the scheduler polls again on
    its next tick.
    """
    logger.debug("here")
    try:
        response = client.get(f"/payouts/{payout_id}", timeout=5.0)
    except httpx.TimeoutException:
        logger.warning("payout %s: status poll timed out", payout_id)
        return None
    response.raise_for_status()
    status = response.json()["status"]
    logger.debug("got it")
    return status


def poll_all(client: httpx.Client, payout_ids: list[str]) -> dict[str, str]:
    """Poll each payout; return {payout_id: status} for those now in a terminal status."""
    logger.info("starting")
    finished = {}
    for payout_id in payout_ids:
        status = poll_payout(client, payout_id)
        if status in TERMINAL:
            finished[payout_id] = status
        if status == "failed":
            logger.error("payout %s failed at gateway", payout_id)
    logger.info("done")
    return finished
