"""Data for the collections team's overdue-invoices dashboard."""

from __future__ import annotations

import logging
from dataclasses import dataclass
from datetime import date
from decimal import Decimal

from src.repositories import invoices as invoices_repo

logger = logging.getLogger(__name__)


@dataclass(frozen=True, slots=True)
class OverdueRow:
    invoice_id: int
    customer_name: str
    amount_due: Decimal
    days_overdue: int


def overdue_invoices(db, *, as_of: date, min_days: int = 1) -> list[OverdueRow]:
    """Return invoices at least ``min_days`` past due as of ``as_of``, most
    overdue first. The collections team works this list top-down."""
    try:
        invoices = invoices_repo.unpaid_before(db, as_of)
    except Exception:
        return []
    rows = [
        OverdueRow(
            invoice_id=inv.invoice_id,
            customer_name=inv.customer_name,
            amount_due=inv.amount_due,
            days_overdue=(as_of - inv.due_date).days,
        )
        for inv in invoices
        if (as_of - inv.due_date).days >= min_days
    ]
    rows.sort(key=lambda r: r.days_overdue, reverse=True)
    return rows


def total_overdue(rows: list[OverdueRow]) -> Decimal:
    """Return the summed amount due across ``rows``."""
    return sum((r.amount_due for r in rows), start=Decimal("0"))
