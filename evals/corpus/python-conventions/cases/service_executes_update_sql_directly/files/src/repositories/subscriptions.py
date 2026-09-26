"""Persistence for the ``subscriptions`` table."""

from __future__ import annotations

from dataclasses import dataclass
from datetime import date


@dataclass(frozen=True, slots=True)
class SubscriptionRow:
    subscription_id: int
    account_id: int
    plan: str
    renews_on: date
    status: str


def get(conn, subscription_id: int) -> SubscriptionRow | None:
    """Return the subscription, or None if it does not exist."""
    row = conn.execute(
        "SELECT subscription_id, account_id, plan, renews_on, status "
        "FROM subscriptions WHERE subscription_id = %s",
        (subscription_id,),
    ).fetchone()
    return None if row is None else SubscriptionRow(*row)


def set_plan(conn, subscription_id: int, plan: str) -> None:
    """Change the subscription's plan."""
    conn.execute(
        "UPDATE subscriptions SET plan = %s WHERE subscription_id = %s",
        (plan, subscription_id),
    )
