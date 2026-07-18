"""End-to-end control and streaming tests against the public simulator."""

import contextlib
import io
import socket
import struct
import tempfile
import unittest
from pathlib import Path

import numpy as np

from helic_daq import Device, DeviceError, protocol
from helic_daq import cli
from helic_daq.device import Parameter
from helic_daq.discovery import find_devices
from helic_daq.protocol import MsgType
from helic_daq.sim import COMMAND_EPOCH_MASK, Simulator


class TestCliValueParsing(unittest.TestCase):
    def test_parser_uses_discovered_wire_type(self):
        for type_code in "BbHhIi":
            parameter = Parameter(0, "integer", type_code, 1, True)
            self.assertEqual(cli._parse_values("1", parameter), 1)
        parameter = Parameter(0, "signed", "i", 1, True)
        self.assertEqual(cli._parse_values("-1", parameter), -1)
        parameter = Parameter(0, "float", "f", 1, True)
        self.assertEqual(cli._parse_values("1", parameter), 1.0)
        parameter = Parameter(0, "string", "c", 16, True)
        self.assertEqual(cli._parse_values("hello world", parameter), "hello world")

    def test_parser_handles_arrays_and_reports_bad_input(self):
        parameter = Parameter(0, "values", "f", 3, True)
        self.assertEqual(cli._parse_values("1, 2 3", parameter), [1.0, 2.0, 3.0])
        with self.assertRaisesRegex(DeviceError, "expects 3 value"):
            cli._parse_values("1,2", parameter)

        parameter = Parameter(0, "mode", "I", 1, True)
        with self.assertRaisesRegex(DeviceError, "invalid integer value"):
            cli._parse_values("1.5", parameter)


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

    def test_command_epoch_tracks_rt_commands_and_wraps_exactly(self):
        initial = self.dev.capture(["cmd_epoch"], samples=1, port=0)
        self.assertEqual(initial["cmd_epoch"][0], 0.0)

        self.dev.set("freq", 10.0)
        self.dev.set("diag_reset", 1)
        self.dev.set("ctrl_reset", 0)
        changed = self.dev.capture(["cmd_epoch"], samples=1, port=0)
        self.assertEqual(changed["cmd_epoch"][0], 1.0)

        table = self.dev.param("table")
        raw = np.asarray([0.0, 1.0], dtype="<f4").tobytes()
        self.dev._request(
            MsgType.SET_BLOCK,
            protocol.encode_set_block(table.index, 0, raw),
        )
        staged = self.dev.capture(["cmd_epoch"], samples=1, port=0)
        self.assertEqual(staged["cmd_epoch"][0], 1.0)
        self.dev._request(MsgType.COMMIT, protocol.encode_commit(table.index, 2))
        committed = self.dev.capture(["cmd_epoch"], samples=1, port=0)
        self.assertEqual(committed["cmd_epoch"][0], 2.0)

        self.sim._cmd_epoch = COMMAND_EPOCH_MASK
        self.dev.set("table_gain", 2.0)
        wrapped = self.dev.capture(["cmd_epoch"], samples=1, port=0)
        self.assertEqual(wrapped["cmd_epoch"][0], 0.0)

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
        self.dev.upload_table(
            [0.0, 1.0, 0.0, -1.0],
            duration=0.1,
            gain=2.0,
            interpolation="hold",
        )
        self.assertEqual(self.sim.table, [0.0, 1.0, 0.0, -1.0])
        self.assertEqual(self.dev.get("table_len"), 4)
        self.assertAlmostEqual(self.dev.get("table_freq"), 10.0)
        self.assertEqual(self.dev.get("table_interp"), 0)
        self.assertEqual(self.dev.get("table_mode"), 1)

    def test_table_interpolation_changes_simulated_shape(self):
        self.dev.upload_table([0.0, 1.0], freq=1000.0, interpolation="hold")
        held = [self.sim._table_value(t) for t in (0.000125, 0.000375, 0.000625)]
        np.testing.assert_allclose(held, [0.0, 0.0, 1.0])

        self.dev.set("table_interp", 1)
        linear = [self.sim._table_value(t) for t in (0.000125, 0.000375, 0.000625)]
        np.testing.assert_allclose(linear, [0.25, 0.75, 0.75], atol=1e-6)

    def test_timing_diagnostics_match_firmware_registry(self):
        names = [param.name for param in self.dev.params]
        self.assertEqual(
            names[23:32],
            [
                "wake_phase_min",
                "wake_phase_max",
                "t_measure_max",
                "t_actuate_max",
                "t_rest_max",
                "diag_reset",
                "cmd_backlog_max",
                "arm",
                "safety",
            ],
        )
        self.sim._by_name["loop_time_max"].value = 42
        self.sim._by_name["laser_uart_errors"].value = 7
        self.sim._by_name["laser_frames_received"].value = 123
        self.dev.set("diag_reset", 1)
        self.assertEqual(
            self.dev.get(
                "loop_time_max",
                "diag_reset",
                "laser_uart_errors",
                "laser_frames_received",
            ),
            [0, 0, 0, 123],
        )

    def test_arm_is_direct_and_disconnect_disarms(self):
        initial_epoch = self.sim._cmd_epoch
        self.dev.set("arm", 1)
        self.assertEqual(self.dev.get("arm"), 1)
        self.assertEqual(self.dev.get("safety") & 1, 1)
        self.assertEqual(self.sim._cmd_epoch, initial_epoch)

        self.dev.close()
        self.dev = Device("127.0.0.1", self.sim.port)
        self.assertEqual(self.dev.get("arm"), 0)
        self.assertEqual(self.dev.get("safety") & 1, 0)

    def test_beacon_response(self):
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as client:
            client.settimeout(1.0)
            client.sendto(
                struct.pack("<HB", protocol.MAGIC, 1),
                ("127.0.0.1", self.sim.beacon_port),
            )
            response, _ = client.recvfrom(64)
        magic, kind, version, port = struct.unpack_from("<HBBH", response)
        self.assertEqual(
            (magic, kind, version, port),
            (protocol.MAGIC, 2, protocol.VERSION, self.sim.port),
        )
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

    def test_cli_set_uses_discovered_integer_and_float_types(self):
        self.dev.close()
        for name, value in (("table_mode", "2"), ("freq", "17.5")):
            with contextlib.redirect_stdout(io.StringIO()):
                result = cli.main(
                    [
                        "--host",
                        "127.0.0.1",
                        "--port",
                        str(self.sim.port),
                        "set",
                        name,
                        value,
                    ]
                )
            self.assertEqual(result, 0)
        self.dev = Device("127.0.0.1", self.sim.port)
        self.assertEqual(self.dev.get("table_mode"), 2)
        self.assertEqual(self.dev.get("freq"), 17.5)

    def test_cli_bad_integer_is_a_concise_error(self):
        self.dev.close()
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            result = cli.main(
                [
                    "--host",
                    "127.0.0.1",
                    "--port",
                    str(self.sim.port),
                    "set",
                    "table_mode",
                    "1.5",
                ]
            )
        self.dev = Device("127.0.0.1", self.sim.port)
        self.assertEqual(result, 1)
        self.assertIn("invalid integer value", stderr.getvalue())
        self.assertNotIn("Traceback", stderr.getvalue())

    def test_cli_diag_reset_command(self):
        self.sim._by_name["loop_time_max"].value = 42
        self.dev.close()
        with contextlib.redirect_stdout(io.StringIO()):
            result = cli.main(
                [
                    "--host",
                    "127.0.0.1",
                    "--port",
                    str(self.sim.port),
                    "set",
                    "diag_reset",
                    "1",
                ]
            )
        self.assertEqual(result, 0)
        self.assertEqual(self.sim._by_name["loop_time_max"].value, 0)

        self.sim._by_name["loop_time_max"].value = 42
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            result = cli.main(
                [
                    "--host",
                    "127.0.0.1",
                    "--port",
                    str(self.sim.port),
                    "diag-reset",
                ]
            )
        self.dev = Device("127.0.0.1", self.sim.port)
        self.assertEqual(result, 0)
        self.assertEqual(output.getvalue().strip(), "diagnostics reset")
        self.assertEqual(self.dev.get("loop_time_max"), 0)

    def test_cli_refuses_one_shot_arm(self):
        self.dev.close()
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            result = cli.main(
                [
                    "--host",
                    "127.0.0.1",
                    "--port",
                    str(self.sim.port),
                    "set",
                    "arm",
                    "1",
                ]
            )
        self.dev = Device("127.0.0.1", self.sim.port)
        self.assertEqual(result, 1)
        self.assertIn("persistent Python session", stderr.getvalue())
        self.assertEqual(self.dev.get("arm"), 0)

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
                        "--interpolation",
                        "hold",
                    ]
                )
            self.dev = Device("127.0.0.1", self.sim.port)
        self.assertEqual(result, 0)
        self.assertEqual(self.dev.get("table_len"), 4)
        self.assertAlmostEqual(self.dev.get("table_freq"), 5.0)
        self.assertEqual(self.dev.get("table_interp"), 0)

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
