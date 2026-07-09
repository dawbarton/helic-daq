//! Wire protocol shared between the CBC-DAQ firmware and host.
//!
//! Fixed-layout little-endian binary, hand-parseable with Python `struct`.
//! Full message definitions land with milestone 5 (`docs/protocol.md`); this
//! establishes the constants that both ends must agree on from day one.

#![cfg_attr(not(test), no_std)]

/// Magic prefix on every control frame and stream packet.
pub const MAGIC: u16 = 0xCBCD;

/// Protocol version, bumped on any incompatible wire change.
pub const VERSION: u8 = 1;

/// TCP port for parameter get/set and commands.
pub const CONTROL_PORT: u16 = 2350;

/// UDP port for sample streaming.
pub const STREAM_PORT: u16 = 2351;

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
