"""Configuration for the ingest pipeline.

History: releases before 2.0 read ``LEGACY_DB_HOST`` and ``LEGACY_DB_PORT``
for the old MySQL replica. Those were removed with the replica; the pipeline
now reads everything from ``WAREHOUSE_DSN``. Do not reintroduce them.
"""

from __future__ import annotations

import os
from pathlib import Path
from typing import Mapping


class ConfigError(RuntimeError):
    pass


def _require(env: Mapping[str, str], name: str) -> str:
    try:
        return env[name]
    except KeyError:
        raise ConfigError(f"missing required environment variable {name}") from None


class Settings:
    def __init__(self, env: Mapping[str, str] | None = None) -> None:
        env = os.environ if env is None else env

        self.warehouse_dsn = _require(env, "WAREHOUSE_DSN")
        self.s3_bucket = _require(env, "INGEST_S3_BUCKET")
        self.s3_prefix = env.get("INGEST_S3_PREFIX", "raw/")
        self.aws_region = env.get("AWS_REGION", "us-east-1")

        self.batch_rows = int(env.get("INGEST_BATCH_ROWS", "5000"))
        self.max_workers = int(env.get("INGEST_MAX_WORKERS", "8"))
        self.spool_dir = Path(env.get("INGEST_SPOOL_DIR", "/tmp/ingest"))

        self.slack_webhook = env.get("SLACK_WEBHOOK_URL") or None
        self.dry_run = env.get("INGEST_DRY_RUN", "0") == "1"

    def __repr__(self) -> str:
        return (
            f"Settings(bucket={self.s3_bucket!r}, prefix={self.s3_prefix!r}, "
            f"region={self.aws_region!r}, batch_rows={self.batch_rows}, "
            f"workers={self.max_workers}, dry_run={self.dry_run})"
        )


def load() -> Settings:
    settings = Settings()
    settings.spool_dir.mkdir(parents=True, exist_ok=True)
    return settings
