"""Catalog search for the storefront listing pages."""

from __future__ import annotations

import logging
from dataclasses import dataclass

from src.repositories import products as products_repo

logger = logging.getLogger(__name__)

SORTABLE = {"price": "unit_price", "newest": "created_at", "name": "lower(title)"}
PAGE_SIZE = 24


@dataclass(frozen=True, slots=True)
class SearchPage:
    items: list[products_repo.ProductRow]
    page: int
    has_more: bool


def search(conn, term: str, *, sort: str = "newest", descending: bool = True, page: int = 1) -> SearchPage:
    """Return one page of products matching ``term``.

    Raises:
        ValueError: if ``sort`` is not one of ``price``, ``newest``, ``name``, or
            ``page`` is less than 1.
    """
    if sort not in SORTABLE:
        raise ValueError(f"cannot sort by {sort!r}; choose from {sorted(SORTABLE)}")
    if page < 1:
        raise ValueError(f"page must be >= 1, got {page}")
    order_by = f"{SORTABLE[sort]} {'DESC' if descending else 'ASC'}, product_id"
    rows = products_repo.search(
        conn,
        term,
        order_by=order_by,
        limit=PAGE_SIZE + 1,
        offset=(page - 1) * PAGE_SIZE,
    )
    return SearchPage(rows[:PAGE_SIZE], page, len(rows) > PAGE_SIZE)
