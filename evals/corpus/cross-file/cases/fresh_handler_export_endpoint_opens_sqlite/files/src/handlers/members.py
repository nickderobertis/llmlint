"""HTTP handlers for /members."""

import csv
import io
import sqlite3

from flask import Blueprint, Response, abort, jsonify, request

from src import repository
from src.repository import DB_PATH

bp = Blueprint("members", __name__, url_prefix="/members")


def _page_args() -> tuple[int, int]:
    try:
        limit = min(int(request.args.get("limit", 50)), 200)
        offset = max(int(request.args.get("offset", 0)), 0)
    except ValueError:
        abort(400, "limit and offset must be integers")
    return limit, offset


@bp.get("/")
def index():
    limit, offset = _page_args()
    return jsonify(repository.list_members(limit=limit, offset=offset))


@bp.get("/<int:member_id>")
def show(member_id: int):
    member = repository.get_member(member_id)
    if member is None:
        abort(404)
    return jsonify(member)


@bp.post("/")
def create():
    body = request.get_json(force=True) or {}
    email = (body.get("email") or "").strip().lower()
    name = (body.get("name") or "").strip()
    if "@" not in email or not name:
        abort(400, "email and name are required")
    member_id = repository.create_member(email=email, name=name)
    return jsonify({"id": member_id}), 201


@bp.delete("/<int:member_id>")
def deactivate(member_id: int):
    if repository.get_member(member_id) is None:
        abort(404)
    repository.deactivate_member(member_id)
    return "", 204


@bp.get("/export.csv")
def export_csv():
    """Stream every active member as CSV for the finance team."""
    buf = io.StringIO()
    writer = csv.writer(buf)
    writer.writerow(["id", "email", "name", "joined_at"])
    conn = sqlite3.connect(DB_PATH)
    try:
        for row in conn.execute(
            "SELECT id, email, name, joined_at FROM members WHERE active = 1 ORDER BY id"
        ):
            writer.writerow(row)
    finally:
        conn.close()
    return Response(
        buf.getvalue(),
        mimetype="text/csv",
        headers={"Content-Disposition": "attachment; filename=members.csv"},
    )
