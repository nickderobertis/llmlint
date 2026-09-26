"""Pagination parameters for list endpoints."""

from __future__ import annotations

from collections.abc import Mapping

MAX_PAGE_SIZE = 200
DEFAULT_PAGE_SIZE = 50


def page_size(query: Mapping[str, str]) -> int:
    """Return the requested ``page_size``, capped at ``MAX_PAGE_SIZE``.

    A query without ``page_size`` gets ``DEFAULT_PAGE_SIZE`` (50).

    Raises:
        ValueError: if ``page_size`` is present but not a positive integer.
    """
    try:
        raw = query["page_size"]
    except KeyError:
        return DEFAULT_PAGE_SIZE
    size = int(raw)
    if size < 1:
        raise ValueError(f"page_size must be positive, got {size}")
    return min(size, MAX_PAGE_SIZE)
