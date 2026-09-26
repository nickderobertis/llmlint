"""Store and fetch user avatar images in object storage."""

from __future__ import annotations

import logging

from botocore.exceptions import BotoCoreError, ClientError

logger = logging.getLogger(__name__)

MAX_AVATAR_BYTES = 2 * 1024 * 1024


class AvatarTooLarge(ValueError):
    """The uploaded avatar exceeds ``MAX_AVATAR_BYTES``."""


class AvatarStorageError(Exception):
    """Object storage rejected or failed the avatar upload."""


def avatar_key(user_id: int) -> str:
    """Return the object key for ``user_id``'s avatar."""
    return f"avatars/{user_id}.png"


def save_avatar(bucket, user_id: int, png: bytes) -> str:
    """Upload ``png`` as the user's avatar and return its object key.

    Raises:
        AvatarTooLarge: if ``png`` exceeds ``MAX_AVATAR_BYTES``.
        AvatarStorageError: if object storage fails the upload; the storage
            error is chained as its cause.
    """
    if len(png) > MAX_AVATAR_BYTES:
        raise AvatarTooLarge(f"avatar for user {user_id} is {len(png)} bytes")
    key = avatar_key(user_id)
    try:
        bucket.put_object(Key=key, Body=png, ContentType="image/png")
    except (BotoCoreError, ClientError) as exc:
        logger.error("avatar upload for user %s to %s failed: %s", user_id, key, exc)
        raise AvatarStorageError(f"could not store avatar for user {user_id}") from exc
    return key
