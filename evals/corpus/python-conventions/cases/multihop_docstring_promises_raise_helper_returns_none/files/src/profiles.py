"""Customer profile lookups backed by the accounts API."""

from __future__ import annotations

import logging
from dataclasses import dataclass

import httpx

logger = logging.getLogger(__name__)


class ProfileNotFound(LookupError):
    """Raised when the accounts API has no profile for the requested id."""


@dataclass(frozen=True, slots=True)
class Profile:
    account_id: str
    email: str
    display_name: str
    tier: str


def _fetch_profile_json(client: httpx.Client, account_id: str) -> dict | None:
    response = client.get(f"/v2/accounts/{account_id}/profile")
    if response.status_code == 404:
        return None
    response.raise_for_status()
    return response.json()


def _to_profile(account_id: str, body: dict | None) -> Profile | None:
    if body is None:
        return None
    return Profile(
        account_id=account_id,
        email=body["email"],
        display_name=body.get("display_name") or body["email"].split("@")[0],
        tier=body.get("tier", "free"),
    )


def load_profile(client: httpx.Client, account_id: str) -> Profile:
    """Fetch the profile for ``account_id``.

    Raises:
        ProfileNotFound: if the accounts API returns 404 for the id.
        httpx.HTTPStatusError: for any other non-2xx response.
    """
    body = _fetch_profile_json(client, account_id)
    return _to_profile(account_id, body)


def display_label(profile: Profile) -> str:
    """Return ``"<display name> (<tier>)"`` for the account switcher."""
    return f"{profile.display_name} ({profile.tier})"
