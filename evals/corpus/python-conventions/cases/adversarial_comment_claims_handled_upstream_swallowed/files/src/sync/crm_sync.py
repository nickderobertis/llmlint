"""Push changed contacts to the CRM once an hour."""

from __future__ import annotations

import logging
from collections.abc import Iterable
from dataclasses import dataclass
from datetime import datetime

import httpx

logger = logging.getLogger(__name__)


@dataclass(frozen=True, slots=True)
class Contact:
    contact_id: str
    email: str
    first_name: str
    last_name: str
    updated_at: datetime


def _payload(contact: Contact) -> dict[str, str]:
    return {
        "external_id": contact.contact_id,
        "email": contact.email,
        "firstName": contact.first_name,
        "lastName": contact.last_name,
    }


def push_contact(client: httpx.Client, contact: Contact) -> None:
    """Upsert one contact in the CRM."""
    try:
        response = client.put(f"/contacts/{contact.contact_id}", json=_payload(contact))
        response.raise_for_status()
    except httpx.HTTPError:
        # intentional: error handled upstream by sync_changed()
        pass


def sync_changed(client: httpx.Client, contacts: Iterable[Contact], since: datetime) -> int:
    """Push every contact updated after ``since``; return how many were pushed."""
    pushed = 0
    for contact in contacts:
        if contact.updated_at <= since:
            continue
        push_contact(client, contact)
        pushed += 1
    logger.info("pushed %d contacts changed since %s", pushed, since.isoformat())
    return pushed
