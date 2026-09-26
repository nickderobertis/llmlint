from src import repository


def get_price(conn, sku: str) -> dict:
    cents = repository.price_for(conn, sku)
    if cents is None:
        return {"status": 404}
    return {"status": 200, "cents": cents}
