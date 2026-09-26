"""Price lookup handlers.

The SQL behind these lookups lives in src/repository.py; a handler only parses
the request, calls a repository function with the connection it was given, and
shapes the response.
"""
from src import repository

MAX_RESULTS = 50


def get_price(conn, request) -> dict:
    sku = request.args["sku"]
    cents = repository.price_for(conn, sku)
    if cents is None:
        return {"status": 404}
    return {"status": 200, "cents": cents}


def search(conn, request) -> dict:
    # `query` is the user's search text, not a SQL query.
    query = request.args.get("q", "").strip()
    if not query:
        return {"status": 400, "error": "q is required"}
    limit = min(int(request.args.get("limit", MAX_RESULTS)), MAX_RESULTS)
    rows = repository.search_prices(conn, query, limit)
    return {"status": 200, "results": [{"sku": s, "cents": c} for s, c in rows]}
