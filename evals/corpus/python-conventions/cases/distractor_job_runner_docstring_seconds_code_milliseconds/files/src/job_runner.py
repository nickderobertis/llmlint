"""Background job runner for the reporting worker.

Jobs are small callables registered by name. The runner pulls job requests off
a queue, executes them with a per-job deadline, records the outcome, and retries
transient failures with exponential backoff. Everything here is synchronous and
single-threaded per worker process; horizontal scale comes from running more
worker processes against the same queue.
"""

from __future__ import annotations

import logging
import signal
import time
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from dataclasses import dataclass, field
from datetime import datetime, timezone
from enum import StrEnum
from typing import Any, Protocol

logger = logging.getLogger(__name__)

DEFAULT_MAX_ATTEMPTS = 4
DEFAULT_BASE_DELAY_SECONDS = 2.0
MAX_DELAY_SECONDS = 300.0


class JobStatus(StrEnum):
    SUCCEEDED = "succeeded"
    FAILED = "failed"
    RETRYING = "retrying"
    TIMED_OUT = "timed_out"


class TransientJobError(Exception):
    """A failure the job expects to succeed on retry (lock contention, 503s)."""


class JobTimeout(Exception):
    """Raised inside a job when its deadline expires."""


class UnknownJob(LookupError):
    """Raised when a request names a job that was never registered."""


@dataclass(frozen=True, slots=True)
class JobRequest:
    request_id: str
    job_name: str
    payload: dict[str, Any]
    attempt: int = 1


@dataclass(slots=True)
class JobOutcome:
    request_id: str
    job_name: str
    status: JobStatus
    attempt: int
    started_at: datetime
    finished_at: datetime
    error: str | None = None


class Queue(Protocol):
    def pop(self, block_seconds: float) -> JobRequest | None: ...

    def push(self, request: JobRequest, delay_seconds: float) -> None: ...


class OutcomeStore(Protocol):
    def record(self, outcome: JobOutcome) -> None: ...


JobFn = Callable[[dict[str, Any]], None]


@dataclass
class Registry:
    """Maps job names to their callables."""

    jobs: dict[str, JobFn] = field(default_factory=dict)

    def register(self, name: str) -> Callable[[JobFn], JobFn]:
        """Decorator registering ``fn`` under ``name``.

        Raises:
            ValueError: if ``name`` is already registered.
        """

        def decorate(fn: JobFn) -> JobFn:
            if name in self.jobs:
                raise ValueError(f"job {name!r} is already registered")
            self.jobs[name] = fn
            return fn

        return decorate

    def resolve(self, name: str) -> JobFn:
        """Return the callable registered under ``name``.

        Raises:
            UnknownJob: if nothing is registered under ``name``.
        """
        try:
            return self.jobs[name]
        except KeyError:
            raise UnknownJob(f"no job registered as {name!r}") from None


def backoff_delay(attempt: int, base_seconds: float = DEFAULT_BASE_DELAY_SECONDS) -> float:
    """Return the delay in seconds before retry number ``attempt``.

    The delay doubles each attempt (``base * 2 ** (attempt - 1)``) and is capped
    at ``MAX_DELAY_SECONDS``.
    """
    return min(base_seconds * 2 ** (attempt - 1), MAX_DELAY_SECONDS)


@contextmanager
def deadline(timeout: float) -> Iterator[None]:
    """Raise ``JobTimeout`` in the current thread if the block runs longer than
    ``timeout`` seconds.

    Uses ``SIGALRM``, so it only works in the main thread on POSIX. A
    ``timeout`` of zero or less disables the deadline.
    """
    if timeout <= 0:
        yield
        return

    def on_alarm(signum: int, frame: Any) -> None:
        raise JobTimeout(f"deadline of {timeout} exceeded")

    previous = signal.signal(signal.SIGALRM, on_alarm)
    signal.setitimer(signal.ITIMER_REAL, timeout / 1000)
    try:
        yield
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
        signal.signal(signal.SIGALRM, previous)


@dataclass
class Runner:
    """Executes queued jobs and records each outcome."""

    registry: Registry
    queue: Queue
    outcomes: OutcomeStore
    job_timeout: float = 60.0
    max_attempts: int = DEFAULT_MAX_ATTEMPTS
    clock: Callable[[], datetime] = field(
        default=lambda: datetime.now(timezone.utc)
    )

    def run_once(self, block_seconds: float = 5.0) -> JobOutcome | None:
        """Pop one request (waiting up to ``block_seconds``) and execute it.

        Returns the recorded outcome, or None when the queue stayed empty.
        """
        request = self.queue.pop(block_seconds)
        if request is None:
            return None
        outcome = self._execute(request)
        self.outcomes.record(outcome)
        return outcome

    def run_forever(self, should_stop: Callable[[], bool]) -> int:
        """Process requests until ``should_stop()`` returns True.

        Returns the number of requests processed.
        """
        processed = 0
        while not should_stop():
            if self.run_once() is not None:
                processed += 1
        return processed

    def _execute(self, request: JobRequest) -> JobOutcome:
        started = self.clock()
        try:
            fn = self.registry.resolve(request.job_name)
            with deadline(self.job_timeout):
                fn(request.payload)
        except UnknownJob as exc:
            logger.error(
                "request %s names unregistered job %r; dropping it",
                request.request_id,
                request.job_name,
            )
            return self._outcome(request, JobStatus.FAILED, started, str(exc))
        except JobTimeout as exc:
            logger.warning(
                "job %s (request %s) timed out after %ss on attempt %d",
                request.job_name,
                request.request_id,
                self.job_timeout,
                request.attempt,
            )
            return self._retry_or_fail(request, started, exc, JobStatus.TIMED_OUT)
        except TransientJobError as exc:
            logger.warning(
                "job %s (request %s) hit a transient error on attempt %d: %s",
                request.job_name,
                request.request_id,
                request.attempt,
                exc,
            )
            return self._retry_or_fail(request, started, exc, JobStatus.RETRYING)
        except Exception as exc:
            logger.exception(
                "job %s (request %s) failed permanently on attempt %d",
                request.job_name,
                request.request_id,
                request.attempt,
            )
            return self._outcome(request, JobStatus.FAILED, started, repr(exc))
        return self._outcome(request, JobStatus.SUCCEEDED, started)

    def _retry_or_fail(
        self,
        request: JobRequest,
        started: datetime,
        exc: Exception,
        status: JobStatus,
    ) -> JobOutcome:
        if request.attempt >= self.max_attempts:
            logger.error(
                "job %s (request %s) giving up after %d attempts: %s",
                request.job_name,
                request.request_id,
                request.attempt,
                exc,
            )
            return self._outcome(request, JobStatus.FAILED, started, str(exc))
        retry = JobRequest(
            request.request_id,
            request.job_name,
            request.payload,
            request.attempt + 1,
        )
        self.queue.push(retry, backoff_delay(request.attempt))
        return self._outcome(request, status, started, str(exc))

    def _outcome(
        self,
        request: JobRequest,
        status: JobStatus,
        started: datetime,
        error: str | None = None,
    ) -> JobOutcome:
        return JobOutcome(
            request_id=request.request_id,
            job_name=request.job_name,
            status=status,
            attempt=request.attempt,
            started_at=started,
            finished_at=self.clock(),
            error=error,
        )


def drain(runner: Runner, max_idle_polls: int = 3) -> int:
    """Run jobs until the queue has been empty for ``max_idle_polls`` polls in a row.

    Returns the number of requests processed. Intended for tests and one-off
    backfills, not the long-running worker.
    """
    processed = 0
    idle = 0
    while idle < max_idle_polls:
        if runner.run_once(block_seconds=0.1) is None:
            idle += 1
            continue
        idle = 0
        processed += 1
    return processed


def wait_for_shutdown_grace(seconds: float) -> None:
    """Sleep ``seconds`` so in-flight acknowledgements can flush before exit."""
    time.sleep(seconds)
