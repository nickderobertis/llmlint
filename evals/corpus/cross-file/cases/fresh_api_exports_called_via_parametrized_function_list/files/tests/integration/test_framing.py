"""Frames sent over a real socket pair."""

import socket

from src.api import pack_frame, unpack_frame


def test_frame_survives_socketpair():
    a, b = socket.socketpair()
    try:
        payload = b"hello" * 100
        a.sendall(pack_frame(kind=3, payload=payload))
        data = b.recv(4096)
        frame, rest = unpack_frame(data)
        assert frame.kind == 3
        assert frame.payload == payload
        assert rest == b""
    finally:
        a.close()
        b.close()


def test_two_frames_back_to_back():
    blob = pack_frame(kind=1, payload=b"x") + pack_frame(kind=2, payload=b"yy")
    first, rest = unpack_frame(blob)
    second, rest = unpack_frame(rest)
    assert (first.kind, second.kind) == (1, 2)
    assert rest == b""
