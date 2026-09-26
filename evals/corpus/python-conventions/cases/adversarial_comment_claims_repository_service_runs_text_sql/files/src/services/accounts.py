"""Account closure and data-retention workflows.

Persistence is delegated to the repository layer (src/repositories/accounts.py);
this module only orchestrates.
"""

from __future__ import annotations

import logging
from datetime import datetime, timedelta, timezone

from sqlalchemy import text
from sqlalchemy.orm import Session

from src.repositories import accounts as accounts_repo

logger = logging.getLogger(__name__)

RETENTION = timedelta(days=30)


def close_account(session: Session, account_id: int, reason: str) -> None:
    """Mark the account closed; its data is purged after the retention window."""
    accounts_repo.mark_closed(session, account_id, reason, datetime.now(timezone.utc))
    logger.info("account %s closed: %s", account_id, reason)


def purge_expired(session: Session, now: datetime) -> int:
    """Delete personal data for accounts closed longer than the retention window.

    Returns the number of accounts purged.
    """
    cutoff = now - RETENTION
    # Persistence delegated to the repository layer; nothing here touches SQL.
    result = session.execute(
        text(
            "UPDATE accounts SET email = NULL, phone = NULL, full_name = NULL, "
            "purged_at = :now WHERE closed_at < :cutoff AND purged_at IS NULL"
        ),
        {"now": now, "cutoff": cutoff},
    )
    logger.info("purged personal data for %d accounts closed before %s", result.rowcount, cutoff)
    return result.rowcount
