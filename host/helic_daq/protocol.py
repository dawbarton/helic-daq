"""Wire protocol, mirroring the Rust ``helic-proto`` crate.

Everything is little-endian. The authoritative description is
``docs/protocol.md``; the known-answer vectors there are tested against both
this module and the Rust implementation.
"""

from __future__ import annotations

import struct
from dataclasses import dataclass

MAGIC = 0x4C48  # little-endian ASCII "HL"
VERSION = 1
CONTROL_PORT = 2350
STREAM_PORT = 2351

HEADER_LEN = 6
TRAILER_LEN = 2
MAX_PAYLOAD = 512


# Control message types.
class MsgType:
    GET_PAR_NAMES = 1
    GET_PAR_INFO = 2
    GET_PAR = 3
    SET_PAR = 4
    SET_BLOCK = 5  # reserved
    COMMIT = 6  # reserved
    STREAM_SETUP = 7
    STREAM_START = 8
    STREAM_STOP = 9
    STATUS = 10
    ERROR = 0xFF


ERROR_NAMES = {
    1: "bad frame",
    2: "unknown message type",
    3: "bad parameter index",
    4: "bad length",
    5: "parameter is read-only",
    6: "bad value",
    7: "device busy",
}

# Stream source ids: adc0..adc7 are 0..7.
SOURCES = {f"adc{i}": i for i in range(8)}
SOURCES.update({"laser": 8, "target": 9, "forcing": 10, "out": 11})
SOURCE_NAMES = {v: k for k, v in SOURCES.items()}


class ProtocolError(Exception):
    """Malformed frame or packet."""


def crc16(data: bytes) -> int:
    """CRC-16/CCITT-FALSE (poly 0x1021, init 0xFFFF)."""
    crc = 0xFFFF
    for byte in data:
        crc ^= byte << 8
        for _ in range(8):
            crc = ((crc << 1) ^ 0x1021) & 0xFFFF if crc & 0x8000 else (crc << 1) & 0xFFFF
    return crc


def encode_frame(msg_type: int, seq: int, payload: bytes = b"") -> bytes:
    """Encode one control frame."""
    if len(payload) > MAX_PAYLOAD:
        raise ProtocolError(f"payload too long ({len(payload)} > {MAX_PAYLOAD})")
    body = struct.pack("<BBH", msg_type, seq & 0xFF, len(payload)) + payload
    return struct.pack("<H", MAGIC) + body + struct.pack("<H", crc16(body))


def decode_frame(buf: bytes) -> tuple[int, int, bytes]:
    """Decode one complete control frame; returns (type, seq, payload)."""
    if len(buf) < HEADER_LEN + TRAILER_LEN:
        raise ProtocolError("frame truncated")
    magic, msg_type, seq, length = struct.unpack_from("<HBBH", buf)
    if magic != MAGIC:
        raise ProtocolError(f"bad magic 0x{magic:04X}")
    if len(buf) != HEADER_LEN + length + TRAILER_LEN:
        raise ProtocolError("frame length mismatch")
    (crc_stored,) = struct.unpack_from("<H", buf, HEADER_LEN + length)
    if crc16(buf[2 : HEADER_LEN + length]) != crc_stored:
        raise ProtocolError("CRC mismatch")
    return msg_type, seq, buf[HEADER_LEN : HEADER_LEN + length]


# UDP stream packet header (20 bytes).
_STREAM_HEADER = struct.Struct("<HBBIIIHH")
STREAM_HEADER_LEN = _STREAM_HEADER.size


@dataclass
class StreamHeader:
    n_sources: int
    seq: int
    first_index: int
    dropped: int
    decimation: int
    n_records: int


def decode_stream_header(buf: bytes) -> StreamHeader:
    if len(buf) < STREAM_HEADER_LEN:
        raise ProtocolError("stream packet too short")
    magic, version, n_sources, seq, first_index, dropped, decimation, n_records = (
        _STREAM_HEADER.unpack_from(buf)
    )
    if magic != MAGIC or version != VERSION:
        raise ProtocolError("bad stream packet magic/version")
    return StreamHeader(n_sources, seq, first_index, dropped, decimation, n_records)


def encode_stream_header(h: StreamHeader) -> bytes:
    """Used by tests and emulators."""
    return _STREAM_HEADER.pack(
        MAGIC, VERSION, h.n_sources, h.seq, h.first_index, h.dropped, h.decimation, h.n_records
    )
