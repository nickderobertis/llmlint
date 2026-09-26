"""Process settings, read once from the environment at import time."""
import os


def _env(name: str, default: str | None = None) -> str:
    value = os.environ.get(name, default)
    if value is None:
        raise RuntimeError(f"missing required environment variable {name}")
    return value


DATABASE_URL = _env("DATABASE_URL")
CACHE_URL = _env("CACHE_URL", "redis://localhost:6379/0")
PAYMENTS_KEY = _env("STRIPE_API_KEY")
PORT = int(_env("PORT", "8000"))
