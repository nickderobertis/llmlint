import time

import pytest

from src.api import (
    issue_token,
    refresh_token,
    revoke_token,
    verify_token,
)


@pytest.fixture
def token():
    return issue_token(user_id=42, scopes=["read"], ttl=60)


def test_issue_token_round_trips(token):
    claims = verify_token(token)
    assert claims["sub"] == 42
    assert claims["scopes"] == ["read"]


def test_verify_rejects_tampered_token(token):
    tampered = token[:-2] + ("AA" if not token.endswith("AA") else "BB")
    with pytest.raises(ValueError):
        verify_token(tampered)


def test_refresh_extends_expiry(token):
    before = verify_token(token)["exp"]
    time.sleep(1)
    refreshed = refresh_token(token, ttl=120)
    assert verify_token(refreshed)["exp"] > before


def test_refresh_keeps_scopes(token):
    refreshed = refresh_token(token, ttl=60)
    assert verify_token(refreshed)["scopes"] == ["read"]


def test_expired_token_is_rejected():
    short = issue_token(user_id=1, scopes=[], ttl=0)
    with pytest.raises(ValueError, match="expired"):
        verify_token(short)
