"""A small JSON HTTP client with bounded retries."""

from __future__ import annotations

import logging
import time

import httpx

logger = logging.getLogger(__name__)


class UpstreamUnavailable(Exception):
    """Raised when the upstream still fails after every retry."""


def get_json(
    client: httpx.Client, url: str, *, attempts: int = 3, backoff_seconds: float = 0.5
) -> dict:
    """GET ``url`` and return the decoded JSON body.

    Retries transport errors and 5xx responses up to ``attempts`` times, sleeping
    ``backoff_seconds * attempt`` between tries.

    Raises:
        UpstreamUnavailable: when every attempt fails; the last error is chained.
        httpx.HTTPStatusError: immediately, for a 4xx response.
    """
    last_error: Exception | None = None
    for attempt in range(1, attempts + 1):
        try:
            response = client.get(url)
            response.raise_for_status()
            return response.json()
        except httpx.HTTPStatusError as exc:
            if exc.response.status_code < 500:
                raise
            last_error = exc
        except httpx.TransportError as exc:
            last_error = exc
        logger.warning(
            "GET %s failed on attempt %d/%d: %s", url, attempt, attempts, last_error
        )
        if attempt < attempts:
            time.sleep(backoff_seconds * attempt)
    raise UpstreamUnavailable(f"GET {url} failed after {attempts} attempts") from last_error
