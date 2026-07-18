//! Loopback TCP/UDP system test for shared streaming, replay, and recording.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Result};
use helic_broker::config::Config;
use helic_proto::beacon::{BeaconResponse, REQUEST as BEACON_REQUEST, RESPONSE_LEN};
use helic_proto::broker::{self, BrokerInfo, STATE_CLIENT_QUIET, STATE_RUNNING};
use helic_proto::frame::{self, MsgType, HEADER_LEN, TRAILER_LEN};
use helic_proto::stream::{StreamHeader, STREAM_HEADER_LEN};
use rust_hdf5::swmr::SwmrFileReader;
use tempfile::tempdir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::Mutex;
use tokio::time::{sleep, timeout};

type SharedConfiguration = Arc<Mutex<Option<(u16, Vec<u8>)>>>;

#[derive(Clone, Default)]
struct FakeState {
    configuration: SharedConfiguration,
    target: Arc<Mutex<Option<SocketAddr>>>,
    disarmed: Arc<AtomicBool>,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_clients_share_stream_replay_and_recording() -> Result<()> {
    let mcu_control = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let mcu_control_port = mcu_control.local_addr()?.port();
    let mcu_stream = Arc::new(UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await?);
    let mcu_stream_port = mcu_stream.local_addr()?.port();
    let fake = FakeState::default();
    tokio::spawn(fake_mcu_control(mcu_control, fake.clone()));
    tokio::spawn(fake_mcu_stream(mcu_stream, fake.clone()));

    let output = tempdir()?;
    let control_port = free_tcp_port()?;
    let stream_port = free_udp_port()?;
    let discovery_port = free_udp_port()?;
    let broker = tokio::spawn(helic_broker::server::run(Config {
        mcu_host: Ipv4Addr::LOCALHOST.to_string(),
        output_dir: output.path().to_path_buf(),
        mcu_control_port,
        mcu_stream_port,
        mcu_discovery_port: free_udp_port()?,
        control_port,
        stream_port,
        discovery_port,
        history: Duration::from_secs(2),
        segment_size: 1 << 30,
        request_timeout: Duration::from_millis(500),
        reconnect_delay: Duration::from_millis(20),
        log_level: "warn".into(),
    }));

    let mut first = connect_when_ready(control_port).await?;
    let mut second = connect_when_ready(control_port).await?;
    let discovery = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    discovery
        .send_to(&BEACON_REQUEST, (Ipv4Addr::LOCALHOST, discovery_port))
        .await?;
    let mut beacon_bytes = [0; RESPONSE_LEN];
    let length = timeout(Duration::from_secs(1), discovery.recv(&mut beacon_bytes)).await??;
    let beacon = BeaconResponse::decode(&beacon_bytes[..length]).expect("valid broker beacon");
    assert_eq!(beacon.control_port, control_port);
    assert!(beacon.experiment.starts_with(b"cbc-rig\0"));
    assert!(beacon.firmware.starts_with(b"helic-broker "));

    let setup = [1, 0, 0, 0, 0, 0, 2, 0, 1];
    assert_ack(&mut first, MsgType::StreamSetup as u8, &setup).await?;

    let first_udp = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    first_udp
        .send_to(b"primer", (Ipv4Addr::LOCALHOST, stream_port))
        .await?;
    assert_ack(
        &mut first,
        MsgType::StreamStart as u8,
        &first_udp.local_addr()?.port().to_le_bytes(),
    )
    .await?;
    let first_packet = receive_packet(&first_udp).await?;
    assert_eq!(first_packet.n_sources, 2);

    let second_udp = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    second_udp
        .send_to(b"primer", (Ipv4Addr::LOCALHOST, stream_port))
        .await?;
    assert_ack(
        &mut second,
        broker::QUIET_STREAM_START,
        &second_udp.local_addr()?.port().to_le_bytes(),
    )
    .await?;
    assert!(
        timeout(Duration::from_millis(60), receive_packet(&second_udp))
            .await
            .is_err()
    );

    sleep(Duration::from_millis(80)).await;
    let (response_type, information) = request(&mut second, broker::BROKER_INFO, &[]).await?;
    assert_eq!(response_type, broker::BROKER_INFO);
    let information = BrokerInfo::decode(&information).expect("valid BrokerInfo");
    assert_ne!(information.state & STATE_RUNNING, 0);
    assert_ne!(information.state & STATE_CLIENT_QUIET, 0);
    assert_eq!(information.connected_clients, 2);
    assert_eq!(&information.sources[..2], &[0, 1]);

    let requested = 10u32;
    let (response_type, response) =
        request(&mut second, broker::GET_RECENT, &requested.to_le_bytes()).await?;
    assert_eq!(response_type, broker::GET_RECENT);
    assert_eq!(response, requested.to_le_bytes());
    let mut replayed = 0usize;
    while replayed < requested as usize {
        replayed += receive_packet(&second_udp).await?.n_records as usize;
    }
    assert_eq!(replayed, requested as usize);

    assert_ack(&mut second, broker::SET_CLIENT_QUIET, &[0]).await?;
    receive_packet(&second_udp).await?;
    assert_ack(&mut second, MsgType::StreamStop as u8, &[]).await?;

    let files = std::fs::read_dir(output.path())?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    assert_eq!(files.len(), 1);
    assert_eq!(
        files[0].extension().and_then(|value| value.to_str()),
        Some("h5")
    );
    let mut recording = SwmrFileReader::open(&files[0])?;
    assert_eq!(recording.dataset_shape("records/values")?[1], 2);
    assert!(!recording.read_dataset::<f32>("records/values")?.is_empty());

    drop(first);
    drop(second);
    timeout(Duration::from_secs(1), async {
        while !fake.disarmed.load(Ordering::Acquire) {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    broker.abort();
    Ok(())
}

async fn fake_mcu_control(listener: TcpListener, state: FakeState) -> Result<()> {
    let (mut stream, peer) = listener.accept().await?;
    loop {
        let (message_type, sequence, payload) = match read_frame(&mut stream).await {
            Ok(frame) => frame,
            Err(_) => return Ok(()),
        };
        let mut response_type = message_type;
        let response = match message_type {
            value if value == MsgType::Status as u8 => {
                let mut payload = vec![helic_proto::VERSION];
                payload.extend_from_slice(&3u16.to_le_bytes());
                payload.push(2);
                payload.extend_from_slice(&1000f32.to_le_bytes());
                payload.extend_from_slice(&42u32.to_le_bytes());
                payload
            }
            value if value == MsgType::GetParams as u8 => parameter_page(&payload),
            value if value == MsgType::GetSources as u8 => b"adc0\0V\0out\0V\0".to_vec(),
            value if value == MsgType::GetPar as u8 => parameter_values(&payload),
            value if value == MsgType::SetPar as u8 => {
                if payload == [2, 0, 0, 0, 0, 0] {
                    state.disarmed.store(true, Ordering::Release);
                }
                Vec::new()
            }
            value if value == MsgType::StreamSetup as u8 => {
                if payload.len() != 9 {
                    response_type = MsgType::Error as u8;
                    vec![6, message_type]
                } else {
                    let decimation = u16::from_le_bytes([payload[0], payload[1]]);
                    *state.configuration.lock().await = Some((decimation, payload[7..].to_vec()));
                    Vec::new()
                }
            }
            value if value == MsgType::StreamStart as u8 => {
                let port = u16::from_le_bytes([payload[0], payload[1]]);
                *state.target.lock().await = Some(SocketAddr::new(peer.ip(), port));
                Vec::new()
            }
            value if value == MsgType::StreamStop as u8 => {
                *state.target.lock().await = None;
                Vec::new()
            }
            _ => {
                response_type = MsgType::Error as u8;
                vec![2, message_type]
            }
        };
        write_frame(&mut stream, response_type, sequence, &response).await?;
    }
}

async fn fake_mcu_stream(socket: Arc<UdpSocket>, state: FakeState) -> Result<()> {
    let mut sequence = 0u32;
    let mut first_index = 0u32;
    loop {
        sleep(Duration::from_millis(10)).await;
        let Some(target) = *state.target.lock().await else {
            continue;
        };
        let Some((decimation, sources)) = state.configuration.lock().await.clone() else {
            continue;
        };
        let header = StreamHeader {
            n_sources: sources.len() as u8,
            seq: sequence,
            first_index,
            dropped: 0,
            decimation,
            n_records: 8,
        };
        let mut packet = vec![0; STREAM_HEADER_LEN];
        header.encode(&mut packet);
        for record in 0..header.n_records {
            for source in &sources {
                let value = first_index as f32 + record as f32 + *source as f32 / 10.0;
                packet.extend_from_slice(&value.to_le_bytes());
            }
        }
        socket.send_to(&packet, target).await?;
        sequence = sequence.wrapping_add(1);
        first_index = first_index.wrapping_add(decimation as u32 * header.n_records as u32);
    }
}

fn parameter_page(request: &[u8]) -> Vec<u8> {
    assert_eq!(request, [0, 0]);
    let mut payload = Vec::new();
    payload.extend_from_slice(&0u16.to_le_bytes());
    payload.extend_from_slice(&3u16.to_le_bytes());
    for (name, type_code, count, writable) in [
        ("firmware", b'c', 16u16, 0u8),
        ("experiment", b'c', 16u16, 0u8),
        ("arm", b'I', 1u16, 1u8),
    ] {
        payload.extend_from_slice(name.as_bytes());
        payload.push(0);
        payload.push(type_code);
        payload.extend_from_slice(&count.to_le_bytes());
        payload.push(writable);
    }
    payload
}

fn parameter_values(request: &[u8]) -> Vec<u8> {
    request
        .chunks_exact(2)
        .flat_map(|index| match u16::from_le_bytes([index[0], index[1]]) {
            0 => fixed_text("helic-daq test").to_vec(),
            1 => fixed_text("cbc-rig").to_vec(),
            2 => 0u32.to_le_bytes().to_vec(),
            _ => Vec::new(),
        })
        .collect()
}

fn fixed_text(value: &str) -> [u8; 16] {
    let mut result = [0; 16];
    result[..value.len()].copy_from_slice(value.as_bytes());
    result
}

async fn connect_when_ready(port: u16) -> Result<TcpStream> {
    for _ in 0..100 {
        if let Ok(mut stream) = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await {
            if let Ok(Ok((response_type, _))) = timeout(
                Duration::from_millis(100),
                request(&mut stream, MsgType::Status as u8, &[]),
            )
            .await
            {
                if response_type == MsgType::Status as u8 {
                    return Ok(stream);
                }
            }
        }
        sleep(Duration::from_millis(20)).await;
    }
    bail!("broker did not become ready")
}

async fn assert_ack(stream: &mut TcpStream, message_type: u8, payload: &[u8]) -> Result<()> {
    let (response_type, response) = request(stream, message_type, payload).await?;
    assert_eq!(response_type, message_type, "error response: {response:?}");
    assert!(response.is_empty());
    Ok(())
}

async fn request(
    stream: &mut TcpStream,
    message_type: u8,
    payload: &[u8],
) -> Result<(u8, Vec<u8>)> {
    write_frame(stream, message_type, 1, payload).await?;
    let (response_type, _, response) = read_frame(stream).await?;
    Ok((response_type, response))
}

async fn read_frame(stream: &mut TcpStream) -> Result<(u8, u8, Vec<u8>)> {
    let mut header = [0; HEADER_LEN];
    stream.read_exact(&mut header).await?;
    let length = u16::from_le_bytes([header[4], header[5]]) as usize;
    let mut frame_bytes = header.to_vec();
    frame_bytes.resize(HEADER_LEN + length + TRAILER_LEN, 0);
    stream.read_exact(&mut frame_bytes[HEADER_LEN..]).await?;
    let (message_type, sequence, payload) =
        frame::decode(&frame_bytes).map_err(|error| anyhow::anyhow!("{error:?}"))?;
    Ok((message_type, sequence, payload.to_vec()))
}

async fn write_frame(
    stream: &mut TcpStream,
    message_type: u8,
    sequence: u8,
    payload: &[u8],
) -> Result<()> {
    let mut encoded = vec![0; HEADER_LEN + payload.len() + TRAILER_LEN];
    frame::encode(&mut encoded, message_type, sequence, payload)
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    stream.write_all(&encoded).await?;
    Ok(())
}

async fn receive_packet(socket: &UdpSocket) -> Result<StreamHeader> {
    let mut packet = [0; 2048];
    let length = timeout(Duration::from_secs(1), socket.recv(&mut packet)).await??;
    StreamHeader::decode(&packet[..length]).ok_or_else(|| anyhow::anyhow!("invalid stream packet"))
}

fn free_tcp_port() -> Result<u16> {
    Ok(std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?
        .local_addr()?
        .port())
}

fn free_udp_port() -> Result<u16> {
    Ok(std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?
        .local_addr()?
        .port())
}
