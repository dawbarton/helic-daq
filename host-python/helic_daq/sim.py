"""Protocol-v2 HELIC-DAQ simulator with synthetic UDP streaming."""

from __future__ import annotations

import argparse
import math
import random
import socket
import struct
import threading
import time
from dataclasses import dataclass

from . import protocol
from .protocol import MsgType, ProtocolError, StreamHeader


@dataclass
class SimParam:
    name: str
    type_code: str
    count: int
    writable: bool
    value: object

    @property
    def size(self) -> int:
        return struct.calcsize(self.type_code) * self.count

    def pack(self) -> bytes:
        if self.type_code == "c":
            return str(self.value).encode()[: self.count].ljust(self.count, b"\0")
        values = [self.value] if self.count == 1 else list(self.value)
        return struct.pack(f"<{self.count}{self.type_code}", *values)

    def unpack(self, raw: bytes) -> object:
        values = struct.unpack(f"<{self.count}{self.type_code}", raw)
        return values[0] if self.count == 1 else list(values)


def default_params(sample_rate: float) -> list[SimParam]:
    coefficients = [0.0] * 33
    return [
        SimParam("firmware", "c", 16, False, "helic-daq sim"),
        SimParam("experiment", "c", 16, False, "cbc-rig"),
        SimParam("sample_freq", "f", 1, False, sample_rate),
        SimParam("ticks", "I", 1, False, 0),
        SimParam("loop_time_last", "I", 1, False, 5),
        SimParam("loop_time_max", "I", 1, False, 7),
        SimParam("clock_jitter", "I", 1, False, 0),
        SimParam("overruns", "I", 1, False, 0),
        SimParam("tick_timeouts", "I", 1, False, 0),
        SimParam("records_dropped", "I", 1, False, 0),
        SimParam("freq", "f", 1, True, 0.0),
        SimParam("target_coeffs", "f", 33, True, coefficients.copy()),
        SimParam("forcing_coeffs", "f", 33, True, coefficients.copy()),
        SimParam("ctrl_reset", "I", 1, True, 0),
        SimParam("table", "f", 4096, True, [0.0] * 4096),
        SimParam("table_len", "H", 1, False, 0),
        SimParam("table_freq", "f", 1, True, 0.0),
        SimParam("table_gain", "f", 1, True, 1.0),
        SimParam("table_mode", "I", 1, True, 0),
        SimParam("table_mult", "I", 1, True, 1),
        SimParam("table_phase", "f", 1, True, 0.0),
        SimParam("table_trigger", "I", 1, True, 0),
        SimParam("wake_phase_min", "I", 1, False, 0),
        SimParam("wake_phase_max", "I", 1, False, 0),
        SimParam("t_measure_max", "I", 1, False, 0),
        SimParam("t_actuate_max", "I", 1, False, 0),
        SimParam("t_rest_max", "I", 1, False, 0),
        SimParam("diag_reset", "I", 1, True, 0),
        SimParam("cmd_backlog_max", "I", 1, False, 0),
        SimParam("laser", "f", 1, False, 25.0),
        SimParam("rig_laser_range", "f", 1, True, 50.0),
        SimParam("rig_out_channel", "f", 1, True, 0.0),
    ]


def default_sources() -> list[tuple[str, str]]:
    return [(f"adc{i}", "V") for i in range(8)] + [
        ("laser", "mm"),
        ("target", "V"),
        ("forcing", "V"),
        ("table", "V"),
        ("out", "V"),
    ]


class Simulator:
    """A localhost-compatible v2 device used by tests and host development."""

    def __init__(
        self,
        host: str = "127.0.0.1",
        port: int = 0,
        sample_rate: float = 8000.0,
        noise: float = 0.001,
        version: int = protocol.VERSION,
        beacon_port: int | None = None,
    ):
        self.params = default_params(sample_rate)
        self.sources = default_sources()
        self.version = version
        self.noise = noise
        self.stream_setup = None
        self.stream_target = None
        self.table: list[float] = []
        self._table_trigger_time: float | None = None
        self._staging = [0.0] * 4096
        self._by_name = {param.name: param for param in self.params}
        self._started = time.monotonic()
        self._lock = threading.Lock()
        self._closed = threading.Event()
        self._connection: socket.socket | None = None
        self._stream_generation = 0
        self._listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self._listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self._listener.bind((host, port))
        self._listener.listen(1)
        self._listener.settimeout(0.1)
        self.host, self.port = self._listener.getsockname()
        if beacon_port is None:
            beacon_port = protocol.DISCOVERY_PORT if port == protocol.CONTROL_PORT else 0
        self._beacon = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self._beacon.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self._beacon.bind((host, beacon_port))
        self._beacon.settimeout(0.1)
        self.beacon_port = self._beacon.getsockname()[1]
        self._thread = threading.Thread(target=self._serve, daemon=True)
        self._beacon_thread = threading.Thread(target=self._serve_beacon, daemon=True)
        self._thread.start()
        self._beacon_thread.start()

    def __enter__(self) -> "Simulator":
        return self

    def __exit__(self, *exc) -> None:
        self.close()

    def close(self) -> None:
        self._closed.set()
        self._listener.close()
        self._beacon.close()
        connection = self._connection
        if connection is not None:
            try:
                connection.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            connection.close()
        self._thread.join(timeout=1.0)
        self._beacon_thread.join(timeout=1.0)

    def _serve_beacon(self) -> None:
        while not self._closed.is_set():
            try:
                payload, peer = self._beacon.recvfrom(64)
            except (OSError, socket.timeout):
                continue
            if payload != protocol.BEACON_REQUEST:
                continue
            response = protocol.encode_beacon_response(
                protocol.BeaconResponse(
                    self.version,
                    self.port,
                    b"\x02HL\x00\x00\x01",
                    str(self._by_name["experiment"].value),
                    str(self._by_name["firmware"].value),
                )
            )
            self._beacon.sendto(response, peer)

    def _serve(self) -> None:
        while not self._closed.is_set():
            try:
                connection, peer = self._listener.accept()
            except (OSError, socket.timeout):
                continue
            self._connection = connection
            with connection:
                self._serve_connection(connection, peer[0])
            self._connection = None
            with self._lock:
                self.stream_target = None
                self._stream_generation += 1

    def _serve_connection(self, connection: socket.socket, peer: str) -> None:
        buf = b""
        while not self._closed.is_set():
            try:
                data = connection.recv(4096)
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
                try:
                    msg_type, seq, payload = protocol.decode_frame(frame)
                except ProtocolError:
                    return
                response_type, response = self._handle(msg_type, payload, peer)
                try:
                    connection.sendall(protocol.encode_frame(response_type, seq, response))
                except OSError:
                    return

    @staticmethod
    def _error(code: int, msg_type: int) -> tuple[int, bytes]:
        return MsgType.ERROR, bytes([code, msg_type])

    def _handle(self, msg_type: int, payload: bytes, peer: str) -> tuple[int, bytes]:
        if msg_type == MsgType.STATUS:
            uptime_ms = int((time.monotonic() - self._started) * 1000) & 0xFFFFFFFF
            return msg_type, struct.pack(
                "<BHBfI",
                self.version,
                len(self.params),
                len(self.sources),
                self._by_name["sample_freq"].value,
                uptime_ms,
            )
        if msg_type == MsgType.GET_PARAMS:
            response = b"".join(
                param.name.encode()
                + b"\0"
                + struct.pack("<cHB", param.type_code.encode(), param.count, param.writable)
                for param in self.params
            )
            return msg_type, response
        if msg_type == MsgType.GET_SOURCES:
            return msg_type, b"".join(
                name.encode() + b"\0" + unit.encode() + b"\0"
                for name, unit in self.sources
            )
        if msg_type == MsgType.GET_PAR:
            if not payload or len(payload) % 2:
                return self._error(4, msg_type)
            response = b""
            for (index,) in struct.iter_unpack("<H", payload):
                if index >= len(self.params):
                    return self._error(3, msg_type)
                raw = self.params[index].pack()
                if len(response) + len(raw) > protocol.MAX_PAYLOAD:
                    return self._error(4, msg_type)
                response += raw
            return msg_type, response
        if msg_type == MsgType.SET_PAR:
            if len(payload) < 2:
                return self._error(4, msg_type)
            (index,) = struct.unpack_from("<H", payload)
            if index >= len(self.params):
                return self._error(3, msg_type)
            param = self.params[index]
            raw = payload[2:]
            if not param.writable:
                return self._error(5, msg_type)
            if len(raw) != param.size:
                return self._error(4, msg_type)
            value = param.unpack(raw)
            values = [value] if param.count == 1 else value
            if param.type_code == "f" and not all(math.isfinite(v) for v in values):
                return self._error(6, msg_type)
            if param.name == "freq" and not 0.0 <= value < self._by_name["sample_freq"].value / 2:
                return self._error(6, msg_type)
            if param.name == "table_freq" and not 0.0 <= value < self._by_name["sample_freq"].value / 2:
                return self._error(6, msg_type)
            if param.name == "table_gain" and not math.isfinite(value):
                return self._error(6, msg_type)
            if param.name == "table_mode" and value not in range(5):
                return self._error(6, msg_type)
            if param.name == "table_mult" and value < 1:
                return self._error(6, msg_type)
            if param.name == "table_phase" and not 0.0 <= value < 1.0:
                return self._error(6, msg_type)
            if param.name == "rig_laser_range" and value <= 0.0:
                return self._error(6, msg_type)
            if param.name == "rig_out_channel" and (
                value < 0.0 or value >= 4.0 or not value.is_integer()
            ):
                return self._error(6, msg_type)
            param.value = value
            if param.name == "diag_reset" and value:
                for name in (
                    "loop_time_max",
                    "clock_jitter",
                    "overruns",
                    "tick_timeouts",
                    "records_dropped",
                    "wake_phase_min",
                    "wake_phase_max",
                    "t_measure_max",
                    "t_actuate_max",
                    "t_rest_max",
                    "cmd_backlog_max",
                ):
                    self._by_name[name].value = 0
                param.value = 0
            if param.name == "table_trigger" and value:
                self._table_trigger_time = (
                    self._by_name["ticks"].value / self._by_name["sample_freq"].value
                )
                param.value = 0
            return msg_type, b""
        if msg_type == MsgType.SET_BLOCK:
            if len(payload) < 6:
                return self._error(4, msg_type)
            index, offset = struct.unpack_from("<HI", payload)
            raw = payload[6:]
            table_index = self.params.index(self._by_name["table"])
            if index != table_index:
                return self._error(3, msg_type)
            if not raw or len(raw) % 4 or offset + len(raw) // 4 > len(self._staging):
                return self._error(4, msg_type)
            values = struct.unpack(f"<{len(raw) // 4}f", raw)
            self._staging[offset : offset + len(values)] = values
            return msg_type, b""
        if msg_type == MsgType.COMMIT:
            if len(payload) != 6:
                return self._error(4, msg_type)
            index, length = struct.unpack("<HI", payload)
            table_index = self.params.index(self._by_name["table"])
            if index != table_index:
                return self._error(3, msg_type)
            if not 2 <= length <= len(self._staging):
                return self._error(6, msg_type)
            values = self._staging[:length]
            if not all(math.isfinite(value) for value in values):
                return self._error(6, msg_type)
            self.table = values.copy()
            self._by_name["table_len"].value = length
            return msg_type, b""
        if msg_type == MsgType.STREAM_SETUP:
            if len(payload) < 7:
                return self._error(4, msg_type)
            decimation, count, n_sources = struct.unpack_from("<HIB", payload)
            sources = list(payload[7:])
            if (
                len(payload) != 7 + n_sources
                or decimation == 0
                or n_sources == 0
                or any(source >= len(self.sources) for source in sources)
            ):
                return self._error(6, msg_type)
            with self._lock:
                if self.stream_target is not None:
                    return self._error(protocol.ERROR_BUSY, msg_type)
                self.stream_setup = (decimation, count, sources)
            return msg_type, b""
        if msg_type == MsgType.STREAM_START:
            if len(payload) != 2 or self.stream_setup is None:
                return self._error(6, msg_type)
            (port,) = struct.unpack("<H", payload)
            if port == 0:
                return self._error(6, msg_type)
            with self._lock:
                self.stream_target = (peer, port)
                self._stream_generation += 1
                generation = self._stream_generation
            threading.Thread(target=self._stream, args=(generation,), daemon=True).start()
            return msg_type, b""
        if msg_type == MsgType.STREAM_STOP:
            with self._lock:
                self.stream_target = None
                self._stream_generation += 1
            return msg_type, b""
        return self._error(2, msg_type)

    @staticmethod
    def _fourier(coefficients: list[float], theta: float) -> float:
        harmonics = (len(coefficients) - 1) // 2
        value = coefficients[0]
        for k in range(1, harmonics + 1):
            value += coefficients[k] * math.cos(k * theta)
            value += coefficients[harmonics + k] * math.sin(k * theta)
        return value

    def _table_value(self, t: float) -> float:
        if len(self.table) < 2:
            return 0.0
        mode = self._by_name["table_mode"].value
        if mode == 0:
            return 0.0
        if mode in (1, 2):
            frequency = self._by_name["table_freq"].value
            origin = self._table_trigger_time if mode == 2 else 0.0
            if origin is None:
                return 0.0
            elapsed = t - origin
            if mode == 2 and not 0.0 <= elapsed * frequency < 1.0:
                return 0.0
            phase = (elapsed * frequency) % 1.0
        else:
            frequency = self._by_name["freq"].value
            multiplier = self._by_name["table_mult"].value
            phase = (
                t * frequency * multiplier + self._by_name["table_phase"].value
            ) % 1.0
            if mode == 4:
                if self._table_trigger_time is None or frequency == 0.0:
                    return 0.0
                start = math.ceil(self._table_trigger_time * frequency) / frequency
                if not start <= t < start + 1.0 / (frequency * multiplier):
                    return 0.0
        position = phase * len(self.table)
        index = int(position)
        fraction = position - index
        following = (index + 1) % len(self.table)
        value = self.table[index] + fraction * (self.table[following] - self.table[index])
        return self._by_name["table_gain"].value * value

    def _values(self, index: int, rng: random.Random) -> list[float]:
        sample_rate = self._by_name["sample_freq"].value
        t = index / sample_rate
        theta = 2.0 * math.pi * self._by_name["freq"].value * t
        target = self._fourier(self._by_name["target_coeffs"].value, theta)
        forcing = self._fourier(self._by_name["forcing_coeffs"].value, theta)
        table = self._table_value(t)
        out = target + forcing + table
        inputs = [out + rng.gauss(0.0, self.noise) for _ in range(8)]
        laser = 25.0 + 0.1 * math.sin(theta) + rng.gauss(0.0, self.noise)
        self._by_name["laser"].value = laser
        return inputs + [laser, target, forcing, table, out]

    def _stream(self, generation: int) -> None:
        with self._lock:
            target = self.stream_target
            decimation, count, sources = self.stream_setup
        if target is None:
            return
        udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        rng = random.Random(0)
        sequence = 0
        index = int(self._by_name["ticks"].value)
        remaining = count
        max_records = max(1, (1472 - protocol.STREAM_HEADER_LEN) // (4 * len(sources)))
        try:
            while not self._closed.is_set():
                with self._lock:
                    if generation != self._stream_generation or self.stream_target is None:
                        return
                n_records = min(max_records, remaining) if count else min(max_records, 40)
                rows = []
                first_index = index
                for _ in range(n_records):
                    values = self._values(index, rng)
                    rows.extend(values[source] for source in sources)
                    index += decimation
                header = protocol.encode_stream_header(
                    StreamHeader(len(sources), sequence, first_index, 0, decimation, n_records)
                )
                udp.sendto(header + struct.pack(f"<{len(rows)}f", *rows), target)
                sequence = (sequence + 1) & 0xFFFFFFFF
                self._by_name["ticks"].value = index
                if count:
                    remaining -= n_records
                    if remaining == 0:
                        with self._lock:
                            if generation == self._stream_generation:
                                self.stream_target = None
                        return
                else:
                    time.sleep(n_records * decimation / self._by_name["sample_freq"].value)
        finally:
            udp.close()


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=protocol.CONTROL_PORT)
    parser.add_argument("--sample-rate", type=float, default=8000.0)
    parser.add_argument("--noise", type=float, default=0.001)
    args = parser.parse_args(argv)
    with Simulator(args.host, args.port, args.sample_rate, args.noise) as simulator:
        print(f"HELIC-DAQ simulator listening on {simulator.host}:{simulator.port}")
        try:
            while True:
                time.sleep(1.0)
        except KeyboardInterrupt:
            pass
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
