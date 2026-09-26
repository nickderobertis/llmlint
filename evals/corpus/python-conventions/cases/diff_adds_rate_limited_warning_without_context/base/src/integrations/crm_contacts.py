"""Fetch contact records from the CRM's REST API."""

from __future__ import annotations

import logging

import httpx

logger = logging.getLogger(__name__)


class CrmUnavailable(Exception):
    """The CRM could not serve the request; retry later."""


def fetch_contacts(client: httpx.Client, account_id: str, cursor: str | None = None) -> dict:
    """Return one page of the account's contacts from the CRM.

    Raises:
        CrmUnavailable: on any 5xx response.
        httpx.HTTPStatusError: on any other non-2xx response.
    """
    params = {"account": account_id, **({"cursor": cursor} if cursor else {})}
    response = client.get("/v3/contacts", params=params)
    if response.status_code >= 500:
        logger.warning(
            "CRM contacts page for account %s returned %s", account_id, response.status_code
        )
        raise CrmUnavailable(f"CRM returned {response.status_code} for account {account_id}")
    response.raise_for_status()
    return response.json()
