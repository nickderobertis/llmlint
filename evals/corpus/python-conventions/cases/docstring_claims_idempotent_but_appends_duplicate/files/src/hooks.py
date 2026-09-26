"""Lifecycle hooks plugins can attach to the request pipeline."""

from __future__ import annotations

from collections import defaultdict
from collections.abc import Callable
from typing import Any

Hook = Callable[[dict[str, Any]], None]

PHASES = ("before_request", "after_request", "on_error")

_hooks: dict[str, list[Hook]] = defaultdict(list)


def register(phase: str, hook: Hook) -> None:
    """Attach ``hook`` to ``phase``.

    Idempotent: registering the same hook for the same phase again has no
    effect, so plugins may call this on every import.

    Raises:
        ValueError: if ``phase`` is not one of ``PHASES``.
    """
    if phase not in PHASES:
        raise ValueError(f"unknown hook phase {phase!r}; expected one of {PHASES}")
    _hooks[phase].append(hook)


def unregister(phase: str, hook: Hook) -> bool:
    """Detach ``hook`` from ``phase``; return True if it was attached."""
    try:
        _hooks[phase].remove(hook)
    except ValueError:
        return False
    return True


def fire(phase: str, context: dict[str, Any]) -> None:
    """Call every hook attached to ``phase`` in registration order."""
    for hook in list(_hooks[phase]):
        hook(context)
