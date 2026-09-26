import sqlite3


def connect(path: str) -> sqlite3.Connection:
    return sqlite3.connect(path)


def sku_exists(conn: sqlite3.Connection, sku: str) -> bool:
    row = conn.execute("SELECT 1 FROM skus WHERE sku = ?", (sku,)).fetchone()
    return row is not None


def price_for(conn: sqlite3.Connection, sku: str) -> int | None:
    row = conn.execute("SELECT cents FROM prices WHERE sku = ?", (sku,)).fetchone()
    return row[0] if row else None
