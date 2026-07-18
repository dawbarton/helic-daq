"""Device tests against the public simulator over real TCP loopback."""

import struct
import unittest

from helic_daq import Device, DeviceError, protocol
from helic_daq.protocol import ProtocolError

from helic_daq.sim import SimParam, Simulator, default_params


class NonProgressingSimulator(Simulator):
    def _handle(self, msg_type, payload, peer):
        if msg_type == protocol.MsgType.GET_PARAMS:
            (start,) = struct.unpack("<H", payload)
            return msg_type, struct.pack("<HH", start, start)
        return super()._handle(msg_type, payload, peer)


class TestDevice(unittest.TestCase):
    def setUp(self):
        self.sim = Simulator()
        self.dev = Device("127.0.0.1", port=self.sim.port)

    def tearDown(self):
        self.dev.close()
        self.sim.close()

    def test_discovery(self):
        names = [p.name for p in self.dev.params]
        self.assertEqual(names[:4], ["firmware", "experiment", "sample_freq", "ticks"])
        self.assertIn("table", names)
        self.assertIn("arm", names)
        self.assertIn("safety", names)
        self.assertIn("rig_laser_range", names)
        self.assertIn("laser_frames_received", names)
        coeffs = self.dev.param("forcing_coeffs")
        self.assertEqual(coeffs.type_code, "f")
        self.assertEqual(coeffs.count, 33)
        self.assertTrue(coeffs.writable)
        self.assertFalse(self.dev.params[0].writable)

    def test_multi_page_discovery_preserves_late_parameter_indices(self):
        params = default_params(8000.0)
        params.extend(
            SimParam(f"paged_extra_{index:03d}", "f", 1, True, float(index))
            for index in range(50)
        )
        with Simulator(params=params) as simulator:
            with Device("127.0.0.1", port=simulator.port) as device:
                late = device.param("paged_extra_049")
                self.assertEqual(late.index, len(params) - 1)
                self.assertEqual(device.get(late.name), 49.0)
                device.set(late.name, 12.5)
                self.assertEqual(device.get(late.name), 12.5)

    def test_non_progressing_parameter_page_is_rejected(self):
        with NonProgressingSimulator() as simulator:
            with self.assertRaisesRegex(ProtocolError, "did not advance"):
                Device("127.0.0.1", port=simulator.port)

    def test_get_scalar_and_string(self):
        self.assertEqual(self.dev.get("firmware"), "helic-daq sim")
        self.assertEqual(self.dev.get("experiment"), "cbc-rig")
        self.assertEqual(self.dev.get("sample_freq"), 8000.0)
        self.assertEqual(self.dev.get("ticks"), 0)

    def test_multi_get_single_round_trip(self):
        fs, ticks = self.dev.get("sample_freq", "ticks")
        self.assertEqual((fs, ticks), (8000.0, 0))

    def test_oversize_get_is_rejected_locally(self):
        with self.assertRaisesRegex(DeviceError, str(protocol.MAX_PAYLOAD)):
            self.dev.get("table")

    def test_set_and_read_back(self):
        self.dev.set("freq", 17.5)
        self.assertEqual(self.dev.get("freq"), 17.5)

    def test_invalid_rig_values_are_rejected_without_changing_shadow(self):
        for name, value, initial in [
            ("rig_laser_range", 0.0, 50.0),
            ("rig_out_channel", 7.0, 0.0),
            ("rig_out_channel", 1.5, 0.0),
        ]:
            with self.assertRaises(DeviceError):
                self.dev.set(name, value)
            self.assertEqual(self.dev.get(name), initial)

    def test_array_round_trip(self):
        coeffs = [0.0] * 33
        coeffs[17] = 1.25  # b1
        self.dev.set("forcing_coeffs", coeffs)
        self.assertEqual(self.dev.get("forcing_coeffs"), coeffs)

    def test_attribute_access(self):
        self.dev.par.rig_laser_range = 20.0
        self.assertEqual(self.dev.par.rig_laser_range, 20.0)

    def test_read_only_rejected_locally(self):
        with self.assertRaises(DeviceError):
            self.dev.set("ticks", 0)

    def test_invalid_value_type_is_reported_as_device_error(self):
        with self.assertRaisesRegex(DeviceError, "invalid value.*diag_reset"):
            self.dev.set("diag_reset", 1.0)
        with self.assertRaisesRegex(DeviceError, "invalid value.*diag_reset"):
            self.dev.set("diag_reset", -1)

    def test_wrong_array_length_rejected(self):
        with self.assertRaises(DeviceError):
            self.dev.set("forcing_coeffs", [1.0, 2.0])

    def test_unknown_parameter(self):
        with self.assertRaises(DeviceError):
            self.dev.get("nonexistent")
        with self.assertRaises(DeviceError):
            self.dev.get(-1)

    def test_status(self):
        status = self.dev.status()
        self.assertEqual(status["sample_rate"], 8000.0)
        self.assertEqual(status["n_params"], len(self.sim.params))
        self.assertEqual(status["n_sources"], 14)
        self.assertGreaterEqual(status["uptime_s"], 0.0)

    def test_stream_setup_and_start(self):
        names = self.dev.stream_setup(["adc0", "out"], decimation=4, count=0)
        self.assertEqual(names, ["adc0", "out"])
        self.assertEqual(self.sim.stream_setup, (4, 0, [0, 12]))
        self.dev.stream_start(2351)
        self.assertEqual(self.sim.stream_target, ("127.0.0.1", 2351))
        self.dev.stream_stop()
        self.assertIsNone(self.sim.stream_target)

    def test_unknown_source_rejected(self):
        with self.assertRaisesRegex(DeviceError, r"adc0 \[V\].*laser \[mm\]"):
            self.dev.stream_setup(["bogus"])
        with self.assertRaises(DeviceError):
            self.dev.stream_setup([-1])

    def test_source_discovery(self):
        self.assertEqual((self.dev.sources[8].name, self.dev.sources[8].unit), ("laser", "mm"))
        self.assertEqual(
            (self.dev.sources[-1].name, self.dev.sources[-1].unit),
            ("cmd_epoch", "count"),
        )

    def test_protocol_version_mismatch_is_clear(self):
        with Simulator(version=2) as old:
            with self.assertRaisesRegex(DeviceError, "protocol version mismatch: device 2, host 3"):
                Device("127.0.0.1", port=old.port)


if __name__ == "__main__":
    unittest.main()
