"""Read-through cache for product detail pages."""

from __future__ import annotations

import json
import logging
from typing import Any, Callable

import redis

logger = logging.getLogger(__name__)

TTL_SECONDS = 600


class ProductCache:
    """Caches product documents in Redis in front of the catalog database.

    The cache is an optimization only: when Redis is unreachable every read
    goes to the database and writes are skipped, so a cache outage costs
    latency, never correctness.
    """

    def __init__(self, client: redis.Redis, load_from_db: Callable[[str], dict[str, Any]]):
        self._client = client
        self._load_from_db = load_from_db

    def _cached(self, sku: str) -> dict[str, Any] | None:
        try:
            raw = self._client.get(f"product:{sku}")
        except redis.ConnectionError as exc:
            logger.warning(
                "product cache read for sku %s failed (%s); serving from database", sku, exc
            )
            return None
        return None if raw is None else json.loads(raw)

    def _store(self, sku: str, product: dict[str, Any]) -> None:
        try:
            self._client.set(f"product:{sku}", json.dumps(product), ex=TTL_SECONDS)
        except redis.ConnectionError as exc:
            logger.warning("product cache write for sku %s skipped: %s", sku, exc)

    def get(self, sku: str) -> dict[str, Any]:
        """Return the product document for ``sku``, from cache when possible.

        A miss (or a cache outage) loads from the database and, when the cache
        is reachable, stores the result for ``TTL_SECONDS``.
        """
        product = self._cached(sku)
        if product is None:
            product = self._load_from_db(sku)
            self._store(sku, product)
        return product
