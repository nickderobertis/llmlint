"""Post-upload processing for user documents.

When a user uploads a document, the web tier stores the raw bytes and enqueues
an ``UploadReceived`` task. This module runs that task: it validates the file
type, scans it for malware, extracts a text preview, generates thumbnails for
image and PDF uploads, and finally marks the upload ready (or rejected). Each
stage is idempotent so a crashed worker can simply re-run the task.
"""

from __future__ import annotations

import hashlib
import logging
import mimetypes
from dataclasses import dataclass, field
from enum import StrEnum
from pathlib import PurePosixPath
from typing import Protocol

logger = logging.getLogger(__name__)

MAX_UPLOAD_BYTES = 50 * 1024 * 1024
PREVIEW_CHARS = 2_000
THUMBNAIL_SIZES = ((128, 128), (512, 512))
ALLOWED_TYPES = frozenset(
    {
        "application/pdf",
        "image/png",
        "image/jpeg",
        "text/plain",
        "text/csv",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    }
)


class UploadState(StrEnum):
    RECEIVED = "received"
    READY = "ready"
    REJECTED = "rejected"


class Rejected(Exception):
    """The upload must not be served; the message is shown to the user."""


class ScannerUnavailable(Exception):
    """The malware scanner could not be reached; retry the task later."""


@dataclass(frozen=True, slots=True)
class Upload:
    upload_id: str
    owner_id: int
    filename: str
    size_bytes: int
    storage_key: str


@dataclass
class ProcessingResult:
    upload_id: str
    state: UploadState
    sha256: str | None = None
    preview: str | None = None
    thumbnails: list[str] = field(default_factory=list)
    rejection_reason: str | None = None


class BlobStore(Protocol):
    def read(self, key: str) -> bytes: ...

    def write(self, key: str, data: bytes, content_type: str) -> None: ...


class Scanner(Protocol):
    def scan(self, data: bytes) -> list[str]: ...


class Thumbnailer(Protocol):
    def render(self, data: bytes, content_type: str, size: tuple[int, int]) -> bytes: ...


class TextExtractor(Protocol):
    def extract(self, data: bytes, content_type: str) -> str: ...


class UploadStore(Protocol):
    def save_result(self, result: ProcessingResult) -> None: ...


def guess_content_type(filename: str) -> str:
    """Return the MIME type implied by ``filename``'s extension, or
    ``application/octet-stream`` when it is unknown."""
    guessed, _ = mimetypes.guess_type(PurePosixPath(filename).name)
    return guessed or "application/octet-stream"


def validate(upload: Upload) -> str:
    """Check size and type limits and return the upload's content type.

    Raises:
        Rejected: if the upload is too large or of a disallowed type.
    """
    if upload.size_bytes > MAX_UPLOAD_BYTES:
        raise Rejected(
            f"{upload.filename} is {upload.size_bytes} bytes; the limit is {MAX_UPLOAD_BYTES}"
        )
    content_type = guess_content_type(upload.filename)
    if content_type not in ALLOWED_TYPES:
        raise Rejected(f"{upload.filename}: files of type {content_type} are not accepted")
    return content_type


def scan(scanner: Scanner, upload: Upload, data: bytes) -> None:
    """Run the malware scan.

    Raises:
        Rejected: if the scanner reports any signature.
        ScannerUnavailable: if the scanner cannot be reached.
    """
    try:
        findings = scanner.scan(data)
    except ConnectionError as exc:
        logger.warning(
            "malware scan for upload %s could not reach the scanner: %s",
            upload.upload_id,
            exc,
        )
        raise ScannerUnavailable(f"scanner unreachable for upload {upload.upload_id}") from exc
    if findings:
        logger.warning(
            "upload %s from owner %s matched malware signatures %s",
            upload.upload_id,
            upload.owner_id,
            ", ".join(findings),
        )
        raise Rejected(f"{upload.filename} failed the malware scan")


def make_thumbnails(
    thumbnailer: Thumbnailer,
    blobs: BlobStore,
    upload: Upload,
    data: bytes,
    content_type: str,
) -> list[str]:
    """Render and store one PNG thumbnail per ``THUMBNAIL_SIZES`` entry.

    Returns the storage keys written. Non-image, non-PDF uploads get none.
    """
    if content_type not in ("application/pdf", "image/png", "image/jpeg"):
        return []
    keys = []
    for width, height in THUMBNAIL_SIZES:
        key = f"thumbs/{upload.upload_id}/{width}x{height}.png"
        png = thumbnailer.render(data, content_type, (width, height))
        blobs.write(key, png, "image/png")
        keys.append(key)
    return keys


def extract_preview(extractor: TextExtractor, upload: Upload, data: bytes, content_type: str) -> str | None:
    """Return the first ``PREVIEW_CHARS`` characters of extracted text.

    Returns None for images, and for documents whose text cannot be
    extracted (the failure is logged; a missing preview is not fatal).
    """
    if content_type.startswith("image/"):
        return None
    try:
        text = extractor.extract(data, content_type)
    except Exception as exc:
        logger.error("Error: %s", exc)
        return None
    return text[:PREVIEW_CHARS]


def process(
    upload: Upload,
    *,
    blobs: BlobStore,
    scanner: Scanner,
    thumbnailer: Thumbnailer,
    extractor: TextExtractor,
    store: UploadStore,
) -> ProcessingResult:
    """Run every stage for ``upload`` and persist the result.

    A ``Rejected`` upload is saved in the REJECTED state and returned, not
    raised. ``ScannerUnavailable`` propagates so the task queue retries.
    """
    try:
        content_type = validate(upload)
        data = blobs.read(upload.storage_key)
        scan(scanner, upload, data)
    except Rejected as exc:
        logger.info("upload %s rejected: %s", upload.upload_id, exc)
        result = ProcessingResult(upload.upload_id, UploadState.REJECTED, rejection_reason=str(exc))
        store.save_result(result)
        return result

    result = ProcessingResult(
        upload.upload_id,
        UploadState.READY,
        sha256=hashlib.sha256(data).hexdigest(),
        preview=extract_preview(extractor, upload, data, content_type),
        thumbnails=make_thumbnails(thumbnailer, blobs, upload, data, content_type),
    )
    store.save_result(result)
    logger.info(
        "upload %s ready (%d thumbnails, preview=%s)",
        upload.upload_id,
        len(result.thumbnails),
        result.preview is not None,
    )
    return result
