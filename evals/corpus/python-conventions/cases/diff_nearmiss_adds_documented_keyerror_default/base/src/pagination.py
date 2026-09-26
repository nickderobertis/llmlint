"""Pagination parameters for list endpoints."""

from __future__ import annotations

from collections.abc import Mapping

MAX_PAGE_SIZE = 200


def page_size(query: Mapping[str, str]) -> int:
    """Return the requested ``page_size``, capped at ``MAX_PAGE_SIZE``.

    Raises:
        KeyError: if the query has no ``page_size``.
        ValueError: if ``page_size`` is not a positive integer.
    """
    size = int(query["page_size"])
    if size < 1:
        raise ValueError(f"page_size must be positive, got {size}")
    return min(size, MAX_PAGE_SIZE)
