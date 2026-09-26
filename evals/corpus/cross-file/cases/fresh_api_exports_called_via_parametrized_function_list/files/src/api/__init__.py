"""Codec API: stable entry points re-exported from the internal modules."""

from ._base64url import b64url_decode, b64url_encode
from ._varint import read_varint, write_varint
from ._frame import Frame, pack_frame, unpack_frame

__all__ = [
    "b64url_encode",
    "b64url_decode",
    "write_varint",
    "read_varint",
    "pack_frame",
    "unpack_frame",
]

# Frame is intentionally not in __all__: it is an implementation detail that
# callers only ever receive from unpack_frame.
