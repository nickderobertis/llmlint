"""Runtime settings for the storefront service.

All configuration comes from the environment so the same image runs in every
stage. Values are read once at import time and exposed as module constants.
"""

from __future__ import annotations

import os
from dataclasses import dataclass
from functools import lru_cache


def _bool(name: str, default: bool = False) -> bool:
    raw = os.getenv(name)
    if raw is None:
        return default
    return raw.strip().lower() in {"1", "true", "yes", "on"}


DEBUG = _bool("DEBUG")
SECRET_KEY = os.environ["SECRET_KEY"]
ALLOWED_HOSTS = [h for h in os.getenv("ALLOWED_HOSTS", "localhost").split(",") if h]

DATABASE_URL = os.environ.get("DATABASE_URL", "postgresql://localhost/storefront")
REDIS_URL = os.environ.get("REDIS_URL", "redis://localhost:6379/0")

EMAIL_FROM = os.getenv("EMAIL_FROM", "orders@example.com")
SMTP_HOST = os.getenv("SMTP_HOST", "localhost")
SMTP_PORT = int(os.getenv("SMTP_PORT", "25"))


@dataclass(frozen=True)
class StripeConfig:
    api_key: str
    webhook_secret: str
    currency: str = "usd"


@lru_cache(maxsize=1)
def stripe_config() -> StripeConfig:
    """Stripe credentials, loaded lazily so tests without Stripe still import."""
    return StripeConfig(
        api_key=os.environ["STRIPE_API_KEY"],
        webhook_secret=os.environ["STRIPE_WEBHOOK_SECRET"],
        currency=os.getenv("STRIPE_CURRENCY", "usd"),
    )


def describe() -> dict[str, object]:
    """A redacted view of the settings for the /healthz endpoint."""
    return {
        "debug": DEBUG,
        "allowed_hosts": ALLOWED_HOSTS,
        "database": DATABASE_URL.split("@")[-1],
        "smtp": f"{SMTP_HOST}:{SMTP_PORT}",
    }
