"""Subscription lifecycle: plan changes and pausing."""

from __future__ import annotations

import logging
from datetime import date, timedelta

from src.repositories import subscriptions as subscriptions_repo

logger = logging.getLogger(__name__)

PLANS = ("starter", "team", "enterprise")
MAX_PAUSE_DAYS = 90


class SubscriptionMissing(LookupError):
    """Raised when the subscription id does not exist."""


def change_plan(conn, subscription_id: int, plan: str) -> None:
    """Move the subscription to ``plan``.

    Raises:
        ValueError: if ``plan`` is unknown.
        SubscriptionMissing: if the subscription does not exist.
    """
    if plan not in PLANS:
        raise ValueError(f"unknown plan {plan!r}")
    if subscriptions_repo.get(conn, subscription_id) is None:
        raise SubscriptionMissing(f"subscription {subscription_id} not found")
    subscriptions_repo.set_plan(conn, subscription_id, plan)


def pause(conn, subscription_id: int, days: int, today: date) -> date:
    """Pause the subscription for ``days`` days and return the new renewal date.

    Raises:
        ValueError: if ``days`` is outside 1..90.
        SubscriptionMissing: if the subscription does not exist.
    """
    if not 1 <= days <= MAX_PAUSE_DAYS:
        raise ValueError(f"pause length must be 1..{MAX_PAUSE_DAYS} days, got {days}")
    sub = subscriptions_repo.get(conn, subscription_id)
    if sub is None:
        raise SubscriptionMissing(f"subscription {subscription_id} not found")
    renews_on = max(sub.renews_on, today) + timedelta(days=days)
    conn.execute(
        "UPDATE subscriptions SET status = 'paused', renews_on = %s "
        "WHERE subscription_id = %s",
        (renews_on, subscription_id),
    )
    return renews_on
