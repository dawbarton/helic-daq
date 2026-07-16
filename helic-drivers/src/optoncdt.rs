//! Micro-Epsilon optoNCDT 1420 measurement stream parser.
//!
//! RS422 (via TTL converter), factory default 921.6 kBaud 8N1. Measurement
//! values are transmitted in binary as three bytes distinguished by their two
//! most significant flag bits, in the order L, M, H (manual §8.2–8.4):
//!
//! ```text
//! L-Byte: 0 0 D5  D4  D3  D2  D1  D0
//! M-Byte: 0 1 D11 D10 D9  D8  D7  D6
//! H-Byte: 1 0 D17 D16 D15 D14 D13 D12   (first output value)
//! H-Byte: 1 1 D17 D16 D15 D14 D13 D12   (output values 2..32)
//! ```
//!
//! The distance value (without mastering) is 16 bits in D15..D0: the span
//! [643, 64887] maps linearly onto [0, MR] with 1% reserves at each end:
//! `d = (x·102/(100·65520) − 0.01)·MR`.

/// One decoded output value from the stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawValue {
    /// The 18-bit payload (D17..D0).
    pub value: u32,
    /// True if this was the first output value of a measurement block
    /// (H-byte flags `10`); false for values 2..32 (flags `11`). With the
    /// factory data selection, the first value is the distance.
    pub first: bool,
}

/// Outcome of feeding one byte to the binary measurement parser.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseEvent {
    /// The byte advanced a partial value without completing it.
    Pending,
    /// One complete output value was decoded.
    Value(RawValue),
    /// Flag bits did not match the expected L/M/H sequence.
    Resynchronised,
}

/// Result of consuming one complete ASCII command reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandReply {
    /// The sensor returned its prompt without an error.
    Ok,
    /// The reply contained an `Exxx` sensor error code before the prompt.
    Error(u16),
}

/// Parser for ASCII command replies multiplexed with binary measurements.
///
/// The sensor terminates every reply with the `->` prompt. Binary output may
/// continue during command exchange, but a well-formed binary stream cannot
/// contain this two-byte sequence because every third byte has its top bit
/// set. The parser also remembers an `Exxx` error until the prompt arrives.
#[derive(Debug, Default)]
pub struct CommandReplyParser {
    previous: u8,
    window: u32,
    error: Option<u16>,
}

impl CommandReplyParser {
    pub const fn new() -> Self {
        Self {
            previous: 0,
            window: 0,
            error: None,
        }
    }

    /// Feed one received byte, returning when the terminating prompt arrives.
    pub fn push(&mut self, byte: u8) -> Option<CommandReply> {
        self.window = (self.window << 8) | byte as u32;
        let bytes = self.window.to_be_bytes();
        if bytes[0] == b'E'
            && bytes[1].is_ascii_digit()
            && bytes[2].is_ascii_digit()
            && bytes[3].is_ascii_digit()
        {
            self.error = Some(
                u16::from(bytes[1] - b'0') * 100
                    + u16::from(bytes[2] - b'0') * 10
                    + u16::from(bytes[3] - b'0'),
            );
        }

        let complete = self.previous == b'-' && byte == b'>';
        self.previous = byte;
        if complete {
            Some(match self.error {
                Some(code) => CommandReply::Error(code),
                None => CommandReply::Ok,
            })
        } else {
            None
        }
    }
}

/// Classification of a 16-bit distance value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Reading {
    /// Within the measuring range; distance in mm.
    InRange(f32),
    /// Below the start-of-range reserve band.
    BelowRange,
    /// In the end-of-range reserve band.
    AboveRange,
    /// Error code (no target, too dark, …).
    Error(u16),
}

/// Push parser: feed received bytes one at a time; a completed value is
/// returned on its H-byte. Resynchronises on flag-bit violations, so it
/// recovers from a dropped byte within one measurement.
#[derive(Debug, Default)]
pub struct Parser {
    /// Accumulated D11..D0 and fill state: 0 = expect L, 1 = expect M,
    /// 2 = expect H.
    acc: u32,
    stage: u8,
}

impl Parser {
    pub const fn new() -> Self {
        Self { acc: 0, stage: 0 }
    }

    /// Feed one byte; returns a value when its final (H) byte arrives.
    #[inline]
    pub fn push(&mut self, byte: u8) -> Option<RawValue> {
        match self.push_event(byte) {
            ParseEvent::Value(value) => Some(value),
            ParseEvent::Pending | ParseEvent::Resynchronised => None,
        }
    }

    /// Feed one byte and report malformed sequences as resynchronisations.
    ///
    /// An L-byte received mid-value both reports the truncated value and
    /// starts the next candidate value, so recovery does not discard a good
    /// byte.
    #[inline]
    pub fn push_event(&mut self, byte: u8) -> ParseEvent {
        let flags = byte >> 6;
        let data = (byte & 0x3F) as u32;
        match (self.stage, flags) {
            (0, 0b00) => {
                self.acc = data;
                self.stage = 1;
                ParseEvent::Pending
            }
            (1, 0b01) => {
                self.acc |= data << 6;
                self.stage = 2;
                ParseEvent::Pending
            }
            (2, 0b10) | (2, 0b11) => {
                let value = self.acc | (data << 12);
                self.stage = 0;
                ParseEvent::Value(RawValue {
                    value,
                    first: flags == 0b10,
                })
            }
            // Out-of-sequence byte: an L-byte restarts a value, anything
            // else drops us back to hunting for an L-byte.
            (_, 0b00) => {
                self.acc = data;
                self.stage = 1;
                ParseEvent::Resynchronised
            }
            _ => {
                self.stage = 0;
                ParseEvent::Resynchronised
            }
        }
    }
}

/// Distance scaling for a sensor with the given measuring range (e.g.
/// 10/25/50/100/200/500 mm), without mastering.
#[derive(Clone, Copy, Debug)]
pub struct DistanceScale {
    pub measuring_range_mm: f32,
}

impl DistanceScale {
    /// Start of the measuring-range span in raw counts.
    pub const SPAN_LO: u16 = 643;
    /// End of the measuring-range span in raw counts.
    pub const SPAN_HI: u16 = 64887;
    /// Values above this are error codes rather than reserve-band readings.
    pub const ERROR_FLOOR: u16 = 65520;

    pub const fn new(measuring_range_mm: f32) -> Self {
        Self { measuring_range_mm }
    }

    /// Convert a distance raw value (low 16 bits of the 18-bit payload).
    pub fn convert(&self, raw: u32) -> Reading {
        let x = (raw & 0xFFFF) as u16;
        if x > Self::ERROR_FLOOR {
            Reading::Error(x)
        } else if x < Self::SPAN_LO {
            Reading::BelowRange
        } else if x > Self::SPAN_HI {
            Reading::AboveRange
        } else {
            Reading::InRange(self.mm(x))
        }
    }

    /// The manual's conversion formula, valid over the full coded span.
    #[inline]
    pub fn mm(&self, x: u16) -> f32 {
        (x as f32 * (102.0 / (100.0 * 65520.0)) - 0.01) * self.measuring_range_mm
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode an 18-bit value as its three stream bytes.
    fn encode(value: u32, first: bool) -> [u8; 3] {
        let h_flags = if first { 0b10 } else { 0b11 };
        [
            (value & 0x3F) as u8,
            0b0100_0000 | ((value >> 6) & 0x3F) as u8,
            (h_flags << 6) | ((value >> 12) & 0x3F) as u8,
        ]
    }

    #[test]
    fn round_trips_encoded_values() {
        let mut p = Parser::new();
        for value in [0u32, 643, 32768, 64887, 0x3FFFF] {
            let bytes = encode(value, true);
            assert_eq!(p.push(bytes[0]), None);
            assert_eq!(p.push(bytes[1]), None);
            assert_eq!(p.push(bytes[2]), Some(RawValue { value, first: true }));
        }
    }

    #[test]
    fn distinguishes_first_from_subsequent_values() {
        let mut p = Parser::new();
        let mut out = Vec::new();
        for byte in encode(100, true).into_iter().chain(encode(200, false)) {
            if let Some(v) = p.push(byte) {
                out.push(v);
            }
        }
        assert_eq!(
            out,
            vec![
                RawValue {
                    value: 100,
                    first: true
                },
                RawValue {
                    value: 200,
                    first: false
                }
            ]
        );
    }

    #[test]
    fn resynchronises_after_dropped_byte() {
        let mut p = Parser::new();
        let good = encode(12345, true);
        // Drop the L-byte of one measurement: its M and H bytes must be
        // discarded, and the next full measurement parsed correctly.
        p.push(good[1]);
        p.push(good[2]);
        assert_eq!(p.push_event(good[0]), ParseEvent::Pending);
        assert_eq!(p.push(good[1]), None);
        assert_eq!(
            p.push(good[2]),
            Some(RawValue {
                value: 12345,
                first: true
            })
        );
    }

    #[test]
    fn reports_out_of_sequence_bytes_while_recovering() {
        let mut p = Parser::new();
        assert_eq!(p.push_event(0b0100_0001), ParseEvent::Resynchronised);
        assert_eq!(p.push_event(0b1000_0010), ParseEvent::Resynchronised);

        let good = encode(1234, true);
        assert_eq!(p.push_event(good[0]), ParseEvent::Pending);
        assert_eq!(p.push_event(good[1]), ParseEvent::Pending);
        assert_eq!(
            p.push_event(good[2]),
            ParseEvent::Value(RawValue {
                value: 1234,
                first: true
            })
        );
    }

    #[test]
    fn l_byte_mid_value_restarts_parsing() {
        let mut p = Parser::new();
        let good = encode(999, true);
        p.push(good[0]);
        // A fresh L-byte arrives instead of the M-byte (previous value was
        // truncated): parsing restarts from the new L-byte.
        assert_eq!(p.push_event(good[0]), ParseEvent::Resynchronised);
        assert_eq!(p.push(good[1]), None);
        assert_eq!(
            p.push(good[2]),
            Some(RawValue {
                value: 999,
                first: true
            })
        );
    }

    #[test]
    fn distance_scaling_spans_measuring_range() {
        // ILD1420-50: 50 mm measuring range.
        let scale = DistanceScale::new(50.0);
        // Span endpoints map to ~0 and ~MR (1% reserve bands outside).
        match scale.convert(643) {
            Reading::InRange(mm) => assert!(mm.abs() < 0.01, "{mm}"),
            other => panic!("{other:?}"),
        }
        match scale.convert(64887) {
            Reading::InRange(mm) => assert!((mm - 50.0).abs() < 0.01, "{mm}"),
            other => panic!("{other:?}"),
        }
        // Mid-span is mid-range.
        match scale.convert((643 + 64887) / 2) {
            Reading::InRange(mm) => assert!((mm - 25.0).abs() < 0.01, "{mm}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn reserve_bands_and_errors_are_classified() {
        let scale = DistanceScale::new(50.0);
        assert_eq!(scale.convert(0), Reading::BelowRange);
        assert_eq!(scale.convert(642), Reading::BelowRange);
        assert_eq!(scale.convert(64888), Reading::AboveRange);
        assert_eq!(scale.convert(65520), Reading::AboveRange);
        assert_eq!(scale.convert(65535), Reading::Error(65535));
    }

    #[test]
    fn command_reply_waits_for_prompt_and_reports_errors() {
        let mut ok = CommandReplyParser::new();
        let mut result = None;
        for byte in b"MEASRATE 8 ok\r\n->" {
            result = ok.push(*byte).or(result);
        }
        assert_eq!(result, Some(CommandReply::Ok));

        let mut error = CommandReplyParser::new();
        let mut result = None;
        for byte in b"E202 Access denied\r\n->" {
            result = error.push(*byte).or(result);
        }
        assert_eq!(result, Some(CommandReply::Error(202)));
    }

    #[test]
    fn command_reply_ignores_interleaved_binary_measurements() {
        let mut reply = CommandReplyParser::new();
        let mut result = None;
        for byte in encode(12345, true)
            .into_iter()
            .chain(*b"OUTPUT NONE\r\n")
            .chain(encode(23456, true))
            .chain(*b"->")
        {
            result = reply.push(byte).or(result);
        }
        assert_eq!(result, Some(CommandReply::Ok));
    }

    #[test]
    fn measurement_parser_ignores_ascii_replies_and_prompts() {
        let mut parser = Parser::new();
        let mut values = Vec::new();
        for byte in b"MEASRATE 8 ok\r\n->"
            .iter()
            .copied()
            .chain(encode(32768, true))
        {
            if let Some(value) = parser.push(byte) {
                values.push(value);
            }
        }
        assert_eq!(
            values,
            vec![RawValue {
                value: 32768,
                first: true
            }]
        );
    }
}
