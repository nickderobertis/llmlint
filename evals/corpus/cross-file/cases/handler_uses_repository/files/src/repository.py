import sqlite3


def connect(path: str) -> sqlite3.Connection:
    return sqlite3.connect(path)


def price_for(conn: sqlite3.Connection, sku: str) -> int | None:
    row = conn.execute("SELECT cents FROM prices WHERE sku = ?", (sku,)).fetchone()
    return row[0] if row else None
