from src import repository


def _latest_price(conn, sku: str) -> int | None:
    # Prices are versioned by effective date; take the newest one.
    row = (
        conn.execute(
            "SELECT cents FROM prices WHERE sku = ? ORDER BY effective_at DESC LIMIT 1",
            (sku,),
        )
        .fetchone()
    )
    return row[0] if row else None


def get_price(conn, sku: str) -> dict:
    if not repository.sku_exists(conn, sku):
        return {"status": 404}
    cents = _latest_price(conn, sku)
    if cents is None:
        return {"status": 404}
    return {"status": 200, "cents": cents}
