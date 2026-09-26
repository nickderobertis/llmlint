import pytest

from src.api import parse_scopes, scope_allows


@pytest.mark.parametrize(
    "raw, expected",
    [
        ("read", ["read"]),
        ("read write", ["read", "write"]),
        ("  admin  ", ["admin"]),
        ("", []),
    ],
)
def test_parse_scopes(raw, expected):
    assert parse_scopes(raw) == expected


def test_admin_allows_everything():
    assert scope_allows(["admin"], "write")


def test_read_does_not_allow_write():
    assert not scope_allows(["read"], "write")
