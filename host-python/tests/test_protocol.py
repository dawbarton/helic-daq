"""Protocol codec tests, including the known-answer vectors shared with the
Rust implementation (see docs/protocol.md and helic-proto)."""

import unittest

from helic_daq import protocol
from helic_daq.protocol import MsgType, ProtocolError, StreamHeader


class TestCrc16(unittest.TestCase):
    def test_known_answers(self):
        # Same vectors as helic-proto/src/crc.rs.
        self.assertEqual(protocol.crc16(b"123456789"), 0x29B1)
        self.assertEqual(protocol.crc16(b""), 0xFFFF)
        self.assertEqual(protocol.crc16(bytes([0x00])), 0xE1F0)
        self.assertEqual(protocol.crc16(bytes([0x0A, 0x01, 0x00, 0x00])), 0xDB5B)


class TestFrame(unittest.TestCase):
    def test_known_answer_frame(self):
        # Status request, seq 1, empty payload — same vector as helic-proto.
        frame = protocol.encode_frame(MsgType.STATUS, 1)
        self.assertEqual(frame, bytes([0x48, 0x4C, 0x0A, 0x01, 0x00, 0x00, 0x5B, 0xDB]))

    def test_known_answer_discovery_requests(self):
        self.assertEqual(
            protocol.encode_frame(MsgType.GET_PARAMS, 1),
            bytes.fromhex("48 4C 01 01 00 00 44 C5"),
        )
        self.assertEqual(
            protocol.encode_frame(MsgType.GET_SOURCES, 1),
            bytes.fromhex("48 4C 02 01 00 00 98 5E"),
        )

    def test_known_answer_v2_frames(self):
        block = protocol.encode_set_block(12, 0x01020304, b"\xaa\xbb")
        self.assertEqual(
            protocol.encode_frame(MsgType.SET_BLOCK, 2, block),
            bytes.fromhex("48 4C 05 02 08 00 0C 00 04 03 02 01 AA BB 39 A7"),
        )
        commit = protocol.encode_commit(12, 0x01020304)
        self.assertEqual(
            protocol.encode_frame(MsgType.COMMIT, 3, commit),
            bytes.fromhex("48 4C 06 03 06 00 0C 00 04 03 02 01 08 D1"),
        )
        status = bytes.fromhex("02 11 00 0D 00 00 FA 45 10 A4 00 00")
        self.assertEqual(
            protocol.encode_frame(MsgType.STATUS, 1, status),
            bytes.fromhex(
                "48 4C 0A 01 0C 00 02 11 00 0D 00 00 FA 45 10 A4 00 00 03 09"
            ),
        )

    def test_round_trip(self):
        frame = protocol.encode_frame(MsgType.GET_PAR, 7, b"\x01\x00\x02\x00")
        msg_type, seq, payload = protocol.decode_frame(frame)
        self.assertEqual(msg_type, MsgType.GET_PAR)
        self.assertEqual(seq, 7)
        self.assertEqual(payload, b"\x01\x00\x02\x00")

    def test_corrupt_crc_rejected(self):
        frame = bytearray(protocol.encode_frame(3, 0, b"\x2a"))
        frame[protocol.HEADER_LEN] ^= 0xFF
        with self.assertRaises(ProtocolError):
            protocol.decode_frame(bytes(frame))

    def test_bad_magic_rejected(self):
        frame = bytearray(protocol.encode_frame(3, 0))
        frame[0] = 0
        with self.assertRaises(ProtocolError):
            protocol.decode_frame(bytes(frame))

    def test_oversize_payload_rejected(self):
        with self.assertRaises(ProtocolError):
            protocol.encode_frame(3, 0, bytes(protocol.MAX_PAYLOAD + 1))


class TestPayload(unittest.TestCase):
    def test_known_answer_discovery_entries(self):
        params = protocol.decode_params(b"freq\0f\x01\x00\x01")
        self.assertEqual(params, [("freq", "f", 1, True)])
        sources = protocol.decode_sources(b"adc0\0V\0laser\0mm\0")
        self.assertEqual(sources, [("adc0", "V"), ("laser", "mm")])

    def test_known_answer_block_payloads(self):
        self.assertEqual(
            protocol.encode_set_block(12, 0x01020304, b"\xaa\xbb"),
            b"\x0c\x00\x04\x03\x02\x01\xaa\xbb",
        )
        self.assertEqual(
            protocol.encode_commit(12, 0x01020304),
            b"\x0c\x00\x04\x03\x02\x01",
        )

    def test_malformed_discovery_rejected(self):
        with self.assertRaises(ProtocolError):
            protocol.decode_params(b"freq\0f")
        with self.assertRaises(ProtocolError):
            protocol.decode_sources(b"adc0\0V")


class TestBeacon(unittest.TestCase):
    def test_known_request_and_response_round_trip(self):
        self.assertEqual(protocol.BEACON_REQUEST, bytes.fromhex("48 4c 01"))
        beacon = protocol.BeaconResponse(
            2, 2350, bytes.fromhex("02 48 4c 00 00 01"), "cbc-rig", "helic-daq sim"
        )
        encoded = protocol.encode_beacon_response(beacon)
        self.assertEqual(
            encoded,
            bytes.fromhex(
                "48 4c 02 02 2e 09 02 48 4c 00 00 01 "
                "63 62 63 2d 72 69 67 00 00 00 00 00 00 00 00 00 "
                "68 65 6c 69 63 2d 64 61 71 20 73 69 6d 00 00 00"
            ),
        )
        self.assertEqual(protocol.decode_beacon_response(encoded), beacon)

    def test_malformed_response_is_rejected(self):
        with self.assertRaises(ProtocolError):
            protocol.decode_beacon_response(b"")


class TestStreamHeader(unittest.TestCase):
    def test_round_trip(self):
        h = StreamHeader(
            n_sources=12, seq=123456, first_index=42, dropped=3, decimation=2, n_records=28
        )
        buf = protocol.encode_stream_header(h)
        self.assertEqual(len(buf), protocol.STREAM_HEADER_LEN)
        self.assertEqual(protocol.decode_stream_header(buf), h)

    def test_bad_magic_rejected(self):
        h = StreamHeader(1, 0, 0, 0, 1, 0)
        buf = bytearray(protocol.encode_stream_header(h))
        buf[0] = 0
        with self.assertRaises(ProtocolError):
            protocol.decode_stream_header(bytes(buf))


if __name__ == "__main__":
    unittest.main()
