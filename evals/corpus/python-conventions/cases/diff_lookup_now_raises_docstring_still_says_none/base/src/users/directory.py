"""Look up users in the directory by email."""

from __future__ import annotations

import re
from dataclasses import dataclass

EMAIL_RE = re.compile(r"^[^@\s]+@[^@\s]+\.[^@\s]+$")


@dataclass(frozen=True, slots=True)
class User:
    user_id: int
    email: str
    display_name: str


class UserNotFound(LookupError):
    """No user has the requested email."""


def normalize_email(email: str) -> str:
    """Lower-case and strip ``email``.

    Raises:
        ValueError: if the result is not a syntactically valid address.
    """
    normalized = email.strip().lower()
    if not EMAIL_RE.match(normalized):
        raise ValueError(f"{email!r} is not a valid email address")
    return normalized


def find_by_email(users_by_email: dict[str, User], email: str) -> User | None:
    """Return the user registered under ``email``.

    Returns None if no user has that email.

    Raises:
        ValueError: if ``email`` is malformed.
    """
    return users_by_email.get(normalize_email(email))
