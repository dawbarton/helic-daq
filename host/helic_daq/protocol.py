"""Wire protocol, mirroring the Rust ``helic-proto`` crate.

Everything is little-endian. The authoritative description is
``docs/protocol.md``; the known-answer vectors there are tested against both
this module and the Rust implementation.
"""

from __future__ import annotations

import struct
from dataclasses import dataclass

MAGIC = 0x4C48  # little-endian ASCII "HL"
VERSION = 2
CONTROL_PORT = 2350
STREAM_PORT = 2351
DISCOVERY_PORT = 2352

BEACON_REQUEST = struct.pack("<HB", MAGIC, 1)
_BEACON_RESPONSE = struct.Struct("<HBBH6s16s16s")
BEACON_RESPONSE_LEN = _BEACON_RESPONSE.size

HEADER_LEN = 6
TRAILER_LEN = 2
MAX_PAYLOAD = 512


# Control message types.
class MsgType:
    GET_PARAMS = 1
    GET_SOURCES = 2
    GET_PAR = 3
    SET_PAR = 4
    SET_BLOCK = 5
    COMMIT = 6
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

class ProtocolError(Exception):
    """Malformed frame or packet."""


@dataclass(frozen=True)
class BeaconResponse:
    version: int
    control_port: int
    mac: bytes
    experiment: str
    firmware: str


def _fixed_ascii(value: str) -> bytes:
    return value.encode("ascii")[:16].ljust(16, b"\0")


def encode_beacon_response(response: BeaconResponse) -> bytes:
    return _BEACON_RESPONSE.pack(
        MAGIC,
        2,
        response.version,
        response.control_port,
        response.mac,
        _fixed_ascii(response.experiment),
        _fixed_ascii(response.firmware),
    )


def decode_beacon_response(buf: bytes) -> BeaconResponse:
    if len(buf) != BEACON_RESPONSE_LEN:
        raise ProtocolError("bad beacon response length")
    magic, kind, version, port, mac, experiment, firmware = _BEACON_RESPONSE.unpack(buf)
    if magic != MAGIC or kind != 2:
        raise ProtocolError("bad beacon response magic/type")
    try:
        experiment_text = experiment.rstrip(b"\0").decode("ascii")
        firmware_text = firmware.rstrip(b"\0").decode("ascii")
    except UnicodeDecodeError:
        raise ProtocolError("non-ASCII beacon identity") from None
    return BeaconResponse(version, port, mac, experiment_text, firmware_text)


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


def _nul_string(payload: bytes, offset: int) -> tuple[str, int]:
    try:
        end = payload.index(0, offset)
    except ValueError:
        raise ProtocolError("unterminated discovery string") from None
    try:
        value = payload[offset:end].decode("ascii")
    except UnicodeDecodeError:
        raise ProtocolError("non-ASCII discovery string") from None
    return value, end + 1


def decode_params(payload: bytes) -> list[tuple[str, str, int, bool]]:
    """Decode a GetParams response in registry order."""
    params, offset = [], 0
    while offset < len(payload):
        name, offset = _nul_string(payload, offset)
        if offset + 4 > len(payload):
            raise ProtocolError("truncated parameter definition")
        type_code, count, writable = struct.unpack_from("<cHB", payload, offset)
        try:
            type_code = type_code.decode("ascii")
        except UnicodeDecodeError:
            raise ProtocolError("invalid parameter type code") from None
        if type_code not in "BbHhIifc":
            raise ProtocolError(f"invalid parameter type code {type_code!r}")
        if writable not in (0, 1):
            raise ProtocolError("invalid writable flag")
        params.append((name, type_code, count, bool(writable)))
        offset += 4
    return params


def decode_sources(payload: bytes) -> list[tuple[str, str]]:
    """Decode a GetSources response; list position is the source id."""
    sources, offset = [], 0
    while offset < len(payload):
        name, offset = _nul_string(payload, offset)
        unit, offset = _nul_string(payload, offset)
        sources.append((name, unit))
    return sources


def encode_set_block(index: int, offset: int, data: bytes) -> bytes:
    return struct.pack("<HI", index, offset) + data


def encode_commit(index: int, length: int) -> bytes:
    return struct.pack("<HI", index, length)


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
