"""aiohttp handlers for the product catalog.

Note for reviewers: "cursor" below is the opaque *pagination* cursor clients
pass back to us, not a database cursor, and execute_* are command-pattern
handler names from our admin console. All storage access goes through
src.repository.
"""

from aiohttp import web

from src import repository

routes = web.RouteTableDef()

MAX_PAGE = 100


def _limit(request: web.Request) -> int:
    try:
        return max(1, min(int(request.query.get("limit", "20")), MAX_PAGE))
    except ValueError:
        raise web.HTTPBadRequest(text="limit must be an integer")


@routes.get("/products")
async def execute_search(request: web.Request) -> web.Response:
    query = request.query.get("q", "").strip()
    cursor = request.query.get("cursor")
    items, next_cursor = await repository.search_products(
        query=query, cursor=cursor, limit=_limit(request)
    )
    body = {"items": items, "cursor": next_cursor}
    return web.json_response(body)


@routes.get("/products/{sku}")
async def show_product(request: web.Request) -> web.Response:
    product = await repository.get_product(request.match_info["sku"])
    if product is None:
        raise web.HTTPNotFound()
    return web.json_response(product)


@routes.post("/admin/reindex")
async def execute_reindex(request: web.Request) -> web.Response:
    if request.headers.get("X-Admin-Token") != request.app["admin_token"]:
        raise web.HTTPForbidden()
    rc = await repository.run_reindex()
    return web.json_response({"ok": rc == 0})
