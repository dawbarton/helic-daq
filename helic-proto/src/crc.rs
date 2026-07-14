//! CRC-16/CCITT-FALSE: polynomial 0x1021, initial value 0xFFFF, no
//! reflection, no final XOR. Chosen because it is trivial to implement
//! identically in the host codecs (`helic_daq/protocol.py`,
//! `host-julia/src/protocol.jl`, and `host-matlab/+helicdaq/Protocol.m`).

pub fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &byte in data {
        crc ^= (byte as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_answer_vectors() {
        // Standard check value for CRC-16/CCITT-FALSE.
        assert_eq!(crc16(b"123456789"), 0x29B1);
        assert_eq!(crc16(b""), 0xFFFF);
        assert_eq!(crc16(&[0x00]), 0xE1F0);
        // Vector recorded in docs/protocol.md, mirrored in every host codec test.
        assert_eq!(crc16(&[0x0A, 0x01, 0x00, 0x00]), 0xDB5B);
    }
}
