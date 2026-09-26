"""Nightly export of the orders table to the analytics bucket."""

from __future__ import annotations

import gzip
import logging
import os
import tempfile
from contextlib import suppress
from pathlib import Path

logger = logging.getLogger(__name__)


def _remove_quietly(path: Path) -> None:
    """Delete ``path``; a file that is already gone counts as removed."""
    with suppress(FileNotFoundError):
        path.unlink()


def export_orders(repo, uploader, export_date: str) -> str:
    """Write the day's orders as gzipped CSV and upload it.

    Returns the object key it uploaded to. The local temp file is always
    removed, whether or not the upload succeeds.

    Raises:
        Whatever the repository or uploader raised; the failure is logged
        with the export date first.
    """
    key = f"orders/{export_date}.csv.gz"
    fd, tmp_name = tempfile.mkstemp(suffix=".csv.gz")
    os.close(fd)
    tmp_path = Path(tmp_name)
    try:
        with gzip.open(tmp_path, "wt", encoding="utf-8", newline="") as handle:
            for line in repo.orders_as_csv(export_date):
                handle.write(line)
        uploader.upload_file(tmp_path, key)
    except Exception:
        logger.exception("orders export for %s to %s failed", export_date, key)
        raise
    finally:
        _remove_quietly(tmp_path)
    return key
