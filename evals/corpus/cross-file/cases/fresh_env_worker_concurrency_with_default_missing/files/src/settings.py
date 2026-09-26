import os
import logging

log = logging.getLogger(__name__)

# Queue worker configuration. Every value has a sane default so a developer can
# start a worker with no environment at all.

BROKER_URL = os.getenv("BROKER_URL", "amqp://guest:guest@localhost:5672//")
RESULT_BACKEND = os.getenv("RESULT_BACKEND", "redis://localhost:6379/1")

WORKER_TIMEOUT = int(os.getenv("WORKER_TIMEOUT", "600"))
WORKER_CONCURRENCY = int(os.getenv("WORKER_CONCURRENCY", "4"))
WORKER_PREFETCH = int(os.getenv("WORKER_PREFETCH", "1"))

LOG_LEVEL = os.getenv("LOG_LEVEL", "INFO").upper()

TASK_ROUTES = {
    "reports.*": {"queue": "slow"},
    "emails.*": {"queue": "fast"},
}


def validate() -> None:
    if WORKER_CONCURRENCY < 1:
        raise ValueError("WORKER_CONCURRENCY must be >= 1")
    if WORKER_TIMEOUT < 30:
        log.warning("WORKER_TIMEOUT below 30s will kill most report tasks")
    if LOG_LEVEL not in {"DEBUG", "INFO", "WARNING", "ERROR"}:
        raise ValueError(f"unknown LOG_LEVEL {LOG_LEVEL!r}")
