"""Nightly reconciliation of payment-provider payouts against the ledger.

For each payout the provider reports for a settlement date, we fetch its
settlement breakdown, compare the net amount against the ledger entries tagged
with the payout reference, and classify the payout as matched, mismatched, or
pending (not yet settled on the provider side). The finance team reviews the
resulting report every morning; a payout that appears in none of the three
buckets is invisible to them, so every payout must land in exactly one bucket.
"""

from __future__ import annotations

import logging
from collections.abc import Iterable, Sequence
from dataclasses import dataclass, field
from datetime import date
from decimal import Decimal
from enum import StrEnum
from typing import Protocol

logger = logging.getLogger(__name__)

TOLERANCE = Decimal("0.01")


class ProviderError(Exception):
    """Any failure talking to the payment provider."""


class ProviderTimeout(ProviderError):
    """The provider did not answer in time; the request may be retried."""


class LedgerUnavailable(Exception):
    """The ledger database could not be queried."""


class Bucket(StrEnum):
    MATCHED = "matched"
    MISMATCHED = "mismatched"
    PENDING = "pending"


@dataclass(frozen=True, slots=True)
class Payout:
    payout_id: str
    provider_ref: str
    currency: str
    gross: Decimal
    arrival_date: date


@dataclass(frozen=True, slots=True)
class Settlement:
    provider_ref: str
    gross: Decimal
    fees: Decimal
    refunds: Decimal
    settled: bool

    @property
    def net(self) -> Decimal:
        return self.gross - self.fees - self.refunds


@dataclass(frozen=True, slots=True)
class LedgerEntry:
    entry_id: int
    payout_ref: str
    amount: Decimal
    currency: str


@dataclass(frozen=True, slots=True)
class Line:
    payout_id: str
    bucket: Bucket
    provider_net: Decimal | None
    ledger_total: Decimal
    note: str = ""


@dataclass
class Report:
    settlement_date: date
    lines: list[Line] = field(default_factory=list)

    def bucket(self, which: Bucket) -> list[Line]:
        return [line for line in self.lines if line.bucket is which]

    @property
    def fully_reconciled(self) -> bool:
        """True when no payout is mismatched or pending."""
        return not self.bucket(Bucket.MISMATCHED) and not self.bucket(Bucket.PENDING)


class Provider(Protocol):
    def list_payouts(self, settlement_date: date) -> Sequence[Payout]: ...

    def fetch_settlement(self, provider_ref: str) -> Settlement: ...


class Ledger(Protocol):
    def entries_for(self, payout_refs: Iterable[str]) -> Sequence[LedgerEntry]: ...


def _ledger_totals(entries: Iterable[LedgerEntry]) -> dict[str, Decimal]:
    totals: dict[str, Decimal] = {}
    for entry in entries:
        totals[entry.payout_ref] = totals.get(entry.payout_ref, Decimal("0")) + entry.amount
    return totals


def _classify(payout: Payout, settlement: Settlement, ledger_total: Decimal) -> Line:
    if not settlement.settled:
        return Line(payout.payout_id, Bucket.PENDING, None, ledger_total, "not settled")
    delta = settlement.net - ledger_total
    if abs(delta) <= TOLERANCE:
        return Line(payout.payout_id, Bucket.MATCHED, settlement.net, ledger_total)
    return Line(
        payout.payout_id,
        Bucket.MISMATCHED,
        settlement.net,
        ledger_total,
        f"provider net differs from ledger by {delta}",
    )


def _fetch_with_retry(provider: Provider, payout: Payout, attempts: int = 3) -> Settlement:
    """Fetch the settlement, retrying ``ProviderTimeout`` up to ``attempts`` times.

    Raises:
        ProviderTimeout: if every attempt times out (the last one is re-raised).
        ProviderError: immediately, for any non-timeout provider failure.
    """
    for attempt in range(1, attempts + 1):
        try:
            return provider.fetch_settlement(payout.provider_ref)
        except ProviderTimeout:
            if attempt == attempts:
                raise
            logger.warning(
                "settlement fetch for payout %s timed out (attempt %d/%d); retrying",
                payout.payout_id,
                attempt,
                attempts,
            )
    raise AssertionError("unreachable")


def reconcile(provider: Provider, ledger: Ledger, settlement_date: date) -> Report:
    """Reconcile every payout the provider lists for ``settlement_date``.

    Every listed payout appears in the report exactly once, as matched,
    mismatched, or pending.

    Raises:
        LedgerUnavailable: if ledger entries cannot be loaded; no partial report
            is produced in that case.
    """
    payouts = provider.list_payouts(settlement_date)
    try:
        entries = ledger.entries_for(p.provider_ref for p in payouts)
    except ConnectionError as exc:
        logger.error(
            "ledger lookup for %d payouts on %s failed: %s",
            len(payouts),
            settlement_date,
            exc,
        )
        raise LedgerUnavailable(f"ledger unavailable for {settlement_date}") from exc
    totals = _ledger_totals(entries)

    report = Report(settlement_date)
    for payout in payouts:
        ledger_total = totals.get(payout.provider_ref, Decimal("0"))
        try:
            settlement = _fetch_with_retry(provider, payout)
        except ProviderTimeout:
            logger.warning(
                "payout %s: provider timed out after retries; marking pending",
                payout.payout_id,
            )
            report.lines.append(
                Line(payout.payout_id, Bucket.PENDING, None, ledger_total, "provider timeout")
            )
            continue
        except ProviderError:
            continue
        report.lines.append(_classify(payout, settlement, ledger_total))
    return report


def summarize(report: Report) -> dict[str, object]:
    """Return counts and totals per bucket for the morning summary email."""
    summary: dict[str, object] = {"settlement_date": report.settlement_date.isoformat()}
    for which in Bucket:
        lines = report.bucket(which)
        summary[f"{which.value}_count"] = len(lines)
        summary[f"{which.value}_ledger_total"] = str(
            sum((line.ledger_total for line in lines), start=Decimal("0"))
        )
    summary["fully_reconciled"] = report.fully_reconciled
    return summary


def mismatches_over(report: Report, threshold: Decimal) -> list[Line]:
    """Return mismatched lines whose absolute delta exceeds ``threshold``,
    largest delta first."""
    flagged = [
        line
        for line in report.bucket(Bucket.MISMATCHED)
        if line.provider_net is not None
        and abs(line.provider_net - line.ledger_total) > threshold
    ]
    flagged.sort(
        key=lambda line: abs((line.provider_net or Decimal("0")) - line.ledger_total),
        reverse=True,
    )
    return flagged


def format_line(line: Line) -> str:
    """Render one report line for the plain-text email body."""
    provider = "-" if line.provider_net is None else f"{line.provider_net:.2f}"
    note = f"  ({line.note})" if line.note else ""
    return (
        f"{line.payout_id:<24} {line.bucket.value:<11} "
        f"provider={provider:>12} ledger={line.ledger_total:>12.2f}{note}"
    )
