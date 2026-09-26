from src import repository


def _as_dict(row: tuple) -> dict:
    order_id, status, total_cents = row
    return {"id": order_id, "status": status, "total_cents": total_cents}


def list_orders(conn, request) -> dict:
    customer_id = int(request.args["customer_id"])
    rows = repository.list_orders(conn, customer_id)
    return {"status": 200, "orders": [_as_dict(r) for r in rows]}


def get_order(conn, request, order_id: int) -> dict:
    row = repository.get_order(conn, order_id)
    if row is None:
        return {"status": 404}
    return {"status": 200, "order": _as_dict(row)}


def create_order(conn, request) -> dict:
    body = request.json
    order_id = repository.create_order(conn, body["customer_id"], body["total_cents"])
    return {"status": 201, "id": order_id}


def ship_order(conn, request, order_id: int) -> dict:
    row = repository.get_order(conn, order_id)
    if row is None:
        return {"status": 404}
    if row[1] != "open":
        return {"status": 409, "error": "only open orders ship"}
    repository.set_status(conn, order_id, "shipped")
    return {"status": 200}


def cancel_order(conn, request, order_id: int) -> dict:
    row = repository.get_order(conn, order_id)
    if row is None:
        return {"status": 404}
    reason = request.json.get("reason", "")
    cur = conn.cursor()
    cur.execute(
        "UPDATE orders SET status = 'cancelled', cancel_reason = ? WHERE id = ?",
        (reason, order_id),
    )
    conn.commit()
    return {"status": 200}
