//! TCP control server: one connection at a time, framed request/response
//! per `docs/protocol.md`. All parameter access goes through [`ParamStore`].

use defmt::{info, warn};
use embassy_net::tcp::TcpSocket;
use embassy_net::{IpAddress, Stack};
use embassy_time::{Duration, Instant};
use embedded_io_async::{Read, Write};
use helic_core::controller::Controller;
use helic_proto::frame::{self, MsgType, HEADER_LEN, MAX_PAYLOAD, TRAILER_LEN};
use helic_proto::payload;
use helic_proto::{ErrorCode, MAGIC, VERSION};

use super::STREAM;
use crate::params::ParamStore;
use crate::rig::{source, source_count, Rig, MAX_SOURCES};

pub async fn control_run<C: Controller, R: Rig>(
    stack: Stack<'static>,
    mut store: ParamStore<C, R>,
) -> ! {
    let mut rx_buf = [0u8; 2048];
    let mut tx_buf = [0u8; 2048];
    loop {
        let mut socket = TcpSocket::new(stack, &mut rx_buf, &mut tx_buf);
        socket.set_timeout(Some(Duration::from_secs(30)));
        if socket.accept(helic_proto::CONTROL_PORT).await.is_err() {
            continue;
        }
        info!("control: client connected");
        serve(&mut socket, &mut store).await;
        // Stop streaming when the controlling connection goes away.
        STREAM.lock(|s| s.borrow_mut().enabled = false);
        socket.close();
        info!("control: client disconnected");
    }
}

/// Handle framed requests until the connection drops or a framing error
/// makes resynchronisation impossible (TCP guarantees ordering, so any
/// framing error means a broken peer).
async fn serve<C: Controller, R: Rig>(socket: &mut TcpSocket<'_>, store: &mut ParamStore<C, R>) {
    let mut frame_buf = [0u8; HEADER_LEN + MAX_PAYLOAD + TRAILER_LEN];
    let mut resp_payload = [0u8; MAX_PAYLOAD];
    let mut resp_frame = [0u8; HEADER_LEN + MAX_PAYLOAD + TRAILER_LEN];

    loop {
        // Read one complete frame: fixed header, then payload + CRC.
        if socket
            .read_exact(&mut frame_buf[..HEADER_LEN])
            .await
            .is_err()
        {
            return;
        }
        if frame_buf[0..2] != MAGIC.to_le_bytes() {
            warn!("control: bad magic, dropping connection");
            return;
        }
        let len = u16::from_le_bytes([frame_buf[4], frame_buf[5]]) as usize;
        if len > MAX_PAYLOAD {
            warn!("control: oversized frame, dropping connection");
            return;
        }
        let total = HEADER_LEN + len + TRAILER_LEN;
        if socket
            .read_exact(&mut frame_buf[HEADER_LEN..total])
            .await
            .is_err()
        {
            return;
        }
        let (ty, seq, payload) = match frame::decode(&frame_buf[..total]) {
            Ok(f) => f,
            Err(_) => {
                warn!("control: CRC error, dropping connection");
                return;
            }
        };

        // Dispatch. `handle` returns either a response payload length or an
        // error code to report.
        let result = handle(ty, payload, store, socket, &mut resp_payload);
        let n = match result {
            Ok(n) => frame::encode(&mut resp_frame, ty, seq, &resp_payload[..n]),
            Err(code) => frame::encode(
                &mut resp_frame,
                MsgType::Error as u8,
                seq,
                &[code as u8, ty],
            ),
        }
        .expect("response encoding cannot fail");
        // write_all: the inherent `write` may accept only part of the frame,
        // which would silently corrupt the response stream.
        if socket.write_all(&resp_frame[..n]).await.is_err() {
            return;
        }
        let _ = socket.flush().await;
    }
}

fn handle<C: Controller, R: Rig>(
    ty: u8,
    payload: &[u8],
    store: &mut ParamStore<C, R>,
    socket: &TcpSocket<'_>,
    resp: &mut [u8; MAX_PAYLOAD],
) -> Result<usize, ErrorCode> {
    let Some(msg) = MsgType::from_u8(ty) else {
        return Err(ErrorCode::UnknownType);
    };
    match msg {
        MsgType::GetParams => {
            let mut off = 0;
            for i in 0..store.count() {
                let def = store.def(i).unwrap();
                off += payload::encode_param(
                    &mut resp[off..],
                    def.name,
                    def.ty,
                    def.count,
                    def.writable,
                )
                .map_err(|_| ErrorCode::BadLength)?;
            }
            Ok(off)
        }
        MsgType::GetSources => {
            let mut off = 0;
            for i in 0..source_count::<R>() {
                let (name, unit) = source::<R>(i).unwrap();
                off += payload::encode_source(&mut resp[off..], name, unit)
                    .map_err(|_| ErrorCode::BadLength)?;
            }
            Ok(off)
        }
        MsgType::GetPar => {
            if payload.is_empty() || !payload.len().is_multiple_of(2) {
                return Err(ErrorCode::BadLength);
            }
            let mut off = 0;
            for pair in payload.chunks_exact(2) {
                let index = u16::from_le_bytes([pair[0], pair[1]]) as usize;
                off += store.get(index, &mut resp[off..])?;
            }
            Ok(off)
        }
        MsgType::SetPar => {
            if payload.len() < 2 {
                return Err(ErrorCode::BadLength);
            }
            let index = u16::from_le_bytes([payload[0], payload[1]]) as usize;
            store.set(index, &payload[2..])?;
            Ok(0)
        }
        MsgType::SetBlock => {
            let (index, offset, data) =
                payload::decode_set_block(payload).map_err(|_| ErrorCode::BadLength)?;
            store.set_block(index as usize, offset, data)?;
            Ok(0)
        }
        MsgType::Commit => {
            let (index, len) = payload::decode_commit(payload).map_err(|_| ErrorCode::BadLength)?;
            store.commit(index as usize, len)?;
            Ok(0)
        }
        MsgType::StreamSetup => {
            // decimation u16, count u32, n u8, sources u8[n]
            if payload.len() < 7 {
                return Err(ErrorCode::BadLength);
            }
            let decimation = u16::from_le_bytes([payload[0], payload[1]]);
            let count = u32::from_le_bytes([payload[2], payload[3], payload[4], payload[5]]);
            let n = payload[6] as usize;
            if decimation == 0 || n == 0 || n > MAX_SOURCES || payload.len() != 7 + n {
                return Err(ErrorCode::BadValue);
            }
            let sources = &payload[7..7 + n];
            if sources.iter().any(|&s| s as usize >= source_count::<R>()) {
                return Err(ErrorCode::BadValue);
            }
            STREAM.lock(|s| {
                let mut s = s.borrow_mut();
                // Reconfiguring a live stream would change the packet layout
                // mid-session without re-arming the streamer; require a
                // StreamStop first.
                if s.enabled {
                    return Err(ErrorCode::Busy);
                }
                if s.sources.len() < n {
                    s.sources.clear();
                }
                s.sources
                    .resize_default(n)
                    .map_err(|_| ErrorCode::BadValue)?;
                s.sources[..n].copy_from_slice(sources);
                s.sources.truncate(n);
                s.decimation = decimation;
                s.count = count;
                Ok(())
            })?;
            Ok(0)
        }
        MsgType::StreamStart => {
            if payload.len() != 2 {
                return Err(ErrorCode::BadLength);
            }
            let port = u16::from_le_bytes([payload[0], payload[1]]);
            let Some(remote) = socket.remote_endpoint() else {
                return Err(ErrorCode::BadValue);
            };
            let IpAddress::Ipv4(addr) = remote.addr;
            STREAM.lock(|s| {
                let mut s = s.borrow_mut();
                if s.sources.is_empty() {
                    return Err(ErrorCode::BadValue);
                }
                s.target = Some((addr, port));
                s.enabled = true;
                s.generation = s.generation.wrapping_add(1);
                Ok(())
            })?;
            Ok(0)
        }
        MsgType::StreamStop => {
            STREAM.lock(|s| s.borrow_mut().enabled = false);
            Ok(0)
        }
        MsgType::Status => {
            resp[0] = VERSION;
            resp[1..3].copy_from_slice(&(store.count() as u16).to_le_bytes());
            resp[3] = source_count::<R>() as u8;
            resp[4..8].copy_from_slice(&store.sample_rate().hz().to_le_bytes());
            let uptime_ms = Instant::now().as_millis() as u32;
            resp[8..12].copy_from_slice(&uptime_ms.to_le_bytes());
            Ok(12)
        }
        MsgType::Error => Err(ErrorCode::UnknownType),
    }
}
