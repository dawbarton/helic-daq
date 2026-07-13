"""A minimal in-process CBC-DAQ emulator for testing the host package.

Speaks protocol v1 over a localhost TCP socket with a small parameter table
mirroring the firmware's registry shape. Not a simulator: parameter writes
just update a table.
"""

from __future__ import annotations

import socket
import struct
import threading

from cbc_daq import protocol
from cbc_daq.protocol import MsgType


class EmulatedParam:
    def __init__(self, name, type_code, count, writable, value):
        self.name = name
        self.type_code = type_code
        self.count = count
        self.writable = writable
        self.value = value

    @property
    def size(self):
        return struct.calcsize(self.type_code) * self.count


def default_params():
    return [
        EmulatedParam("firmware", "c", 16, False, b"cbc-daq emu\0\0\0\0\0"),
        EmulatedParam("sample_freq", "f", 1, False, (8000.0,)),
        EmulatedParam("ticks", "I", 1, False, (12345,)),
        EmulatedParam("freq", "f", 1, True, (0.0,)),
        EmulatedParam("forcing_coeffs", "f", 33, True, (0.0,) * 33),
        EmulatedParam("ctrl_kp", "f", 1, True, (0.0,)),
    ]


class Emulator:
    """Run with ``with Emulator() as emu: Device('127.0.0.1', emu.port)``."""

    def __init__(self):
        self.params = default_params()
        self.stream_setup = None
        self.stream_target = None
        self._listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self._listener.bind(("127.0.0.1", 0))
        self._listener.listen(1)
        self.port = self._listener.getsockname()[1]
        self._thread = threading.Thread(target=self._serve, daemon=True)
        self._thread.start()

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()

    def close(self):
        self._listener.close()

    def _serve(self):
        # Like the firmware: one connection at a time, but accept the next
        # one once the current client disconnects.
        while True:
            try:
                conn, _ = self._listener.accept()
            except OSError:
                return
            with conn:
                self._serve_connection(conn)

    def _serve_connection(self, conn):
        buf = b""
        while True:
            try:
                data = conn.recv(4096)
            except OSError:
                return
            if not data:
                return
            buf += data
            while len(buf) >= protocol.HEADER_LEN:
                (length,) = struct.unpack_from("<H", buf, 4)
                total = protocol.HEADER_LEN + length + protocol.TRAILER_LEN
                if len(buf) < total:
                    break
                frame, buf = buf[:total], buf[total:]
                msg_type, seq, payload = protocol.decode_frame(frame)
                resp_type, resp = self._handle(msg_type, payload)
                conn.sendall(protocol.encode_frame(resp_type, seq, resp))

    def _error(self, code, msg_type):
        return MsgType.ERROR, bytes([code, msg_type])

    def _handle(self, msg_type, payload):
        if msg_type == MsgType.GET_PAR_NAMES:
            return msg_type, b"".join(p.name.encode() + b"\0" for p in self.params)
        if msg_type == MsgType.GET_PAR_INFO:
            return msg_type, b"".join(
                struct.pack("<cHB", p.type_code.encode(), p.count, p.writable)
                for p in self.params
            )
        if msg_type == MsgType.GET_PAR:
            out = b""
            for (index,) in struct.iter_unpack("<H", payload):
                if index >= len(self.params):
                    return self._error(3, msg_type)
                p = self.params[index]
                if p.type_code == "c":
                    out += p.value
                else:
                    out += struct.pack(f"<{p.count}{p.type_code}", *p.value)
            return msg_type, out
        if msg_type == MsgType.SET_PAR:
            (index,) = struct.unpack_from("<H", payload)
            data = payload[2:]
            if index >= len(self.params):
                return self._error(3, msg_type)
            p = self.params[index]
            if not p.writable:
                return self._error(5, msg_type)
            if len(data) != p.size:
                return self._error(4, msg_type)
            p.value = struct.unpack(f"<{p.count}{p.type_code}", data)
            return msg_type, b""
        if msg_type == MsgType.STREAM_SETUP:
            decimation, count, n = struct.unpack_from("<HIB", payload)
            self.stream_setup = (decimation, count, list(payload[7 : 7 + n]))
            return msg_type, b""
        if msg_type == MsgType.STREAM_START:
            (port,) = struct.unpack("<H", payload)
            self.stream_target = port
            return msg_type, b""
        if msg_type == MsgType.STREAM_STOP:
            self.stream_target = None
            return msg_type, b""
        if msg_type == MsgType.STATUS:
            return msg_type, struct.pack("<BHfI", protocol.VERSION, len(self.params), 8000.0, 42_000)
        return self._error(2, msg_type)
