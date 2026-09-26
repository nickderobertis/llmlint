"""Data access for the members service. The only module that talks to SQLite."""

import sqlite3
from contextlib import contextmanager
from typing import Iterator

DB_PATH = "members.db"


@contextmanager
def _conn() -> Iterator[sqlite3.Connection]:
    conn = sqlite3.connect(DB_PATH)
    conn.row_factory = sqlite3.Row
    try:
        yield conn
        conn.commit()
    finally:
        conn.close()


def get_member(member_id: int) -> dict | None:
    with _conn() as c:
        row = c.execute("SELECT * FROM members WHERE id = ?", (member_id,)).fetchone()
        return dict(row) if row else None


def list_members(limit: int = 50, offset: int = 0) -> list[dict]:
    with _conn() as c:
        rows = c.execute(
            "SELECT * FROM members ORDER BY id LIMIT ? OFFSET ?", (limit, offset)
        ).fetchall()
        return [dict(r) for r in rows]


def create_member(email: str, name: str) -> int:
    with _conn() as c:
        cur = c.execute("INSERT INTO members (email, name) VALUES (?, ?)", (email, name))
        return cur.lastrowid


def deactivate_member(member_id: int) -> None:
    with _conn() as c:
        c.execute("UPDATE members SET active = 0 WHERE id = ?", (member_id,))
