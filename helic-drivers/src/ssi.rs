//! Pure SSI word decoding and revolutions scaling.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SsiError {
    InvalidFormat,
    InvalidWord,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SsiFormat {
    pub bits: u8,
    pub gray: bool,
}

impl SsiFormat {
    pub fn decode(&self, raw: u32) -> Result<u32, SsiError> {
        if !(1..=32).contains(&self.bits) {
            return Err(SsiError::InvalidFormat);
        }
        let mask = if self.bits == 32 {
            u32::MAX
        } else {
            (1u32 << self.bits) - 1
        };
        if raw & !mask != 0 {
            return Err(SsiError::InvalidWord);
        }
        if !self.gray {
            return Ok(raw);
        }
        let mut binary = raw;
        let mut shift = 1;
        while shift < 32 {
            binary ^= binary >> shift;
            shift <<= 1;
        }
        Ok(binary & mask)
    }
}

pub fn deinterleave_pair(raw: u32, bits: u8) -> Result<[u32; 2], SsiError> {
    if !(1..=16).contains(&bits) {
        return Err(SsiError::InvalidFormat);
    }
    let used = u32::from(bits) * 2;
    if used < 32 && raw >> used != 0 {
        return Err(SsiError::InvalidWord);
    }
    let mut words = [0; 2];
    for bit in 0..bits {
        let shift = u32::from(bit) * 2;
        words[0] |= ((raw >> shift) & 1) << bit;
        words[1] |= ((raw >> (shift + 1)) & 1) << bit;
    }
    Ok(words)
}

#[derive(Clone, Copy, Debug)]
pub struct SsiScale {
    pub counts_per_rev: u32,
}

impl SsiScale {
    pub fn position(&self, counts: u32) -> f32 {
        debug_assert!(self.counts_per_rev > 0);
        (counts % self.counts_per_rev) as f32 / self.counts_per_rev as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gray(binary: u32) -> u32 {
        binary ^ (binary >> 1)
    }

    #[test]
    fn known_gray_vectors_decode_at_12_and_13_bits() {
        for bits in [12, 13] {
            let format = SsiFormat { bits, gray: true };
            for binary in [1, 2, 3, 7, 42, 1234, (1 << bits) - 2] {
                assert_eq!(format.decode(gray(binary)), Ok(binary), "{bits} bits");
            }
        }
    }

    #[test]
    fn binary_words_pass_through() {
        let format = SsiFormat {
            bits: 13,
            gray: false,
        };
        assert_eq!(format.decode(0x1234), Ok(0x1234));
    }

    #[test]
    fn binary_endpoints_are_valid_positions() {
        let format = SsiFormat {
            bits: 12,
            gray: false,
        };
        assert_eq!(format.decode(0), Ok(0));
        assert_eq!(format.decode(0x0fff), Ok(0x0fff));
        assert_eq!(format.decode(0x1000), Err(SsiError::InvalidWord));
    }

    #[test]
    fn interleaved_pair_round_trips_all_12_bit_positions() {
        for first in 0..4096u32 {
            let second = 4095 - first;
            let mut raw = 0;
            for bit in (0..12).rev() {
                raw = (raw << 2) | (((second >> bit) & 1) << 1) | ((first >> bit) & 1);
            }
            assert_eq!(deinterleave_pair(raw, 12), Ok([first, second]));
        }
    }

    #[test]
    fn interleaved_pair_rejects_invalid_shapes() {
        assert_eq!(deinterleave_pair(0, 0), Err(SsiError::InvalidFormat));
        assert_eq!(deinterleave_pair(0, 17), Err(SsiError::InvalidFormat));
        assert_eq!(deinterleave_pair(1 << 24, 12), Err(SsiError::InvalidWord));
    }

    #[test]
    fn invalid_bit_counts_are_rejected() {
        assert_eq!(
            SsiFormat {
                bits: 0,
                gray: false
            }
            .decode(1),
            Err(SsiError::InvalidFormat)
        );
    }

    #[test]
    fn scaling_wraps_in_revolutions() {
        let scale = SsiScale {
            counts_per_rev: 4096,
        };
        assert_eq!(scale.position(1024), 0.25);
        assert_eq!(scale.position(4096), 0.0);
        assert_eq!(scale.position(5120), 0.25);
    }
}
