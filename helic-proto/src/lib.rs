//! Wire protocol shared between the HELIC-DAQ firmware and host.
//!
//! Fixed-layout little-endian binary, hand-parseable with Python `struct`.
//! The authoritative description lives in `docs/protocol.md`; the Python
//! package mirrors these definitions and both are tested against the known
//! answer vectors in that document.

#![cfg_attr(not(test), no_std)]

pub mod beacon;
pub mod crc;
pub mod frame;
pub mod payload;
pub mod stream;

pub use beacon::{BeaconError, BeaconResponse};
pub use crc::crc16;
pub use frame::{decode, encode, FrameError, MsgType, HEADER_LEN, MAX_PAYLOAD, TRAILER_LEN};
pub use stream::StreamHeader;

/// Magic prefix on every control frame and stream packet.
/// Little-endian ASCII `HL`, identifying the HELIC-DAQ protocol.
pub const MAGIC: u16 = 0x4C48;

/// Protocol version, bumped on any incompatible wire change.
pub const VERSION: u8 = 2;

/// TCP port for parameter get/set and commands.
pub const CONTROL_PORT: u16 = 2350;

/// Default UDP port for sample streaming (the host requests a port in
/// `StreamStart`; this is the conventional choice).
pub const STREAM_PORT: u16 = 2351;

/// UDP port for device-discovery beacons.
pub const DISCOVERY_PORT: u16 = 2352;

/// Error codes carried in `MsgType::Error` responses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ErrorCode {
    BadFrame = 1,
    UnknownType = 2,
    BadIndex = 3,
    BadLength = 4,
    ReadOnly = 5,
    BadValue = 6,
    Busy = 7,
}

/// Type of a registered parameter, self-described to the host at connect.
///
/// Discriminants are Python `struct` format characters (as in the previous
/// rtc implementation), so the host can build unpackers directly from the
/// discovery response.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ParamType {
    U8 = b'B',
    I8 = b'b',
    U16 = b'H',
    I16 = b'h',
    U32 = b'I',
    I32 = b'i',
    F32 = b'f',
    Char = b'c',
}

impl ParamType {
    /// Size in bytes of one element of this type.
    pub const fn size(self) -> usize {
        match self {
            Self::U8 | Self::I8 | Self::Char => 1,
            Self::U16 | Self::I16 => 2,
            Self::U32 | Self::I32 | Self::F32 => 4,
        }
    }

    pub const fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            b'B' => Self::U8,
            b'b' => Self::I8,
            b'H' => Self::U16,
            b'h' => Self::I16,
            b'I' => Self::U32,
            b'i' => Self::I32,
            b'f' => Self::F32,
            b'c' => Self::Char,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_codes_round_trip() {
        for t in [
            ParamType::U8,
            ParamType::I8,
            ParamType::U16,
            ParamType::I16,
            ParamType::U32,
            ParamType::I32,
            ParamType::F32,
            ParamType::Char,
        ] {
            assert_eq!(ParamType::from_code(t as u8), Some(t));
        }
        assert_eq!(ParamType::from_code(b'x'), None);
    }
}
