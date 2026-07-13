"""TCP control connection: parameter discovery, get/set, stream control."""

from __future__ import annotations

import socket
import struct
from dataclasses import dataclass

from . import protocol
from .protocol import MsgType, ProtocolError
from .stream import StreamReceiver


class DeviceError(Exception):
    """Error reported by the device or the transport."""


@dataclass
class Parameter:
    index: int
    name: str
    type_code: str  # Python struct format character
    count: int
    writable: bool

    @property
    def size(self) -> int:
        return struct.calcsize(self.type_code) * self.count


class _ParamAccessor:
    """Attribute-style parameter access: ``dev.par.freq = 10.0``."""

    def __init__(self, device: "Device"):
        object.__setattr__(self, "_device", device)

    def __getattr__(self, name: str):
        return self._device.get(name)

    def __setattr__(self, name: str, value) -> None:
        self._device.set(name, value)

    def __dir__(self):
        return [p.name for p in self._device.params]


class Device:
    """A connection to a CBC-DAQ device.

    Discovers the parameter registry at connect; parameters are then
    accessible by name through :meth:`get`/:meth:`set` or the :attr:`par`
    attribute accessor.
    """

    def __init__(self, host: str, port: int = protocol.CONTROL_PORT, timeout: float = 5.0):
        self._sock = socket.create_connection((host, port), timeout=timeout)
        try:
            self._sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
            self._seq = 0
            self.host = host
            self.params: list[Parameter] = []
            self._by_name: dict[str, Parameter] = {}
            self._discover()
        except BaseException:
            self._sock.close()
            raise
        self.par = _ParamAccessor(self)

    # -- transport ---------------------------------------------------------

    def close(self) -> None:
        self._sock.close()

    def __enter__(self) -> "Device":
        return self

    def __exit__(self, *exc) -> None:
        self.close()

    def _read_exact(self, n: int) -> bytes:
        chunks = []
        while n > 0:
            chunk = self._sock.recv(n)
            if not chunk:
                raise DeviceError("connection closed by device")
            chunks.append(chunk)
            n -= len(chunk)
        return b"".join(chunks)

    def _request(self, msg_type: int, payload: bytes = b"") -> bytes:
        self._seq = (self._seq + 1) & 0xFF
        self._sock.sendall(protocol.encode_frame(msg_type, self._seq, payload))
        header = self._read_exact(protocol.HEADER_LEN)
        (length,) = struct.unpack_from("<H", header, 4)
        rest = self._read_exact(length + protocol.TRAILER_LEN)
        r_type, r_seq, r_payload = protocol.decode_frame(header + rest)
        if r_seq != self._seq:
            raise DeviceError(f"sequence mismatch (sent {self._seq}, got {r_seq})")
        if r_type == MsgType.ERROR:
            code = r_payload[0] if r_payload else 0
            raise DeviceError(
                f"device error: {protocol.ERROR_NAMES.get(code, f'code {code}')}"
            )
        if r_type != msg_type:
            raise DeviceError(f"response type mismatch ({r_type} != {msg_type})")
        return r_payload

    # -- discovery ---------------------------------------------------------

    def _discover(self) -> None:
        names = self._request(MsgType.GET_PAR_NAMES).split(b"\0")[:-1]
        info = self._request(MsgType.GET_PAR_INFO)
        if len(info) != 4 * len(names):
            raise ProtocolError("GetParInfo length does not match GetParNames")
        self.params = []
        for i, name in enumerate(names):
            type_code, count, writable = struct.unpack_from("<cHB", info, 4 * i)
            self.params.append(
                Parameter(i, name.decode(), type_code.decode(), count, bool(writable))
            )
        self._by_name = {p.name: p for p in self.params}

    def _param(self, name_or_index) -> Parameter:
        if isinstance(name_or_index, int):
            return self.params[name_or_index]
        try:
            return self._by_name[name_or_index]
        except KeyError:
            raise DeviceError(f"no parameter named {name_or_index!r}") from None

    # -- parameters --------------------------------------------------------

    def get(self, *names):
        """Get one or more parameters by name (or index).

        A single argument returns the value directly; multiple arguments
        return a list, fetched in one round trip.
        """
        params = [self._param(n) for n in names]
        payload = b"".join(struct.pack("<H", p.index) for p in params)
        data = self._request(MsgType.GET_PAR, payload)
        values, off = [], 0
        for p in params:
            values.append(self._unpack_value(p, data[off : off + p.size]))
            off += p.size
        return values[0] if len(values) == 1 else values

    def set(self, name, value) -> None:
        """Set a parameter by name (or index)."""
        p = self._param(name)
        if not p.writable:
            raise DeviceError(f"parameter {p.name!r} is read-only")
        raw = self._pack_value(p, value)
        self._request(MsgType.SET_PAR, struct.pack("<H", p.index) + raw)

    @staticmethod
    def _unpack_value(p: Parameter, raw: bytes):
        if p.type_code == "c":
            return raw.rstrip(b"\0").decode(errors="replace")
        values = struct.unpack(f"<{p.count}{p.type_code}", raw)
        return values[0] if p.count == 1 else list(values)

    @staticmethod
    def _pack_value(p: Parameter, value) -> bytes:
        if p.type_code == "c":
            raw = str(value).encode()
            return raw[: p.count].ljust(p.count, b"\0")
        if p.count == 1:
            return struct.pack(f"<{p.type_code}", value)
        values = list(value)
        if len(values) != p.count:
            raise DeviceError(f"{p.name!r} expects {p.count} values, got {len(values)}")
        return struct.pack(f"<{p.count}{p.type_code}", *values)

    # -- status and streaming ----------------------------------------------

    def status(self) -> dict:
        version, n_params, fs, uptime_ms = struct.unpack(
            "<BHfI", self._request(MsgType.STATUS)
        )
        return {
            "protocol_version": version,
            "n_params": n_params,
            "sample_rate": fs,
            "uptime_s": uptime_ms / 1000.0,
        }

    def stream_setup(self, sources, decimation: int = 1, count: int = 0) -> list[str]:
        """Configure the stream: which values, every n-th sample, and how
        many records in total (0 = continuous). Returns the resolved source
        names in record order."""
        ids, names = [], []
        for s in sources:
            if isinstance(s, str):
                if s not in protocol.SOURCES:
                    raise DeviceError(
                        f"unknown source {s!r}; choose from {sorted(protocol.SOURCES)}"
                    )
                ids.append(protocol.SOURCES[s])
                names.append(s)
            else:
                ids.append(int(s))
                names.append(protocol.SOURCE_NAMES.get(int(s), str(s)))
        payload = struct.pack("<HIB", decimation, count, len(ids)) + bytes(ids)
        self._request(MsgType.STREAM_SETUP, payload)
        return names

    def stream_start(self, port: int = protocol.STREAM_PORT) -> None:
        """Start streaming to this host's `port` (UDP)."""
        self._request(MsgType.STREAM_START, struct.pack("<H", port))

    def stream_stop(self) -> None:
        self._request(MsgType.STREAM_STOP)

    def capture(
        self,
        sources,
        samples: int | None = None,
        seconds: float | None = None,
        decimation: int = 1,
        port: int = protocol.STREAM_PORT,
    ):
        """Capture a finite stream and return ``{name: ndarray}`` plus
        ``"index"`` (sample indices) and ``"dropped"`` (source-side drops).

        Give either `samples` (records after decimation) or `seconds`.
        """
        if (samples is None) == (seconds is None):
            raise ValueError("specify exactly one of samples / seconds")
        if samples is None:
            fs = self.status()["sample_rate"]
            samples = max(1, int(seconds * fs / decimation))
        names = self.stream_setup(sources, decimation=decimation, count=samples)
        with StreamReceiver(port=port) as rx:
            self.stream_start(port)
            try:
                return rx.capture(samples, names)
            finally:
                self.stream_stop()
