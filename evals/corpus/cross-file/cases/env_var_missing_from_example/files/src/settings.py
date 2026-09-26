"""Process settings, read once from the environment at import time."""
import os

DATABASE_URL = os.environ["DATABASE_URL"]
REDIS_URL = os.environ.get("REDIS_URL", "redis://localhost:6379/0")
SECRET_KEY = os.environ["SECRET_KEY"]
LOG_LEVEL = os.getenv("LOG_LEVEL", "info")
SENTRY_DSN = os.getenv("SENTRY_DSN")
WORKERS = int(os.getenv("WEB_CONCURRENCY", "2"))

# Not configurable: production never runs with debug on.
DEBUG = False
