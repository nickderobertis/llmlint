"""Resolve a user's display preferences from their stored profile blob."""

from __future__ import annotations

import logging
from typing import Any

logger = logging.getLogger(__name__)

DEFAULT_LOCALE = "en-US"
DEFAULT_TIMEZONE = "UTC"
SUPPORTED_LOCALES = frozenset({"en-US", "en-GB", "de-DE", "fr-FR", "ja-JP"})


def preferred_locale(profile: dict[str, Any], default: str = DEFAULT_LOCALE) -> str:
    """Return the profile's ``display.locale``.

    Profiles created before display preferences existed have no ``display``
    section; for those, and for any profile without a locale, ``default`` is
    returned. An unsupported locale also falls back to ``default``.
    """
    try:
        locale = profile["display"]["locale"]
    except KeyError:
        return default
    return locale if locale in SUPPORTED_LOCALES else default


def preferred_timezone(profile: dict[str, Any]) -> str:
    """Return the profile's ``display.timezone``, or ``"UTC"`` when it is unset."""
    try:
        return profile["display"]["timezone"]
    except KeyError:
        return DEFAULT_TIMEZONE


def page_size(profile: dict[str, Any]) -> int:
    """Return the preferred list page size, clamped to 10..200.

    Raises:
        ValueError: if the stored value is not an integer.
    """
    raw = profile.get("display", {}).get("page_size", 50)
    if not isinstance(raw, int):
        logger.warning(
            "profile %s has non-integer display.page_size %r",
            profile.get("user_id"),
            raw,
        )
        raise ValueError(f"page_size must be an integer, got {raw!r}")
    return max(10, min(200, raw))
