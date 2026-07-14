"""Device tests against the in-process emulator (real TCP loopback)."""

import unittest

from helic_daq import Device, DeviceError

from emulator import Emulator


class TestDevice(unittest.TestCase):
    def setUp(self):
        self.emu = Emulator()
        self.dev = Device("127.0.0.1", port=self.emu.port)

    def tearDown(self):
        self.dev.close()
        self.emu.close()

    def test_discovery(self):
        names = [p.name for p in self.dev.params]
        self.assertEqual(
            names,
            [
                "firmware",
                "experiment",
                "sample_freq",
                "ticks",
                "freq",
                "forcing_coeffs",
                "ctrl_kp",
            ],
        )
        coeffs = self.dev.params[5]
        self.assertEqual(coeffs.type_code, "f")
        self.assertEqual(coeffs.count, 33)
        self.assertTrue(coeffs.writable)
        self.assertFalse(self.dev.params[0].writable)

    def test_get_scalar_and_string(self):
        self.assertEqual(self.dev.get("firmware"), "helic-daq emu")
        self.assertEqual(self.dev.get("experiment"), "cbc-rig")
        self.assertEqual(self.dev.get("sample_freq"), 8000.0)
        self.assertEqual(self.dev.get("ticks"), 12345)

    def test_multi_get_single_round_trip(self):
        fs, ticks = self.dev.get("sample_freq", "ticks")
        self.assertEqual((fs, ticks), (8000.0, 12345))

    def test_set_and_read_back(self):
        self.dev.set("freq", 17.5)
        self.assertEqual(self.dev.get("freq"), 17.5)

    def test_array_round_trip(self):
        coeffs = [0.0] * 33
        coeffs[17] = 1.25  # b1
        self.dev.set("forcing_coeffs", coeffs)
        self.assertEqual(self.dev.get("forcing_coeffs"), coeffs)

    def test_attribute_access(self):
        self.dev.par.ctrl_kp = 2.5
        self.assertEqual(self.dev.par.ctrl_kp, 2.5)

    def test_read_only_rejected_locally(self):
        with self.assertRaises(DeviceError):
            self.dev.set("ticks", 0)

    def test_wrong_array_length_rejected(self):
        with self.assertRaises(DeviceError):
            self.dev.set("forcing_coeffs", [1.0, 2.0])

    def test_unknown_parameter(self):
        with self.assertRaises(DeviceError):
            self.dev.get("nonexistent")

    def test_status(self):
        status = self.dev.status()
        self.assertEqual(status["sample_rate"], 8000.0)
        self.assertEqual(status["n_params"], 7)
        self.assertEqual(status["n_sources"], 13)
        self.assertEqual(status["uptime_s"], 42.0)

    def test_stream_setup_and_start(self):
        names = self.dev.stream_setup(["adc0", "out"], decimation=4, count=100)
        self.assertEqual(names, ["adc0", "out"])
        self.assertEqual(self.emu.stream_setup, (4, 100, [0, 12]))
        self.dev.stream_start(2351)
        self.assertEqual(self.emu.stream_target, 2351)
        self.dev.stream_stop()
        self.assertIsNone(self.emu.stream_target)

    def test_unknown_source_rejected(self):
        with self.assertRaisesRegex(DeviceError, r"adc0 \[V\].*laser \[mm\]"):
            self.dev.stream_setup(["bogus"])

    def test_source_discovery(self):
        self.assertEqual((self.dev.sources[8].name, self.dev.sources[8].unit), ("laser", "mm"))
        self.assertEqual(self.dev.sources[-1].name, "out")

    def test_protocol_version_mismatch_is_clear(self):
        with Emulator(version=1) as old:
            with self.assertRaisesRegex(DeviceError, "protocol version mismatch: device 1, host 2"):
                Device("127.0.0.1", port=old.port)


if __name__ == "__main__":
    unittest.main()
