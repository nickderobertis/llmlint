"""asyncpg-backed data access for the catalog."""

import base64
import json

import asyncpg

_pool: asyncpg.Pool | None = None


async def init(dsn: str) -> None:
    global _pool
    _pool = await asyncpg.create_pool(dsn, min_size=1, max_size=10)


def _encode_cursor(last_id: int) -> str:
    return base64.urlsafe_b64encode(json.dumps({"after": last_id}).encode()).decode()


def _decode_cursor(cursor: str | None) -> int:
    if not cursor:
        return 0
    return int(json.loads(base64.urlsafe_b64decode(cursor.encode()))["after"])


async def search_products(query: str, cursor: str | None, limit: int) -> tuple[list[dict], str | None]:
    after = _decode_cursor(cursor)
    async with _pool.acquire() as conn:
        rows = await conn.fetch(
            "SELECT id, sku, title, price_cents FROM products "
            "WHERE id > $1 AND title ILIKE '%' || $2 || '%' ORDER BY id LIMIT $3",
            after, query, limit,
        )
    items = [dict(r) for r in rows]
    next_cursor = _encode_cursor(items[-1]["id"]) if len(items) == limit else None
    return items, next_cursor


async def get_product(sku: str) -> dict | None:
    async with _pool.acquire() as conn:
        row = await conn.fetchrow("SELECT * FROM products WHERE sku = $1", sku)
    return dict(row) if row else None


async def run_reindex() -> int:
    async with _pool.acquire() as conn:
        status = await conn.execute("REINDEX TABLE products")
    return 0 if status == "REINDEX" else 1
