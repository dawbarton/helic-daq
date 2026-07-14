//! UDP sample streamer: drains the core-1 record ring every few
//! milliseconds and, when a stream session is active, batches selected
//! values into packets per `docs/protocol.md`.

use core::sync::atomic::Ordering;

use defmt::info;
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::{IpEndpoint, Stack};
use embassy_time::{Duration, Ticker};
use helic_proto::source;
use helic_proto::stream::{StreamHeader, STREAM_HEADER_LEN};

use super::STREAM;
use crate::rt_loop::{Record, RecordConsumer, RECORDS_DROPPED};

/// Payload budget: fits an unfragmented packet in a standard 1500 MTU.
const MAX_PACKET: usize = 1472;

/// Flush interval: bounds streaming latency; at 8 kHz this is ~40 records
/// per flush, well inside the 256-record ring.
const FLUSH_MS: u64 = 5;

fn record_value(r: &Record, src: u8) -> f32 {
    match src {
        0..=10 => r.values[src as usize],
        // Protocol v1 has no table slot. During the phase-2 transition its
        // OUT id maps to the final source in the discoverable record shape.
        source::OUT => r.values[r.n.saturating_sub(1) as usize],
        _ => 0.0,
    }
}

#[embassy_executor::task]
pub async fn stream_task(stack: Stack<'static>, mut records: RecordConsumer) -> ! {
    let mut rx_meta = [PacketMetadata::EMPTY; 2];
    let mut rx_buf = [0u8; 64];
    let mut tx_meta = [PacketMetadata::EMPTY; 8];
    let mut tx_buf = [0u8; 4 * MAX_PACKET];
    let mut socket = UdpSocket::new(stack, &mut rx_meta, &mut rx_buf, &mut tx_meta, &mut tx_buf);
    socket
        .bind(helic_proto::STREAM_PORT)
        .expect("UDP bind failed");

    let mut packet = [0u8; MAX_PACKET];
    let mut ticker = Ticker::every(Duration::from_millis(FLUSH_MS));
    let mut seq: u32 = 0;
    let mut remaining: u32 = 0; // records left in a finite session; 0 when continuous
    let mut generation: u32 = 0;

    loop {
        ticker.next().await;

        // Snapshot the session config.
        let (enabled, target, sources, decimation, count, gen) = STREAM.lock(|s| {
            let s = s.borrow();
            (
                s.enabled,
                s.target,
                s.sources.clone(),
                s.decimation.max(1) as u32,
                s.count,
                s.generation,
            )
        });

        if gen != generation {
            // New StreamStart: re-arm.
            generation = gen;
            seq = 0;
            remaining = count;
            info!("stream: armed ({} sources)", sources.len());
        }

        let (Some((addr, port)), true) = (target, enabled) else {
            // Not streaming: keep the ring drained so old data never leaks
            // into a future session.
            while records.dequeue().is_some() {}
            continue;
        };
        let endpoint = IpEndpoint::new(addr.into(), port);

        // Drain the ring into as many packets as needed.
        let rec_size = 4 * sources.len();
        let max_records = ((MAX_PACKET - STREAM_HEADER_LEN) / rec_size).min(u16::MAX as usize);
        'flush: loop {
            let mut n: usize = 0;
            let mut first_index = 0u32;
            while n < max_records {
                let Some(r) = records.dequeue() else { break };
                if r.index % decimation != 0 {
                    continue;
                }
                if n == 0 {
                    first_index = r.index;
                }
                let base = STREAM_HEADER_LEN + n * rec_size;
                for (slot, &src) in sources.iter().enumerate() {
                    packet[base + 4 * slot..base + 4 * slot + 4]
                        .copy_from_slice(&record_value(&r, src).to_le_bytes());
                }
                n += 1;
                if count != 0 {
                    remaining -= 1;
                    if remaining == 0 {
                        break;
                    }
                }
            }
            if n == 0 {
                break 'flush;
            }

            StreamHeader {
                n_sources: sources.len() as u8,
                seq,
                first_index,
                dropped: RECORDS_DROPPED.load(Ordering::Relaxed),
                decimation: decimation as u16,
                n_records: n as u16,
            }
            .encode(&mut packet);
            seq = seq.wrapping_add(1);

            // Best-effort: a full TX buffer drops the packet (UDP semantics)
            // rather than stalling the drain.
            let _ = socket
                .send_to(&packet[..STREAM_HEADER_LEN + n * rec_size], endpoint)
                .await;

            if count != 0 && remaining == 0 {
                STREAM.lock(|s| s.borrow_mut().enabled = false);
                info!("stream: finite capture complete");
                break 'flush;
            }
            if n < max_records {
                break 'flush; // ring drained
            }
        }
    }
}
