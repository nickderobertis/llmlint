import os

import pytest

import src.api as codec

ROUNDTRIPS = [
    (codec.b64url_encode, codec.b64url_decode),
    (codec.write_varint, codec.read_varint),
]


@pytest.mark.parametrize("encode, decode", ROUNDTRIPS, ids=["b64url", "varint"])
@pytest.mark.parametrize("seed", range(5))
def test_roundtrip(encode, decode, seed):
    rng = __import__("random").Random(seed)
    if encode is codec.write_varint:
        value = rng.randrange(0, 2**63)
    else:
        value = os.urandom(rng.randrange(0, 64))
    encoded = encode(value)
    decoded = decode(encoded)
    # read_varint returns (value, bytes_consumed)
    if isinstance(decoded, tuple):
        decoded = decoded[0]
    assert decoded == value


def test_b64url_has_no_padding():
    assert "=" not in codec.b64url_encode(b"ab")


def test_varint_small_values_are_one_byte():
    assert len(codec.write_varint(127)) == 1
    assert len(codec.write_varint(128)) == 2
