"""UDP stream receiver: packets to numpy arrays, with drop accounting."""

from __future__ import annotations

import socket

import numpy as np

from . import protocol
from .protocol import ProtocolError, StreamHeader


class StreamReceiver:
    """Receives CBC-DAQ stream packets on a UDP port.

    Use as a context manager, or call :meth:`close` when done.
    """

    def __init__(self, port: int = protocol.STREAM_PORT, bind: str = "0.0.0.0", timeout: float = 2.0):
        self._sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self._sock.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 1 << 20)
        self._sock.bind((bind, port))
        self._sock.settimeout(timeout)
        self.port = port
        self.last_seq: int | None = None
        self.lost_packets = 0

    def close(self) -> None:
        self._sock.close()

    def __enter__(self) -> "StreamReceiver":
        return self

    def __exit__(self, *exc) -> None:
        self.close()

    def recv(self) -> tuple[StreamHeader, np.ndarray]:
        """Receive one packet: (header, values[n_records, n_sources]).

        Raises ``socket.timeout`` if nothing arrives; tracks lost packets
        via the header sequence numbers in :attr:`lost_packets`.
        """
        data, _addr = self._sock.recvfrom(2048)
        header = protocol.decode_stream_header(data)
        expected = protocol.STREAM_HEADER_LEN + 4 * header.n_sources * header.n_records
        if len(data) != expected:
            raise ProtocolError(f"stream packet length {len(data)} != expected {expected}")
        if self.last_seq is not None:
            gap = (header.seq - self.last_seq - 1) & 0xFFFFFFFF
            if 0 < gap < 1 << 16:  # ignore restarts (seq reset to 0)
                self.lost_packets += gap
        self.last_seq = header.seq
        values = np.frombuffer(data, dtype="<f4", offset=protocol.STREAM_HEADER_LEN)
        return header, values.reshape(header.n_records, header.n_sources)

    def capture(self, n_records: int, names: list[str]) -> dict:
        """Collect `n_records` records; returns ``{name: values}`` arrays
        plus ``"index"`` (sample index of each record) and ``"dropped"``
        (cumulative source-side drop counter at capture end)."""
        blocks, indices = [], []
        dropped = 0
        got = 0
        while got < n_records:
            header, values = self.recv()
            blocks.append(values)
            step = header.decimation
            indices.append(header.first_index + step * np.arange(header.n_records, dtype=np.int64))
            dropped = header.dropped
            got += header.n_records
        data = np.concatenate(blocks, axis=0)[:n_records]
        index = np.concatenate(indices)[:n_records]
        out = {name: data[:, i].copy() for i, name in enumerate(names)}
        out["index"] = index
        out["dropped"] = dropped
        return out
