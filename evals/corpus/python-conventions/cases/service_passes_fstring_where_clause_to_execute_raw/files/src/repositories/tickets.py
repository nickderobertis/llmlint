"""Persistence for support tickets."""

from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime
from typing import Any


@dataclass(frozen=True, slots=True)
class TicketRow:
    ticket_id: int
    requester_id: int
    assignee_id: int | None
    priority: str
    status: str
    opened_at: datetime


_COLUMNS = "ticket_id, requester_id, assignee_id, priority, status, opened_at"


def by_assignee(conn, assignee_id: int) -> list[TicketRow]:
    """Return the assignee's tickets, oldest first."""
    rows = conn.execute(
        f"SELECT {_COLUMNS} FROM tickets WHERE assignee_id = %s ORDER BY opened_at",
        (assignee_id,),
    ).fetchall()
    return [TicketRow(*r) for r in rows]


def execute_raw(conn, where: str, params: tuple[Any, ...] = ()) -> list[TicketRow]:
    """Escape hatch for ad-hoc reports: select tickets matching ``where``."""
    rows = conn.execute(f"SELECT {_COLUMNS} FROM tickets WHERE {where}", params).fetchall()
    return [TicketRow(*r) for r in rows]
