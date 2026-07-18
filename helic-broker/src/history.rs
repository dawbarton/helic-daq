//! Bounded packet history and exact-record historical replay.

use std::collections::VecDeque;
use std::sync::Arc;

use anyhow::{bail, Result};
use helic_proto::stream::{StreamHeader, STREAM_HEADER_LEN};

#[derive(Clone, Debug)]
pub struct Packet {
    pub wire: Arc<[u8]>,
    pub header: StreamHeader,
    pub received_utc_ns: i64,
}

impl Packet {
    pub fn decode(wire: Arc<[u8]>, received_utc_ns: i64) -> Result<Self> {
        let Some(header) = StreamHeader::decode(&wire) else {
            bail!("invalid HELIC-DAQ stream header");
        };
        if header.n_sources == 0 || header.n_records == 0 {
            bail!("stream packet has no sources or records");
        }
        let expected =
            STREAM_HEADER_LEN + 4 * header.n_sources as usize * header.n_records as usize;
        if wire.len() != expected {
            bail!("stream packet length {} != expected {expected}", wire.len());
        }
        Ok(Self {
            wire,
            header,
            received_utc_ns,
        })
    }
}

#[derive(Debug, Default)]
pub struct History {
    packets: VecDeque<Packet>,
    records: usize,
    capacity_records: usize,
}

impl History {
    pub fn reset(&mut self, capacity_records: usize) {
        self.packets.clear();
        self.records = 0;
        self.capacity_records = capacity_records.max(1);
    }

    pub fn clear(&mut self) {
        self.packets.clear();
        self.records = 0;
    }

    pub fn records(&self) -> usize {
        self.records
    }

    pub fn push(&mut self, packet: Packet) {
        self.records += packet.header.n_records as usize;
        self.packets.push_back(packet);
        while self.records > self.capacity_records && self.packets.len() > 1 {
            let removed = self.packets.pop_front().expect("length checked");
            self.records -= removed.header.n_records as usize;
        }
    }

    pub fn recent(&self, requested: usize) -> Option<Vec<Arc<[u8]>>> {
        if requested == 0 || requested > self.records {
            return None;
        }
        let mut selected = Vec::new();
        let mut records = 0usize;
        for packet in self.packets.iter().rev() {
            selected.push(packet);
            records += packet.header.n_records as usize;
            if records >= requested {
                break;
            }
        }
        selected.reverse();
        let excess = records - requested;
        let first = selected[0];
        let mut result = Vec::with_capacity(selected.len());
        if excess == 0 {
            result.push(first.wire.clone());
        } else {
            let record_bytes = 4 * first.header.n_sources as usize;
            let payload_offset = STREAM_HEADER_LEN + excess * record_bytes;
            let mut header = first.header;
            header.first_index = header
                .first_index
                .wrapping_add((excess as u32).wrapping_mul(header.decimation as u32));
            header.n_records -= excess as u16;
            let mut wire = vec![0; STREAM_HEADER_LEN];
            header.encode(&mut wire);
            wire.extend_from_slice(&first.wire[payload_offset..]);
            result.push(wire.into());
        }
        result.extend(selected[1..].iter().map(|packet| packet.wire.clone()));
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(seq: u32, first: u32, records: u16) -> Packet {
        let header = StreamHeader {
            n_sources: 2,
            seq,
            first_index: first,
            dropped: 0,
            decimation: 2,
            n_records: records,
        };
        let mut wire = vec![0; STREAM_HEADER_LEN + records as usize * 8];
        header.encode(&mut wire);
        for (index, value) in wire[STREAM_HEADER_LEN..].iter_mut().enumerate() {
            *value = index as u8;
        }
        Packet::decode(wire.into(), 0).unwrap()
    }

    #[test]
    fn history_is_bounded_by_whole_packets() {
        let mut history = History::default();
        history.reset(5);
        history.push(packet(0, 0, 3));
        history.push(packet(1, 6, 3));
        assert_eq!(history.records(), 3);
    }

    #[test]
    fn recent_trims_only_the_oldest_packet() {
        let mut history = History::default();
        history.reset(20);
        history.push(packet(4, 100, 4));
        history.push(packet(5, 108, 3));
        let replay = history.recent(5).unwrap();
        assert_eq!(replay.len(), 2);
        let first = StreamHeader::decode(&replay[0]).unwrap();
        assert_eq!((first.seq, first.first_index, first.n_records), (4, 104, 2));
        assert_eq!(
            replay
                .iter()
                .map(|p| StreamHeader::decode(p).unwrap().n_records as usize)
                .sum::<usize>(),
            5
        );
        assert!(history.recent(8).is_none());
    }
}
