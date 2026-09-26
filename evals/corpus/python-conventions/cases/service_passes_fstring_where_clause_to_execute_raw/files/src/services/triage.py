"""Support triage: work queues for agents and escalation sweeps."""

from __future__ import annotations

import logging
from datetime import datetime, timedelta

from src.repositories import tickets as tickets_repo

logger = logging.getLogger(__name__)

ESCALATION_AGE = {"urgent": timedelta(hours=1), "high": timedelta(hours=8)}


def agent_queue(conn, agent_id: int) -> list[tickets_repo.TicketRow]:
    """Return the agent's open tickets, oldest first."""
    return [t for t in tickets_repo.by_assignee(conn, agent_id) if t.status == "open"]


def tickets_to_escalate(conn, priority: str, now: datetime) -> list[tickets_repo.TicketRow]:
    """Return unassigned open tickets of ``priority`` older than its escalation age.

    Raises:
        KeyError: if ``priority`` has no escalation age.
    """
    cutoff = now - ESCALATION_AGE[priority]
    where = (
        f"status = 'open' AND assignee_id IS NULL "
        f"AND priority = '{priority}' AND opened_at < %s"
    )
    return tickets_repo.execute_raw(conn, where, (cutoff,))
