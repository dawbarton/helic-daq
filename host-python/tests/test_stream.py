"""StreamReceiver tests: crafted packets over UDP loopback."""

import socket
import struct
import unittest

import numpy as np

from helic_daq import StreamReceiver
from helic_daq.protocol import StreamHeader, encode_stream_header


def make_packet(seq, first_index, values, decimation=1, dropped=0):
    """values: list of records, each a list of floats."""
    n_records = len(values)
    n_sources = len(values[0]) if n_records else 0
    header = StreamHeader(
        n_sources=n_sources,
        seq=seq,
        first_index=first_index,
        dropped=dropped,
        decimation=decimation,
        n_records=n_records,
    )
    flat = [v for record in values for v in record]
    return encode_stream_header(header) + struct.pack(f"<{len(flat)}f", *flat)


class TestStreamReceiver(unittest.TestCase):
    def setUp(self):
        self.rx = StreamReceiver(port=0)  # ephemeral port
        self.port = self.rx._sock.getsockname()[1]
        self.tx = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)

    def tearDown(self):
        self.tx.close()
        self.rx.close()

    def send(self, packet):
        self.tx.sendto(packet, ("127.0.0.1", self.port))

    def test_recv_single_packet(self):
        self.send(make_packet(0, 100, [[1.0, 2.0], [3.0, 4.0]]))
        header, values = self.rx.recv()
        self.assertEqual(header.first_index, 100)
        self.assertEqual(values.shape, (2, 2))
        np.testing.assert_array_equal(values, [[1.0, 2.0], [3.0, 4.0]])

    def test_lost_packet_detection(self):
        self.send(make_packet(0, 0, [[1.0]]))
        self.send(make_packet(3, 10, [[2.0]]))  # packets 1, 2 lost
        self.rx.recv()
        self.rx.recv()
        self.assertEqual(self.rx.lost_packets, 2)

    def test_capture_assembles_and_indexes(self):
        # Two packets, decimation 2: indices step by 2 within a packet.
        self.send(make_packet(0, 0, [[0.0], [1.0], [2.0]], decimation=2))
        self.send(make_packet(1, 6, [[3.0], [4.0]], decimation=2, dropped=7))
        data = self.rx.capture(5, ["out"])
        np.testing.assert_array_equal(data["out"], [0.0, 1.0, 2.0, 3.0, 4.0])
        np.testing.assert_array_equal(data["index"], [0, 2, 4, 6, 8])
        self.assertEqual(data["dropped"], 7)
        self.assertEqual(data["lost_packets"], 0)

    def test_capture_truncates_to_requested_length(self):
        self.send(make_packet(0, 0, [[1.0], [2.0], [3.0], [4.0]]))
        data = self.rx.capture(2, ["adc0"])
        self.assertEqual(len(data["adc0"]), 2)

    def test_prime_sends_from_receive_socket(self):
        sink = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        try:
            sink.bind(("127.0.0.1", 0))
            sink.settimeout(1.0)
            self.rx.prime("127.0.0.1", sink.getsockname()[1])
            data, address = sink.recvfrom(64)
            self.assertEqual(data, b"helic-daq-stream-prime")
            self.assertEqual(address[1], self.port)
        finally:
            sink.close()


if __name__ == "__main__":
    unittest.main()
