"""Import bank statement CSV exports into ledger transactions."""

from __future__ import annotations

import csv
import logging
from collections.abc import Iterator
from dataclasses import dataclass
from datetime import date, datetime
from decimal import Decimal, InvalidOperation
from pathlib import Path

logger = logging.getLogger(__name__)


class StatementFormatError(Exception):
    """Raised when a statement row cannot be interpreted."""


@dataclass(frozen=True, slots=True)
class Transaction:
    posted_on: date
    description: str
    amount: Decimal
    reference: str


def _parse_date(raw: str, row_number: int) -> date:
    try:
        return datetime.strptime(raw.strip(), "%d/%m/%Y").date()
    except ValueError as exc:
        raise StatementFormatError(f"row {row_number}: bad date {raw!r}") from exc


def _parse_amount(raw: str) -> Decimal:
    cleaned = raw.replace(",", "").replace("£", "").strip()
    try:
        return Decimal(cleaned)
    except InvalidOperation:
        return Decimal("0")


def read_statement(path: Path) -> Iterator[Transaction]:
    """Yield one transaction per data row of the CSV at ``path``.

    Raises:
        StatementFormatError: if a row has an unparseable date.
    """
    with path.open(newline="", encoding="utf-8-sig") as handle:
        for row_number, row in enumerate(csv.DictReader(handle), start=2):
            yield Transaction(
                posted_on=_parse_date(row["Date"], row_number),
                description=row["Description"].strip(),
                amount=_parse_amount(row["Amount"]),
                reference=row.get("Reference", "").strip(),
            )
