//! Codecs for variable-length protocol-v3 control payloads.

use crate::ParamType;

pub const MAX_PARAM_NAME_LEN: usize = 23;
pub const MAX_NAME_LEN: usize = 15;
pub const MAX_UNIT_LEN: usize = 7;
/// Bytes preceding the definitions in a paged `GetParams` response.
pub const PARAM_PAGE_HEADER_LEN: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayloadError {
    InvalidText,
    InvalidIndex,
    InvalidLength,
    TooLong,
    Truncated,
}

/// Decode the first requested index from a `GetParams` request.
pub fn decode_param_page_request(payload: &[u8]) -> Result<u16, PayloadError> {
    if payload.len() != 2 {
        return Err(PayloadError::InvalidLength);
    }
    Ok(u16::from_le_bytes([payload[0], payload[1]]))
}

/// Encode one complete-definition page of a parameter registry.
///
/// The response begins with the echoed start and the exclusive next index.
/// `definition` must return every registry entry in `0..total`; the callback
/// keeps registry ownership outside the wire-codec crate without allocating.
pub fn encode_param_page<'a, F>(
    out: &mut [u8],
    start: u16,
    total: u16,
    mut definition: F,
) -> Result<usize, PayloadError>
where
    F: FnMut(u16) -> Option<(&'a str, ParamType, u16, bool)>,
{
    if start > total {
        return Err(PayloadError::InvalidIndex);
    }
    if out.len() < PARAM_PAGE_HEADER_LEN {
        return Err(PayloadError::TooLong);
    }

    out[..2].copy_from_slice(&start.to_le_bytes());
    let mut next = start;
    let mut offset = PARAM_PAGE_HEADER_LEN;
    while next < total {
        let (name, ty, count, writable) = definition(next).ok_or(PayloadError::InvalidIndex)?;
        if name.len() > MAX_PARAM_NAME_LEN || !name.is_ascii() {
            return Err(PayloadError::InvalidText);
        }
        let encoded_len = name.len() + 5;
        if out.len() - offset < encoded_len {
            break;
        }
        offset += encode_param(&mut out[offset..], name, ty, count, writable)?;
        next += 1;
    }
    if next == start && start < total {
        return Err(PayloadError::TooLong);
    }
    out[2..4].copy_from_slice(&next.to_le_bytes());
    Ok(offset)
}

/// Decode the indices and definition bytes from a `GetParams` response page.
pub fn decode_param_page(payload: &[u8]) -> Result<(u16, u16, &[u8]), PayloadError> {
    if payload.len() < PARAM_PAGE_HEADER_LEN {
        return Err(PayloadError::Truncated);
    }
    let start = u16::from_le_bytes([payload[0], payload[1]]);
    let next = u16::from_le_bytes([payload[2], payload[3]]);
    if next < start {
        return Err(PayloadError::InvalidIndex);
    }
    Ok((start, next, &payload[PARAM_PAGE_HEADER_LEN..]))
}

pub fn encode_param(
    out: &mut [u8],
    name: &str,
    ty: ParamType,
    count: u16,
    writable: bool,
) -> Result<usize, PayloadError> {
    if name.len() > MAX_PARAM_NAME_LEN || !name.is_ascii() {
        return Err(PayloadError::InvalidText);
    }
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
    if name.len() > MAX_NAME_LEN
        || unit.len() > MAX_UNIT_LEN
        || !name.is_ascii()
        || !unit.is_ascii()
    {
        return Err(PayloadError::InvalidText);
    }
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
    fn parameter_pages_preserve_complete_entries_and_indices() {
        let definitions = [
            ("a", ParamType::F32, 1, false),
            ("long_name", ParamType::U32, 1, true),
            ("tail", ParamType::U16, 4, false),
        ];
        let mut first = [0u8; 18];
        let n = encode_param_page(&mut first, 0, definitions.len() as u16, |index| {
            definitions.get(index as usize).copied()
        })
        .unwrap();
        let (start, next, entries) = decode_param_page(&first[..n]).unwrap();
        assert_eq!((start, next), (0, 1));
        assert_eq!(entries, b"a\0f\x01\x00\x00");

        let mut second = [0u8; 64];
        let n = encode_param_page(&mut second, next, definitions.len() as u16, |index| {
            definitions.get(index as usize).copied()
        })
        .unwrap();
        let (start, next, entries) = decode_param_page(&second[..n]).unwrap();
        assert_eq!((start, next), (1, 3));
        assert_eq!(entries, b"long_name\0I\x01\x00\x01tail\0H\x04\x00\x00");
    }

    #[test]
    fn parameter_page_validation_rejects_bad_ranges_and_no_progress() {
        let mut buf = [0u8; 8];
        assert_eq!(
            decode_param_page_request(&[]),
            Err(PayloadError::InvalidLength)
        );
        assert_eq!(
            encode_param_page(&mut buf, 2, 1, |_| None),
            Err(PayloadError::InvalidIndex)
        );
        assert_eq!(
            encode_param_page(&mut buf[..4], 0, 1, |_| {
                Some(("a", ParamType::F32, 1, false))
            }),
            Err(PayloadError::TooLong)
        );
        assert_eq!(
            decode_param_page(&[2, 0, 1, 0]),
            Err(PayloadError::InvalidIndex)
        );
    }

    #[test]
    fn discovery_text_limits_are_enforced() {
        let mut buf = [0u8; 32];
        assert!(encode_param(&mut buf, "laser_frames_received", ParamType::U32, 1, false).is_ok());
        assert_eq!(
            encode_param(
                &mut buf,
                "twenty_four_byte_name_xx",
                ParamType::F32,
                1,
                true
            ),
            Err(PayloadError::InvalidText)
        );
        assert_eq!(
            encode_source(&mut buf, "adc0", "abcdefgh"),
            Err(PayloadError::InvalidText)
        );
        assert_eq!(
            encode_source(&mut buf, "µm", "mm"),
            Err(PayloadError::InvalidText)
        );
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
