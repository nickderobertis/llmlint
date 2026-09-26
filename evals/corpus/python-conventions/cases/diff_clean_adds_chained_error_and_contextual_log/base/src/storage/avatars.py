"""Store and fetch user avatar images in object storage."""

from __future__ import annotations

import logging

logger = logging.getLogger(__name__)

MAX_AVATAR_BYTES = 2 * 1024 * 1024


class AvatarTooLarge(ValueError):
    """The uploaded avatar exceeds ``MAX_AVATAR_BYTES``."""


def avatar_key(user_id: int) -> str:
    """Return the object key for ``user_id``'s avatar."""
    return f"avatars/{user_id}.png"


def save_avatar(bucket, user_id: int, png: bytes) -> str:
    """Upload ``png`` as the user's avatar and return its object key.

    Raises:
        AvatarTooLarge: if ``png`` exceeds ``MAX_AVATAR_BYTES``.
    """
    if len(png) > MAX_AVATAR_BYTES:
        raise AvatarTooLarge(f"avatar for user {user_id} is {len(png)} bytes")
    key = avatar_key(user_id)
    bucket.put_object(Key=key, Body=png, ContentType="image/png")
    return key
