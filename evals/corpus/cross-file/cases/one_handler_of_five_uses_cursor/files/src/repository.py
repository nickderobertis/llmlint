import sqlite3


def connect(path: str) -> sqlite3.Connection:
    return sqlite3.connect(path)


def list_orders(conn: sqlite3.Connection, customer_id: int) -> list[tuple]:
    return conn.execute(
        "SELECT id, status, total_cents FROM orders WHERE customer_id = ? ORDER BY id",
        (customer_id,),
    ).fetchall()


def get_order(conn: sqlite3.Connection, order_id: int) -> tuple | None:
    return conn.execute(
        "SELECT id, status, total_cents FROM orders WHERE id = ?", (order_id,)
    ).fetchone()


def create_order(conn: sqlite3.Connection, customer_id: int, total_cents: int) -> int:
    cur = conn.execute(
        "INSERT INTO orders (customer_id, status, total_cents) VALUES (?, 'open', ?)",
        (customer_id, total_cents),
    )
    conn.commit()
    return cur.lastrowid


def set_status(conn: sqlite3.Connection, order_id: int, status: str) -> None:
    conn.execute("UPDATE orders SET status = ? WHERE id = ?", (status, order_id))
    conn.commit()
