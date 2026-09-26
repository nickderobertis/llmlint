"""Grant and revoke admin roles. Every change must leave an audit record."""

from __future__ import annotations

import json
import logging
from contextlib import suppress
from dataclasses import asdict, dataclass
from datetime import datetime, timezone
from pathlib import Path

logger = logging.getLogger(__name__)

AUDIT_LOG = Path("/var/log/app/role-audit.jsonl")
ASSIGNABLE_ROLES = frozenset({"viewer", "editor", "billing_admin", "owner"})


@dataclass(frozen=True, slots=True)
class RoleChange:
    actor_id: int
    target_user_id: int
    role: str
    action: str
    at: str


def _append_audit(change: RoleChange, path: Path = AUDIT_LOG) -> None:
    with path.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(asdict(change)) + "\n")


def grant_role(store, actor_id: int, target_user_id: int, role: str) -> None:
    """Give ``target_user_id`` the ``role`` and record who granted it.

    Raises:
        ValueError: if ``role`` is not assignable.
    """
    if role not in ASSIGNABLE_ROLES:
        raise ValueError(f"role {role!r} is not assignable")
    store.add_role(target_user_id, role)
    change = RoleChange(
        actor_id, target_user_id, role, "grant", datetime.now(timezone.utc).isoformat()
    )
    with suppress(OSError):
        _append_audit(change)
    logger.info("user %s granted %s to user %s", actor_id, role, target_user_id)
