"""Build the per-user activity feed shown on the dashboard."""

from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime, timedelta
from typing import Iterable

MAX_FEED_ITEMS = 50


@dataclass(frozen=True, slots=True)
class Event:
    user_id: int
    kind: str
    occurred_at: datetime
    summary: str


def _is_visible(event: Event, hidden_kinds: frozenset[str]) -> bool:
    return event.kind not in hidden_kinds


def recent_events(
    events: Iterable[Event],
    *,
    now: datetime,
    window: timedelta = timedelta(days=7),
    hidden_kinds: frozenset[str] = frozenset({"heartbeat"}),
) -> list[Event]:
    """Return visible events from the last ``window``, newest first.

    At most ``MAX_FEED_ITEMS`` events are returned; ``hidden_kinds`` are dropped.
    """
    cutoff = now - window
    visible = [
        e for e in events if e.occurred_at >= cutoff and _is_visible(e, hidden_kinds)
    ]
    visible.sort(key=lambda e: e.occurred_at)
    return visible[:MAX_FEED_ITEMS]


def group_by_day(events: Iterable[Event]) -> dict[str, list[Event]]:
    """Group events by ISO calendar date, preserving their input order per day."""
    days: dict[str, list[Event]] = {}
    for event in events:
        days.setdefault(event.occurred_at.date().isoformat(), []).append(event)
    return days
