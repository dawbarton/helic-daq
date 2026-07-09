//! UDP stream packet layout.
//!
//! ```text
//! offset  size  field
//! 0       2     magic       = 0xCBCD (LE)
//! 2       1     version     = 1
//! 3       1     n_sources   values per record
//! 4       4     seq         packet counter (LE)
//! 8       4     first_index sample index of the first record (LE, wraps)
//! 12      4     dropped     cumulative records dropped at source (since boot)
//! 16      2     decimation  records are every decimation-th sample
//! 18      2     n_records
//! 20      -     payload: n_records × n_sources × f32 (LE)
//! ```

use crate::{MAGIC, VERSION};

pub const STREAM_HEADER_LEN: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamHeader {
    pub n_sources: u8,
    pub seq: u32,
    pub first_index: u32,
    pub dropped: u32,
    pub decimation: u16,
    pub n_records: u16,
}

impl StreamHeader {
    /// Write the header into the first `STREAM_HEADER_LEN` bytes of `buf`.
    pub fn encode(&self, buf: &mut [u8]) {
        buf[0..2].copy_from_slice(&MAGIC.to_le_bytes());
        buf[2] = VERSION;
        buf[3] = self.n_sources;
        buf[4..8].copy_from_slice(&self.seq.to_le_bytes());
        buf[8..12].copy_from_slice(&self.first_index.to_le_bytes());
        buf[12..16].copy_from_slice(&self.dropped.to_le_bytes());
        buf[16..18].copy_from_slice(&self.decimation.to_le_bytes());
        buf[18..20].copy_from_slice(&self.n_records.to_le_bytes());
    }

    pub fn decode(buf: &[u8]) -> Option<Self> {
        if buf.len() < STREAM_HEADER_LEN || buf[0..2] != MAGIC.to_le_bytes() || buf[2] != VERSION {
            return None;
        }
        Some(Self {
            n_sources: buf[3],
            seq: u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]),
            first_index: u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]),
            dropped: u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]),
            decimation: u16::from_le_bytes([buf[16], buf[17]]),
            n_records: u16::from_le_bytes([buf[18], buf[19]]),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_round_trips() {
        let h = StreamHeader {
            n_sources: 12,
            seq: 123_456,
            first_index: 42,
            dropped: 3,
            decimation: 2,
            n_records: 28,
        };
        let mut buf = [0u8; STREAM_HEADER_LEN];
        h.encode(&mut buf);
        assert_eq!(StreamHeader::decode(&buf), Some(h));
    }

    #[test]
    fn bad_magic_or_version_rejected() {
        let mut buf = [0u8; STREAM_HEADER_LEN];
        StreamHeader {
            n_sources: 1,
            seq: 0,
            first_index: 0,
            dropped: 0,
            decimation: 1,
            n_records: 0,
        }
        .encode(&mut buf);
        let mut bad = buf;
        bad[0] = 0;
        assert_eq!(StreamHeader::decode(&bad), None);
        let mut bad = buf;
        bad[2] = 99;
        assert_eq!(StreamHeader::decode(&bad), None);
    }
}
