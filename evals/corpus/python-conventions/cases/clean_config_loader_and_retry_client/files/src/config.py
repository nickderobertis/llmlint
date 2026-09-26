"""Load application settings from a TOML file with environment overrides."""

from __future__ import annotations

import logging
import tomllib
from dataclasses import dataclass
from pathlib import Path

logger = logging.getLogger(__name__)

DEFAULT_TIMEOUT_SECONDS = 10.0


class ConfigError(Exception):
    """Raised when the settings file is missing, unreadable, or malformed."""


@dataclass(frozen=True, slots=True)
class Settings:
    database_url: str
    request_timeout_seconds: float
    feature_flags: frozenset[str]


def load_settings(path: Path) -> Settings:
    """Read settings from ``path``.

    ``request_timeout_seconds`` defaults to 10 seconds when the file omits it.

    Raises:
        ConfigError: if the file cannot be read or parsed, lacks
            ``database_url``, or has a non-positive timeout.
    """
    try:
        raw = tomllib.loads(path.read_text(encoding="utf-8"))
    except OSError as exc:
        raise ConfigError(f"cannot read settings file {path}: {exc}") from exc
    except tomllib.TOMLDecodeError as exc:
        raise ConfigError(f"settings file {path} is not valid TOML: {exc}") from exc

    try:
        database_url = raw["database_url"]
    except KeyError:
        raise ConfigError(f"settings file {path} has no database_url") from None

    timeout = raw.get("request_timeout_seconds", DEFAULT_TIMEOUT_SECONDS)
    if not isinstance(timeout, (int, float)) or timeout <= 0:
        raise ConfigError(
            f"settings file {path}: request_timeout_seconds must be a positive number"
        )
    flags = frozenset(raw.get("feature_flags", []))
    return Settings(database_url, float(timeout), flags)


def flag_enabled(settings: Settings, name: str) -> bool:
    """Return True if feature flag ``name`` is listed in the settings."""
    return name in settings.feature_flags
