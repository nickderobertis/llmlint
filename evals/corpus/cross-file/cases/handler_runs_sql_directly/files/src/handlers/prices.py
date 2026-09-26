import sqlite3


def get_price(db_path: str, sku: str) -> dict:
    conn = sqlite3.connect(db_path)
    row = conn.execute("SELECT cents FROM prices WHERE sku = ?", (sku,)).fetchone()
    if row is None:
        return {"status": 404}
    return {"status": 200, "cents": row[0]}
