//! Codecs for variable-length protocol-v2 control payloads.

use crate::ParamType;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayloadError {
    TooLong,
    Truncated,
}

pub fn encode_param(
    out: &mut [u8],
    name: &str,
    ty: ParamType,
    count: u16,
    writable: bool,
) -> Result<usize, PayloadError> {
    let n = name.len() + 5;
    if out.len() < n {
        return Err(PayloadError::TooLong);
    }
    out[..name.len()].copy_from_slice(name.as_bytes());
    out[name.len()] = 0;
    out[name.len() + 1] = ty as u8;
    out[name.len() + 2..name.len() + 4].copy_from_slice(&count.to_le_bytes());
    out[name.len() + 4] = writable as u8;
    Ok(n)
}

pub fn encode_source(out: &mut [u8], name: &str, unit: &str) -> Result<usize, PayloadError> {
    let n = name.len() + unit.len() + 2;
    if out.len() < n {
        return Err(PayloadError::TooLong);
    }
    out[..name.len()].copy_from_slice(name.as_bytes());
    out[name.len()] = 0;
    let unit_start = name.len() + 1;
    out[unit_start..unit_start + unit.len()].copy_from_slice(unit.as_bytes());
    out[n - 1] = 0;
    Ok(n)
}

pub fn decode_set_block(payload: &[u8]) -> Result<(u16, u32, &[u8]), PayloadError> {
    if payload.len() < 6 {
        return Err(PayloadError::Truncated);
    }
    Ok((
        u16::from_le_bytes([payload[0], payload[1]]),
        u32::from_le_bytes([payload[2], payload[3], payload[4], payload[5]]),
        &payload[6..],
    ))
}

pub fn decode_commit(payload: &[u8]) -> Result<(u16, u32), PayloadError> {
    if payload.len() != 6 {
        return Err(PayloadError::Truncated);
    }
    Ok((
        u16::from_le_bytes([payload[0], payload[1]]),
        u32::from_le_bytes([payload[2], payload[3], payload[4], payload[5]]),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_answer_discovery_entries() {
        let mut buf = [0u8; 32];
        let n = encode_param(&mut buf, "freq", ParamType::F32, 1, true).unwrap();
        assert_eq!(&buf[..n], b"freq\0f\x01\x00\x01");
        let n = encode_source(&mut buf, "adc0", "V").unwrap();
        assert_eq!(&buf[..n], b"adc0\0V\0");
    }

    #[test]
    fn known_answer_block_payloads() {
        let payload = [0x0c, 0x00, 0x04, 0x03, 0x02, 0x01, 0xaa, 0xbb];
        assert_eq!(
            decode_set_block(&payload),
            Ok((12, 0x0102_0304, &[0xaa, 0xbb][..]))
        );
        assert_eq!(decode_commit(&payload[..6]), Ok((12, 0x0102_0304)));
    }
}
