//! Control-channel (TCP) framing.
//!
//! ```text
//! offset  size  field
//! 0       2     magic   = 0x4C48 (LE ASCII `HL`)
//! 2       1     type    (MsgType)
//! 3       1     seq     (echoed in the response)
//! 4       2     len     payload length (LE)
//! 6       len   payload
//! 6+len   2     crc16   over bytes 2..6+len (type through payload), LE
//! ```

use crate::{crc16, MAGIC};

/// Bytes before the payload.
pub const HEADER_LEN: usize = 6;
/// Bytes after the payload.
pub const TRAILER_LEN: usize = 2;
/// Maximum payload length either side will accept.
pub const MAX_PAYLOAD: usize = 1024;
/// Largest complete frame.
pub const MAX_FRAME: usize = HEADER_LEN + MAX_PAYLOAD + TRAILER_LEN;

/// Control message types. Requests and their responses share the type; error
/// responses use `Error`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum MsgType {
    /// → (empty)  ← repeated name NUL, type u8, count u16, writable u8.
    GetParams = 1,
    /// → (empty)  ← repeated name NUL, unit NUL.
    GetSources = 2,
    /// → indices u16[]  ← raw values concatenated in request order.
    GetPar = 3,
    /// → index u16, raw value  ← (empty ack).
    SetPar = 4,
    /// → index u16, offset u32, raw block data  ← (empty ack).
    SetBlock = 5,
    /// → index u16, length u32  ← (empty ack).
    Commit = 6,
    /// → decimation u16, count u32, n u8, sources u8[n]  ← (empty ack).
    StreamSetup = 7,
    /// → host UDP port u16 (stream target = TCP peer IP)  ← (empty ack).
    StreamStart = 8,
    /// → (empty)  ← (empty ack).
    StreamStop = 9,
    /// → (empty)  ← version u8, n_params u16, n_sources u8,
    /// sample rate f32, uptime_ms u32.
    Status = 10,
    /// ← error code u8, offending request type u8.
    Error = 0xFF,
}

impl MsgType {
    pub const fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            1 => Self::GetParams,
            2 => Self::GetSources,
            3 => Self::GetPar,
            4 => Self::SetPar,
            5 => Self::SetBlock,
            6 => Self::Commit,
            7 => Self::StreamSetup,
            8 => Self::StreamStart,
            9 => Self::StreamStop,
            10 => Self::Status,
            0xFF => Self::Error,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameError {
    BadMagic,
    BadCrc,
    TooLong,
    Truncated,
}

/// Encode a frame into `buf`; returns the total frame length.
/// `buf` must hold at least `HEADER_LEN + payload.len() + TRAILER_LEN`.
pub fn encode(buf: &mut [u8], msg_type: u8, seq: u8, payload: &[u8]) -> Result<usize, FrameError> {
    let total = HEADER_LEN + payload.len() + TRAILER_LEN;
    if payload.len() > MAX_PAYLOAD {
        return Err(FrameError::TooLong);
    }
    if buf.len() < total {
        return Err(FrameError::Truncated);
    }
    buf[0..2].copy_from_slice(&MAGIC.to_le_bytes());
    buf[2] = msg_type;
    buf[3] = seq;
    buf[4..6].copy_from_slice(&(payload.len() as u16).to_le_bytes());
    buf[6..6 + payload.len()].copy_from_slice(payload);
    let crc = crc16(&buf[2..6 + payload.len()]);
    buf[6 + payload.len()..total].copy_from_slice(&crc.to_le_bytes());
    Ok(total)
}

/// Decode a complete frame from `buf` (exactly one frame, length already
/// established from the header). Returns `(type, seq, payload)`.
pub fn decode(buf: &[u8]) -> Result<(u8, u8, &[u8]), FrameError> {
    if buf.len() < HEADER_LEN + TRAILER_LEN {
        return Err(FrameError::Truncated);
    }
    if buf[0..2] != MAGIC.to_le_bytes() {
        return Err(FrameError::BadMagic);
    }
    let len = u16::from_le_bytes([buf[4], buf[5]]) as usize;
    if len > MAX_PAYLOAD {
        return Err(FrameError::TooLong);
    }
    if buf.len() != HEADER_LEN + len + TRAILER_LEN {
        return Err(FrameError::Truncated);
    }
    let crc_stored = u16::from_le_bytes([buf[HEADER_LEN + len], buf[HEADER_LEN + len + 1]]);
    if crc16(&buf[2..HEADER_LEN + len]) != crc_stored {
        return Err(FrameError::BadCrc);
    }
    Ok((buf[2], buf[3], &buf[HEADER_LEN..HEADER_LEN + len]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let mut buf = [0u8; MAX_FRAME];
        let n = encode(&mut buf, MsgType::GetPar as u8, 7, &[1, 0, 2, 0]).unwrap();
        let (ty, seq, payload) = decode(&buf[..n]).unwrap();
        assert_eq!(ty, MsgType::GetPar as u8);
        assert_eq!(seq, 7);
        assert_eq!(payload, &[1, 0, 2, 0]);
    }

    #[test]
    fn known_answer_frame() {
        // Status request, seq 1, empty payload — vector in docs/protocol.md.
        let mut buf = [0u8; 16];
        let n = encode(&mut buf, MsgType::Status as u8, 1, &[]).unwrap();
        assert_eq!(&buf[..n], &[0x48, 0x4C, 0x0A, 0x01, 0x00, 0x00, 0x5B, 0xDB]);
    }

    #[test]
    fn known_answer_discovery_requests() {
        let mut buf = [0u8; 16];
        let n = encode(&mut buf, MsgType::GetParams as u8, 1, &[]).unwrap();
        assert_eq!(&buf[..n], &[0x48, 0x4c, 0x01, 0x01, 0x00, 0x00, 0x44, 0xc5]);
        let n = encode(&mut buf, MsgType::GetSources as u8, 1, &[]).unwrap();
        assert_eq!(&buf[..n], &[0x48, 0x4c, 0x02, 0x01, 0x00, 0x00, 0x98, 0x5e]);
    }

    #[test]
    fn known_answer_v2_frames() {
        let mut buf = [0u8; 32];
        let block = [0x0c, 0x00, 0x04, 0x03, 0x02, 0x01, 0xaa, 0xbb];
        let n = encode(&mut buf, MsgType::SetBlock as u8, 2, &block).unwrap();
        assert_eq!(
            &buf[..n],
            &[
                0x48, 0x4c, 0x05, 0x02, 0x08, 0x00, 0x0c, 0x00, 0x04, 0x03, 0x02, 0x01, 0xaa, 0xbb,
                0x39, 0xa7
            ]
        );
        let n = encode(&mut buf, MsgType::Commit as u8, 3, &block[..6]).unwrap();
        assert_eq!(
            &buf[..n],
            &[0x48, 0x4c, 0x06, 0x03, 0x06, 0x00, 0x0c, 0x00, 0x04, 0x03, 0x02, 0x01, 0x08, 0xd1]
        );
        let status = [
            0x02, 0x11, 0x00, 0x0d, 0x00, 0x00, 0xfa, 0x45, 0x10, 0xa4, 0x00, 0x00,
        ];
        let n = encode(&mut buf, MsgType::Status as u8, 1, &status).unwrap();
        assert_eq!(
            &buf[..n],
            &[
                0x48, 0x4c, 0x0a, 0x01, 0x0c, 0x00, 0x02, 0x11, 0x00, 0x0d, 0x00, 0x00, 0xfa, 0x45,
                0x10, 0xa4, 0x00, 0x00, 0x03, 0x09
            ]
        );
    }

    #[test]
    fn corrupt_crc_is_rejected() {
        let mut buf = [0u8; MAX_FRAME];
        let n = encode(&mut buf, 3, 0, &[42]).unwrap();
        buf[HEADER_LEN] ^= 0xFF; // flip a payload bit
        assert_eq!(decode(&buf[..n]), Err(FrameError::BadCrc));
    }

    #[test]
    fn bad_magic_is_rejected() {
        let mut buf = [0u8; MAX_FRAME];
        let n = encode(&mut buf, 3, 0, &[]).unwrap();
        buf[0] = 0;
        assert_eq!(decode(&buf[..n]), Err(FrameError::BadMagic));
    }

    #[test]
    fn wrong_length_is_rejected() {
        let mut buf = [0u8; MAX_FRAME];
        let n = encode(&mut buf, 3, 0, &[1, 2, 3]).unwrap();
        assert_eq!(decode(&buf[..n - 1]), Err(FrameError::Truncated));
    }

    #[test]
    fn oversize_payload_is_rejected() {
        let mut buf = [0u8; 2 * MAX_FRAME];
        let payload = [0u8; MAX_PAYLOAD + 1];
        assert_eq!(encode(&mut buf, 3, 0, &payload), Err(FrameError::TooLong));
    }
}
