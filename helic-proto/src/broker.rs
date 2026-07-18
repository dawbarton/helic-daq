//! Host-broker protocol extension layered on the firmware wire protocol.
//!
//! Firmware deliberately does not include these values in [`crate::frame::MsgType`],
//! so a direct MCU connection reports them as unknown message types.

pub const EXTENSION_VERSION: u8 = 1;

pub const BROKER_INFO: u8 = 0x80;
pub const QUIET_STREAM_START: u8 = 0x81;
pub const GET_RECENT: u8 = 0x82;
pub const SET_CLIENT_QUIET: u8 = 0x83;

pub const CAP_QUIET_START: u16 = 1 << 0;
pub const CAP_RECENT: u16 = 1 << 1;
pub const CAP_SET_QUIET: u16 = 1 << 2;
pub const CAP_SHARED_CONFIG: u16 = 1 << 3;
pub const CAPABILITIES: u16 = CAP_QUIET_START | CAP_RECENT | CAP_SET_QUIET | CAP_SHARED_CONFIG;

pub const STATE_UPSTREAM_CONNECTED: u8 = 1 << 0;
pub const STATE_CONFIGURED: u8 = 1 << 1;
pub const STATE_RUNNING: u8 = 1 << 2;
pub const STATE_CLIENT_ATTACHED: u8 = 1 << 3;
pub const STATE_CLIENT_QUIET: u8 = 1 << 4;
pub const STATE_TABLE_TRANSACTION: u8 = 1 << 5;

pub const INFO_HEADER_LEN: usize = 21;
pub const MAX_SOURCES: usize = 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum BrokerError {
    NotAttached = 8,
    NotQuiet = 9,
    NoActiveStream = 10,
    InsufficientHistory = 11,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrokerPayloadError {
    BadLength,
    BadValue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BrokerInfo {
    pub state: u8,
    pub capabilities: u16,
    pub history_capacity_ms: u32,
    pub history_available_records: u32,
    pub decimation: u16,
    pub count: u32,
    pub connected_clients: u16,
    pub n_sources: u8,
    pub sources: [u8; MAX_SOURCES],
}

impl BrokerInfo {
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, BrokerPayloadError> {
        let n_sources = self.n_sources as usize;
        if n_sources > MAX_SOURCES || out.len() < INFO_HEADER_LEN + n_sources {
            return Err(BrokerPayloadError::BadLength);
        }
        out[0] = EXTENSION_VERSION;
        out[1] = self.state;
        out[2..4].copy_from_slice(&self.capabilities.to_le_bytes());
        out[4..8].copy_from_slice(&self.history_capacity_ms.to_le_bytes());
        out[8..12].copy_from_slice(&self.history_available_records.to_le_bytes());
        out[12..14].copy_from_slice(&self.decimation.to_le_bytes());
        out[14..18].copy_from_slice(&self.count.to_le_bytes());
        out[18..20].copy_from_slice(&self.connected_clients.to_le_bytes());
        out[20] = self.n_sources;
        out[INFO_HEADER_LEN..INFO_HEADER_LEN + n_sources]
            .copy_from_slice(&self.sources[..n_sources]);
        Ok(INFO_HEADER_LEN + n_sources)
    }

    pub fn decode(payload: &[u8]) -> Result<Self, BrokerPayloadError> {
        if payload.len() < INFO_HEADER_LEN || payload[0] != EXTENSION_VERSION {
            return Err(BrokerPayloadError::BadLength);
        }
        let n_sources = payload[20] as usize;
        if n_sources > MAX_SOURCES || payload.len() != INFO_HEADER_LEN + n_sources {
            return Err(BrokerPayloadError::BadLength);
        }
        let mut sources = [0; MAX_SOURCES];
        sources[..n_sources].copy_from_slice(&payload[INFO_HEADER_LEN..]);
        Ok(Self {
            state: payload[1],
            capabilities: u16::from_le_bytes([payload[2], payload[3]]),
            history_capacity_ms: u32::from_le_bytes([
                payload[4], payload[5], payload[6], payload[7],
            ]),
            history_available_records: u32::from_le_bytes([
                payload[8],
                payload[9],
                payload[10],
                payload[11],
            ]),
            decimation: u16::from_le_bytes([payload[12], payload[13]]),
            count: u32::from_le_bytes([payload[14], payload[15], payload[16], payload[17]]),
            connected_clients: u16::from_le_bytes([payload[18], payload[19]]),
            n_sources: n_sources as u8,
            sources,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broker_info_round_trips() {
        let mut info = BrokerInfo {
            state: STATE_UPSTREAM_CONNECTED | STATE_CONFIGURED | STATE_RUNNING,
            capabilities: CAPABILITIES,
            history_capacity_ms: 10_000,
            history_available_records: 42,
            decimation: 4,
            count: 0,
            connected_clients: 2,
            n_sources: 3,
            sources: [0; MAX_SOURCES],
        };
        info.sources[..3].copy_from_slice(&[0, 8, 12]);
        let mut encoded = [0; INFO_HEADER_LEN + MAX_SOURCES];
        let n = info.encode(&mut encoded).unwrap();
        assert_eq!(n, INFO_HEADER_LEN + 3);
        assert_eq!(BrokerInfo::decode(&encoded[..n]), Ok(info));
    }

    #[test]
    fn broker_info_rejects_wrong_versions_and_lengths() {
        assert_eq!(BrokerInfo::decode(&[]), Err(BrokerPayloadError::BadLength));
        let mut payload = [0; INFO_HEADER_LEN];
        payload[0] = EXTENSION_VERSION + 1;
        assert_eq!(
            BrokerInfo::decode(&payload),
            Err(BrokerPayloadError::BadLength)
        );
        payload[0] = EXTENSION_VERSION;
        payload[20] = 1;
        assert_eq!(
            BrokerInfo::decode(&payload),
            Err(BrokerPayloadError::BadLength)
        );
    }
}
