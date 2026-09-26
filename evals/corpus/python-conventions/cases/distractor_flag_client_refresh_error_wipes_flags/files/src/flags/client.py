"""Feature-flag client backed by the flags service.

Flags are fetched as one JSON document and cached in-process. ``refresh()`` is
called by a background thread every ``refresh_seconds``; reads never block on
the network. When the flags service is unreachable the client keeps serving the
last good document, so an outage freezes flags rather than flipping them.
"""

from __future__ import annotations

import hashlib
import logging
import threading
import time
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from typing import Any

import httpx

logger = logging.getLogger(__name__)

DEFAULT_REFRESH_SECONDS = 30.0


class FlagDocumentError(ValueError):
    """The flags service returned a document that does not match the schema."""


@dataclass(frozen=True, slots=True)
class Flag:
    key: str
    enabled: bool
    rollout_percent: int
    allow_accounts: frozenset[int]


def parse_document(body: Mapping[str, Any]) -> dict[str, Flag]:
    """Parse the flags service's JSON document into ``Flag`` objects.

    Raises:
        FlagDocumentError: if a flag lacks ``key`` or ``enabled``, or has a
            rollout outside 0..100.
    """
    flags: dict[str, Flag] = {}
    for raw in body.get("flags", []):
        try:
            key = raw["key"]
            enabled = bool(raw["enabled"])
        except KeyError as exc:
            raise FlagDocumentError(f"flag entry {raw!r} is missing {exc}") from exc
        rollout = int(raw.get("rollout_percent", 100))
        if not 0 <= rollout <= 100:
            raise FlagDocumentError(f"flag {key}: rollout_percent {rollout} is outside 0..100")
        flags[key] = Flag(key, enabled, rollout, frozenset(raw.get("allow_accounts", [])))
    return flags


def in_rollout(flag_key: str, account_id: int, percent: int) -> bool:
    """Return True if ``account_id`` falls inside the first ``percent`` percent of
    a stable per-flag hash ring. The same account always gets the same answer
    for the same flag."""
    if percent >= 100:
        return True
    if percent <= 0:
        return False
    digest = hashlib.sha256(f"{flag_key}:{account_id}".encode()).digest()
    bucket = int.from_bytes(digest[:4], "big") % 100
    return bucket < percent


class FlagClient:
    """Thread-safe cached view of the flags service."""

    def __init__(
        self,
        http: httpx.Client,
        *,
        environment: str,
        refresh_seconds: float = DEFAULT_REFRESH_SECONDS,
        clock: Callable[[], float] = time.monotonic,
    ) -> None:
        self._http = http
        self._environment = environment
        self._refresh_seconds = refresh_seconds
        self._clock = clock
        self._lock = threading.Lock()
        self._flags: dict[str, Flag] = {}
        self._etag: str | None = None
        self._last_success: float | None = None

    def refresh(self) -> bool:
        """Fetch the latest document; return True if the cached flags changed.

        On any failure the previous flags are kept (see the module docstring).
        """
        headers = {"If-None-Match": self._etag} if self._etag else {}
        try:
            response = self._http.get(f"/v1/environments/{self._environment}/flags", headers=headers)
            if response.status_code == 304:
                self._last_success = self._clock()
                return False
            response.raise_for_status()
            flags = parse_document(response.json())
        except FlagDocumentError as exc:
            logger.error(
                "flags document for environment %s rejected; keeping previous flags: %s",
                self._environment,
                exc,
            )
            return False
        except Exception:
            with self._lock:
                self._flags = {}
            return True
        with self._lock:
            changed = flags != self._flags
            self._flags = flags
            self._etag = response.headers.get("ETag")
            self._last_success = self._clock()
        return changed

    def is_enabled(self, flag_key: str, account_id: int | None = None) -> bool:
        """Return whether ``flag_key`` is on for ``account_id``.

        Unknown flags are off. An allow-listed account is always on when the
        flag is enabled; otherwise the rollout percentage decides.
        """
        with self._lock:
            flag = self._flags.get(flag_key)
        if flag is None or not flag.enabled:
            return False
        if account_id is None:
            return flag.rollout_percent >= 100
        if account_id in flag.allow_accounts:
            return True
        return in_rollout(flag.key, account_id, flag.rollout_percent)

    def staleness(self) -> float | None:
        """Seconds since the last successful refresh, or None if there was none."""
        if self._last_success is None:
            return None
        return self._clock() - self._last_success

    def run_refresher(self, stop: threading.Event) -> None:
        """Refresh every ``refresh_seconds`` until ``stop`` is set."""
        while not stop.wait(self._refresh_seconds):
            self.refresh()
