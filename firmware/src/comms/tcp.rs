//! TCP control server: one connection at a time, framed request/response
//! per `docs/protocol.md`. All parameter access goes through [`ParamStore`].

use cbc_proto::frame::{self, MsgType, HEADER_LEN, MAX_PAYLOAD, TRAILER_LEN};
use cbc_proto::{source, ErrorCode, MAGIC, VERSION};
use defmt::{info, warn};
use embassy_net::tcp::TcpSocket;
use embassy_net::{IpAddress, Stack};
use embassy_time::{Duration, Instant};
use embedded_io_async::Read;

use super::{MAX_STREAM_SOURCES, STREAM};
use crate::config::SAMPLE_RATE;
use crate::params::ParamStore;

#[embassy_executor::task]
pub async fn control_task(stack: Stack<'static>, mut store: ParamStore) -> ! {
    let mut rx_buf = [0u8; 2048];
    let mut tx_buf = [0u8; 2048];
    loop {
        let mut socket = TcpSocket::new(stack, &mut rx_buf, &mut tx_buf);
        socket.set_timeout(Some(Duration::from_secs(30)));
        if socket.accept(cbc_proto::CONTROL_PORT).await.is_err() {
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
async fn serve(socket: &mut TcpSocket<'_>, store: &mut ParamStore) {
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
        if socket.write(&resp_frame[..n]).await.is_err() {
            return;
        }
        let _ = socket.flush().await;
    }
}

fn handle(
    ty: u8,
    payload: &[u8],
    store: &mut ParamStore,
    socket: &TcpSocket<'_>,
    resp: &mut [u8; MAX_PAYLOAD],
) -> Result<usize, ErrorCode> {
    let Some(msg) = MsgType::from_u8(ty) else {
        return Err(ErrorCode::UnknownType);
    };
    match msg {
        MsgType::GetParNames => {
            let mut off = 0;
            for i in 0..store.count() {
                let name = store.def(i).unwrap().name.as_bytes();
                if off + name.len() + 1 > MAX_PAYLOAD {
                    return Err(ErrorCode::BadLength);
                }
                resp[off..off + name.len()].copy_from_slice(name);
                resp[off + name.len()] = 0;
                off += name.len() + 1;
            }
            Ok(off)
        }
        MsgType::GetParInfo => {
            let mut off = 0;
            for i in 0..store.count() {
                let def = store.def(i).unwrap();
                resp[off] = def.ty as u8;
                resp[off + 1..off + 3].copy_from_slice(&def.count.to_le_bytes());
                resp[off + 3] = def.writable as u8;
                off += 4;
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
        MsgType::SetBlock | MsgType::Commit => Err(ErrorCode::UnknownType), // reserved
        MsgType::StreamSetup => {
            // decimation u16, count u32, n u8, sources u8[n]
            if payload.len() < 7 {
                return Err(ErrorCode::BadLength);
            }
            let decimation = u16::from_le_bytes([payload[0], payload[1]]);
            let count = u32::from_le_bytes([payload[2], payload[3], payload[4], payload[5]]);
            let n = payload[6] as usize;
            if decimation == 0 || n == 0 || n > MAX_STREAM_SOURCES || payload.len() != 7 + n {
                return Err(ErrorCode::BadValue);
            }
            let sources = &payload[7..7 + n];
            if sources.iter().any(|&s| s >= source::COUNT) {
                return Err(ErrorCode::BadValue);
            }
            STREAM.lock(|s| {
                let mut s = s.borrow_mut();
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
            resp[3..7].copy_from_slice(&SAMPLE_RATE.hz().to_le_bytes());
            let uptime_ms = Instant::now().as_millis() as u32;
            resp[7..11].copy_from_slice(&uptime_ms.to_le_bytes());
            Ok(11)
        }
        MsgType::Error => Err(ErrorCode::UnknownType),
    }
}
