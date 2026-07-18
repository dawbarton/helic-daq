//! Fixed-size UDP discovery request and response.

use crate::{CONTROL_PORT, MAGIC, VERSION};

pub const REQUEST: [u8; 3] = [MAGIC as u8, (MAGIC >> 8) as u8, 0x01];
pub const RESPONSE_LEN: usize = 44;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BeaconError {
    BadLength,
    BadMagic,
    BadKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BeaconResponse {
    pub version: u8,
    pub control_port: u16,
    pub mac: [u8; 6],
    pub experiment: [u8; 16],
    pub firmware: [u8; 16],
}

impl BeaconResponse {
    pub fn new(mac: [u8; 6], experiment: &str, firmware: &str) -> Self {
        Self {
            version: VERSION,
            control_port: CONTROL_PORT,
            mac,
            experiment: fixed_string(experiment),
            firmware: fixed_string(firmware),
        }
    }

    pub fn encode(&self, out: &mut [u8; RESPONSE_LEN]) {
        out[0..2].copy_from_slice(&MAGIC.to_le_bytes());
        out[2] = 0x02;
        out[3] = self.version;
        out[4..6].copy_from_slice(&self.control_port.to_le_bytes());
        out[6..12].copy_from_slice(&self.mac);
        out[12..28].copy_from_slice(&self.experiment);
        out[28..44].copy_from_slice(&self.firmware);
    }

    pub fn decode(input: &[u8]) -> Result<Self, BeaconError> {
        if input.len() != RESPONSE_LEN {
            return Err(BeaconError::BadLength);
        }
        if u16::from_le_bytes([input[0], input[1]]) != MAGIC {
            return Err(BeaconError::BadMagic);
        }
        if input[2] != 0x02 {
            return Err(BeaconError::BadKind);
        }
        let mut mac = [0; 6];
        mac.copy_from_slice(&input[6..12]);
        let mut experiment = [0; 16];
        experiment.copy_from_slice(&input[12..28]);
        let mut firmware = [0; 16];
        firmware.copy_from_slice(&input[28..44]);
        Ok(Self {
            version: input[3],
            control_port: u16::from_le_bytes([input[4], input[5]]),
            mac,
            experiment,
            firmware,
        })
    }
}

fn fixed_string(value: &str) -> [u8; 16] {
    let mut out = [0; 16];
    let bytes = value.as_bytes();
    let len = bytes.len().min(out.len());
    out[..len].copy_from_slice(&bytes[..len]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_is_known_answer() {
        assert_eq!(REQUEST, [0x48, 0x4c, 0x01]);
    }

    #[test]
    fn response_round_trips_and_truncates_strings() {
        let response = BeaconResponse::new(
            [0x02, 0x48, 0x4c, 0, 0, 1],
            "whirl-rig",
            "helic-daq 0.1.0 abc123",
        );
        let mut encoded = [0; RESPONSE_LEN];
        response.encode(&mut encoded);
        assert_eq!(BeaconResponse::decode(&encoded), Ok(response));
        assert_eq!(&encoded[..6], &[0x48, 0x4c, 0x02, 0x03, 0x2e, 0x09]);
        assert_eq!(&response.firmware, b"helic-daq 0.1.0 ");
    }

    #[test]
    fn response_matches_shared_known_answer() {
        let response = BeaconResponse::new([0x02, 0x48, 0x4c, 0, 0, 1], "cbc-rig", "helic-daq sim");
        let mut encoded = [0; RESPONSE_LEN];
        response.encode(&mut encoded);
        assert_eq!(
            encoded,
            [
                0x48, 0x4c, 0x02, 0x03, 0x2e, 0x09, 0x02, 0x48, 0x4c, 0x00, 0x00, 0x01, 0x63, 0x62,
                0x63, 0x2d, 0x72, 0x69, 0x67, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x68, 0x65, 0x6c, 0x69, 0x63, 0x2d, 0x64, 0x61, 0x71, 0x20, 0x73, 0x69, 0x6d, 0x00,
                0x00, 0x00,
            ]
        );
    }

    #[test]
    fn malformed_responses_are_rejected() {
        assert_eq!(BeaconResponse::decode(&[]), Err(BeaconError::BadLength));
        let mut encoded = [0; RESPONSE_LEN];
        BeaconResponse::new([0; 6], "cbc-rig", "test").encode(&mut encoded);
        encoded[0] = 0;
        assert_eq!(BeaconResponse::decode(&encoded), Err(BeaconError::BadMagic));
    }
}
