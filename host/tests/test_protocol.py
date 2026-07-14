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
