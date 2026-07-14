"""End-to-end control and streaming tests against the public simulator."""

import contextlib
import io
import socket
import struct
import unittest

import numpy as np

from helic_daq import Device, protocol
from helic_daq import cli
from helic_daq.protocol import MsgType
from helic_daq.sim import Simulator


class TestSimulator(unittest.TestCase):
    def setUp(self):
        self.sim = Simulator(noise=0.0)
        self.dev = Device("127.0.0.1", self.sim.port)

    def tearDown(self):
        self.dev.close()
        self.sim.close()

    def test_finite_capture_end_to_end(self):
        coefficients = [0.0] * 33
        coefficients[17] = 1.0
        self.dev.set("freq", 10.0)
        self.dev.set("forcing_coeffs", coefficients)
        data = self.dev.capture(["forcing", "out"], samples=64, port=0)
        self.assertEqual(len(data["index"]), 64)
        self.assertGreater(np.ptp(data["forcing"]), 0.1)
        np.testing.assert_allclose(data["forcing"], data["out"], atol=1e-6)

    def test_staged_table_is_committed_and_streamed(self):
        table = self.dev._param("table")
        raw = np.asarray([1.0, 2.0], dtype="<f4").tobytes()
        self.dev._request(MsgType.SET_BLOCK, protocol.encode_set_block(table.index, 0, raw))
        self.dev._request(MsgType.COMMIT, protocol.encode_commit(table.index, 2))
        self.assertEqual(self.sim.table, [1.0, 2.0])
        data = self.dev.capture(["table", "out"], samples=4, port=0)
        np.testing.assert_allclose(data["table"], 1.0)
        np.testing.assert_allclose(data["out"], 1.0)

    def test_beacon_response(self):
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as client:
            client.settimeout(1.0)
            client.sendto(
                struct.pack("<HB", protocol.MAGIC, 1),
                ("127.0.0.1", self.sim.beacon_port),
            )
            response, _ = client.recvfrom(64)
        magic, kind, version, port = struct.unpack_from("<HBBH", response)
        self.assertEqual((magic, kind, version, port), (protocol.MAGIC, 2, 2, self.sim.port))
        self.assertEqual(response[12:28].rstrip(b"\0"), b"cbc-rig")

    def test_cli_capture_end_to_end(self):
        self.dev.close()
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            result = cli.main(
                [
                    "--host",
                    "127.0.0.1",
                    "--port",
                    str(self.sim.port),
                    "capture",
                    "--sources",
                    "adc0,out",
                    "--samples",
                    "16",
                ]
            )
        self.assertEqual(result, 0)
        self.assertIn("captured 16 records", output.getvalue())


if __name__ == "__main__":
    unittest.main()
