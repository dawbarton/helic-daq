"""End-to-end control and streaming tests against the public simulator."""

import contextlib
import io
import socket
import struct
import tempfile
import unittest
from pathlib import Path

import numpy as np

from helic_daq import Device, protocol
from helic_daq import cli
from helic_daq.discovery import find_devices
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
        self.assertEqual(data["lost_packets"], 0)
        self.assertGreater(np.ptp(data["forcing"]), 0.1)
        np.testing.assert_allclose(data["forcing"], data["out"], atol=1e-6)

    def test_staged_table_is_committed_and_streamed(self):
        table = self.dev.param("table")
        raw = np.asarray([1.0, 2.0], dtype="<f4").tobytes()
        self.dev._request(MsgType.SET_BLOCK, protocol.encode_set_block(table.index, 0, raw))
        self.dev._request(MsgType.COMMIT, protocol.encode_commit(table.index, 2))
        self.assertEqual(self.sim.table, [1.0, 2.0])
        self.dev.set("table_mode", 1)
        data = self.dev.capture(["table", "out"], samples=4, port=0)
        np.testing.assert_allclose(data["table"], 1.0)
        np.testing.assert_allclose(data["out"], 1.0)

    def test_upload_table_helper(self):
        self.dev.upload_table([0.0, 1.0, 0.0, -1.0], duration=0.1, gain=2.0)
        self.assertEqual(self.sim.table, [0.0, 1.0, 0.0, -1.0])
        self.assertEqual(self.dev.get("table_len"), 4)
        self.assertAlmostEqual(self.dev.get("table_freq"), 10.0)
        self.assertEqual(self.dev.get("table_mode"), 1)

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

    def test_find_devices_and_cli(self):
        devices = find_devices(
            timeout=0.1,
            port=self.sim.beacon_port,
            addresses=["127.0.0.1"],
        )
        self.assertEqual(len(devices), 1)
        self.assertEqual(devices[0].experiment, "cbc-rig")

        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            result = cli.main(
                [
                    "find",
                    "--timeout",
                    "0.1",
                    "--discovery-port",
                    str(self.sim.beacon_port),
                    "--address",
                    "127.0.0.1",
                ]
            )
        self.assertEqual(result, 0)
        self.assertIn("cbc-rig", output.getvalue())

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

    def test_cli_list_skips_block_parameter_and_continues(self):
        self.dev.close()
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            result = cli.main(
                [
                    "--host",
                    "127.0.0.1",
                    "--port",
                    str(self.sim.port),
                    "list",
                ]
            )
        shown = output.getvalue()
        self.assertEqual(result, 0)
        self.assertIn("table", shown)
        self.assertIn("<block parameter>", shown)
        self.assertIn("rig_out_channel", shown)

    def test_cli_upload(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "wave.npy"
            np.save(path, [0.0, 1.0, 0.0, -1.0])
            self.dev.close()
            with contextlib.redirect_stdout(io.StringIO()):
                result = cli.main(
                    [
                        "--host",
                        "127.0.0.1",
                        "--port",
                        str(self.sim.port),
                        "upload",
                        str(path),
                        "--duration",
                        "0.2",
                    ]
                )
            self.dev = Device("127.0.0.1", self.sim.port)
        self.assertEqual(result, 0)
        self.assertEqual(self.dev.get("table_len"), 4)
        self.assertAlmostEqual(self.dev.get("table_freq"), 5.0)

    def test_bad_frame_drops_only_that_connection(self):
        self.dev.close()
        with socket.create_connection(("127.0.0.1", self.sim.port)) as bad:
            frame = bytearray(protocol.encode_frame(MsgType.STATUS, 1))
            frame[-1] ^= 0xFF
            bad.sendall(frame)
            bad.settimeout(1.0)
            self.assertEqual(bad.recv(1), b"")
        self.dev = Device("127.0.0.1", self.sim.port)
        self.assertEqual(self.dev.get("experiment"), "cbc-rig")


if __name__ == "__main__":
    unittest.main()
