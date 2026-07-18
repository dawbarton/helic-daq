//! Async loopback servers and the single shared upstream MCU session.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use helic_proto::beacon::{BeaconResponse, REQUEST as BEACON_REQUEST, RESPONSE_LEN};
use helic_proto::broker::{
    self, BrokerError, BrokerInfo, CAPABILITIES, MAX_SOURCES, STATE_CLIENT_ATTACHED,
    STATE_CLIENT_QUIET, STATE_CONFIGURED, STATE_RUNNING, STATE_TABLE_TRANSACTION,
    STATE_UPSTREAM_CONNECTED,
};
use helic_proto::frame::{self, MsgType, HEADER_LEN, MAX_PAYLOAD, TRAILER_LEN};
use helic_proto::{ErrorCode, MAGIC};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{lookup_host, TcpListener, TcpStream, UdpSocket};
use tokio::sync::{watch, Mutex, Notify, RwLock};
use tokio::time::{interval, sleep, timeout};

use crate::config::Config;
use crate::history::{History, Packet};
use crate::storage::{utc_now_ns, CloseReason, SessionMetadata, SourceMetadata, StorageHandle};

const PRIMER: &[u8] = b"helic-daq-stream-prime";
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
const TABLE_TRANSACTION_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug)]
struct ParameterDefinition {
    index: u16,
    name: String,
    type_code: u8,
    count: u16,
}

impl ParameterDefinition {
    fn size(&self) -> usize {
        let element = match self.type_code {
            b'B' | b'b' | b'c' => 1,
            b'H' | b'h' => 2,
            b'I' | b'i' | b'f' => 4,
            _ => 0,
        };
        element * self.count as usize
    }
}

#[derive(Clone, Debug)]
struct DeviceMetadata {
    sample_rate_hz: f32,
    sources: Vec<SourceMetadata>,
    experiment: String,
    firmware: String,
    arm_index: Option<u16>,
    mac: [u8; 6],
}

#[derive(Clone, Debug)]
struct StreamConfiguration {
    decimation: u16,
    count: u32,
    sources: Vec<u8>,
}

impl StreamConfiguration {
    fn decode(payload: &[u8]) -> Result<Self> {
        if payload.len() < 7 {
            bail!("StreamSetup payload is too short");
        }
        let decimation = u16::from_le_bytes([payload[0], payload[1]]);
        let count = u32::from_le_bytes([payload[2], payload[3], payload[4], payload[5]]);
        let n_sources = payload[6] as usize;
        if decimation == 0
            || n_sources == 0
            || n_sources > MAX_SOURCES
            || payload.len() != 7 + n_sources
        {
            bail!("invalid StreamSetup values");
        }
        Ok(Self {
            decimation,
            count,
            sources: payload[7..].to_vec(),
        })
    }
}

#[derive(Clone, Debug, Default)]
struct ClientState {
    endpoint: Option<SocketAddr>,
    quiet: bool,
}

#[derive(Clone, Debug)]
struct TableOwner {
    client_id: u64,
    touched: Instant,
}

#[derive(Debug, Default)]
struct SharedState {
    configuration: Option<StreamConfiguration>,
    running: bool,
    received_records: u64,
    clients: HashMap<u64, ClientState>,
    history: History,
    table_owner: Option<TableOwner>,
}

impl SharedState {
    fn expire_table_owner(&mut self) {
        if self
            .table_owner
            .as_ref()
            .is_some_and(|owner| owner.touched.elapsed() >= TABLE_TRANSACTION_TIMEOUT)
        {
            self.table_owner = None;
        }
    }

    fn detach_all(&mut self) {
        for client in self.clients.values_mut() {
            client.endpoint = None;
            client.quiet = false;
        }
    }
}

#[derive(Debug)]
struct UpstreamControl {
    stream: TcpStream,
    sequence: u8,
    generation: u64,
    peer_ip: Ipv4Addr,
    udp: Arc<UdpSocket>,
}

#[derive(Debug)]
struct UpstreamResponse {
    message_type: u8,
    payload: Vec<u8>,
}

impl UpstreamControl {
    async fn request(
        &mut self,
        message_type: u8,
        payload: &[u8],
        request_timeout: Duration,
    ) -> Result<UpstreamResponse> {
        self.sequence = self.sequence.wrapping_add(1);
        let sequence = self.sequence;
        let mut encoded = vec![0; HEADER_LEN + payload.len() + TRAILER_LEN];
        frame::encode(&mut encoded, message_type, sequence, payload)
            .map_err(|error| anyhow::anyhow!("could not encode upstream frame: {error:?}"))?;
        timeout(request_timeout, self.stream.write_all(&encoded))
            .await
            .context("upstream request timed out")??;
        let (response_type, response_sequence, response_payload) =
            timeout(request_timeout, read_frame(&mut self.stream))
                .await
                .context("upstream response timed out")??;
        if response_sequence != sequence {
            bail!("upstream sequence mismatch: sent {sequence}, received {response_sequence}");
        }
        Ok(UpstreamResponse {
            message_type: response_type,
            payload: response_payload,
        })
    }
}

struct App {
    config: Config,
    upstream: Mutex<Option<UpstreamControl>>,
    metadata: RwLock<Option<DeviceMetadata>>,
    state: Mutex<SharedState>,
    stream_operations: Mutex<()>,
    downstream_udp: Arc<UdpSocket>,
    storage: StorageHandle,
    epoch: watch::Sender<u64>,
    next_generation: AtomicU64,
    next_client: AtomicU64,
    fatal: watch::Sender<Option<String>>,
    reconnect: Notify,
}

impl App {
    async fn upstream_ready(&self) -> bool {
        self.upstream.lock().await.is_some()
    }

    async fn request(&self, message_type: u8, payload: &[u8]) -> Result<UpstreamResponse> {
        let (generation, result) = {
            let mut upstream = self.upstream.lock().await;
            let Some(upstream) = upstream.as_mut() else {
                bail!("MCU is not connected");
            };
            let generation = upstream.generation;
            let result = upstream
                .request(message_type, payload, self.config.request_timeout)
                .await;
            (generation, result)
        };
        match result {
            Ok(response) => Ok(response),
            Err(error) => {
                self.mark_upstream_failed(generation, &error.to_string())
                    .await;
                Err(error)
            }
        }
    }

    async fn mark_upstream_failed(&self, generation: u64, reason: &str) {
        let removed = {
            let mut upstream = self.upstream.lock().await;
            if upstream
                .as_ref()
                .is_some_and(|candidate| candidate.generation == generation)
            {
                upstream.take().is_some()
            } else {
                false
            }
        };
        if !removed {
            return;
        }
        tracing::warn!(%reason, "MCU connection lost");
        let was_running = {
            let mut state = self.state.lock().await;
            let was_running = state.running;
            state.running = false;
            state.configuration = None;
            state.received_records = 0;
            state.history.clear();
            state.detach_all();
            state.table_owner = None;
            was_running
        };
        if was_running {
            let _ = self.storage.stop(CloseReason::UpstreamLost).await;
        }
        *self.metadata.write().await = None;
        let next = self.epoch.borrow().wrapping_add(1);
        self.epoch.send_replace(next);
        self.reconnect.notify_one();
    }

    async fn force_upstream_disconnect(&self, reason: &str) {
        let generation = self
            .upstream
            .lock()
            .await
            .as_ref()
            .map(|upstream| upstream.generation);
        if let Some(generation) = generation {
            self.mark_upstream_failed(generation, reason).await;
        }
    }

    async fn current_udp(&self) -> Option<(Arc<UdpSocket>, Ipv4Addr, u64)> {
        self.upstream
            .lock()
            .await
            .as_ref()
            .map(|upstream| (upstream.udp.clone(), upstream.peer_ip, upstream.generation))
    }

    async fn broker_info(&self, client_id: u64) -> BrokerInfo {
        let connected = self.upstream_ready().await;
        let mut state = self.state.lock().await;
        state.expire_table_owner();
        let client = state.clients.get(&client_id).cloned().unwrap_or_default();
        let mut flags = 0;
        if connected {
            flags |= STATE_UPSTREAM_CONNECTED;
        }
        if state.configuration.is_some() {
            flags |= STATE_CONFIGURED;
        }
        if state.running {
            flags |= STATE_RUNNING;
        }
        if client.endpoint.is_some() {
            flags |= STATE_CLIENT_ATTACHED;
        }
        if client.quiet {
            flags |= STATE_CLIENT_QUIET;
        }
        if state.table_owner.is_some() {
            flags |= STATE_TABLE_TRANSACTION;
        }
        let mut sources = [0; MAX_SOURCES];
        let (decimation, count, n_sources) = if let Some(configuration) = &state.configuration {
            sources[..configuration.sources.len()].copy_from_slice(&configuration.sources);
            (
                configuration.decimation,
                configuration.count,
                configuration.sources.len() as u8,
            )
        } else {
            (1, 0, 0)
        };
        BrokerInfo {
            state: flags,
            capabilities: CAPABILITIES,
            history_capacity_ms: self.config.history.as_millis().min(u32::MAX as u128) as u32,
            history_available_records: state.history.records().min(u32::MAX as usize) as u32,
            decimation,
            count,
            connected_clients: state.clients.len().min(u16::MAX as usize) as u16,
            n_sources,
            sources,
        }
    }

    async fn register_client(&self) -> u64 {
        let client_id = self.next_client.fetch_add(1, Ordering::Relaxed);
        self.state
            .lock()
            .await
            .clients
            .insert(client_id, ClientState::default());
        client_id
    }

    async fn unregister_client(&self, client_id: u64) {
        let last = {
            let mut state = self.state.lock().await;
            state.clients.remove(&client_id);
            if state
                .table_owner
                .as_ref()
                .is_some_and(|owner| owner.client_id == client_id)
            {
                state.table_owner = None;
            }
            state.clients.is_empty()
        };
        if last {
            self.disarm().await;
        }
    }

    async fn disarm(&self) {
        let arm_index = self
            .metadata
            .read()
            .await
            .as_ref()
            .and_then(|metadata| metadata.arm_index);
        let Some(arm_index) = arm_index else {
            return;
        };
        let mut payload = Vec::with_capacity(6);
        payload.extend_from_slice(&arm_index.to_le_bytes());
        payload.extend_from_slice(&0u32.to_le_bytes());
        match self.request(MsgType::SetPar as u8, &payload).await {
            Ok(response) if response.message_type == MsgType::SetPar as u8 => {
                tracing::info!("disarmed MCU after final client disconnected");
            }
            Ok(_) => {
                self.force_upstream_disconnect("MCU did not acknowledge disarm")
                    .await;
            }
            Err(_) => {}
        }
    }

    async fn process_packet(&self, wire: Arc<[u8]>) -> Result<()> {
        let packet = Packet::decode(wire, utc_now_ns())?;
        let (targets, complete) = {
            let mut state = self.state.lock().await;
            let Some(configuration) = state.configuration.clone() else {
                return Ok(());
            };
            if !state.running {
                return Ok(());
            }
            if packet.header.n_sources as usize != configuration.sources.len()
                || packet.header.decimation != configuration.decimation
            {
                bail!("MCU stream layout changed inside a shared session");
            }
            self.storage.packet(packet.clone())?;
            state.history.push(packet.clone());
            state.received_records += packet.header.n_records as u64;
            let targets = state
                .clients
                .values()
                .filter(|client| !client.quiet)
                .filter_map(|client| client.endpoint)
                .collect::<Vec<_>>();
            let complete =
                configuration.count != 0 && state.received_records >= configuration.count as u64;
            (targets, complete)
        };
        for target in targets {
            if let Err(error) = self.downstream_udp.send_to(&packet.wire, target).await {
                tracing::warn!(%target, %error, "could not send stream packet to client");
            }
        }
        if complete {
            let _operation = self.stream_operations.lock().await;
            let was_running = {
                let mut state = self.state.lock().await;
                if !state.running {
                    false
                } else {
                    state.running = false;
                    state.received_records = 0;
                    state.history.clear();
                    state.detach_all();
                    true
                }
            };
            if was_running {
                self.storage.stop(CloseReason::CountComplete).await?;
            }
        }
        Ok(())
    }

    async fn shutdown(&self) {
        let running = self.state.lock().await.running;
        if running {
            let _ = self.request(MsgType::StreamStop as u8, &[]).await;
            let _ = self.storage.stop(CloseReason::BrokerShutdown).await;
        }
        self.disarm().await;
        self.storage.shutdown().await;
        self.upstream.lock().await.take();
    }
}

#[derive(Debug)]
struct Handled {
    response_type: u8,
    payload: Vec<u8>,
    datagrams: Vec<(Arc<[u8]>, SocketAddr)>,
}

impl Handled {
    fn response(response: UpstreamResponse) -> Self {
        Self {
            response_type: response.message_type,
            payload: response.payload,
            datagrams: Vec::new(),
        }
    }

    fn empty(message_type: u8) -> Self {
        Self {
            response_type: message_type,
            payload: Vec::new(),
            datagrams: Vec::new(),
        }
    }

    fn error(request_type: u8, code: u8) -> Self {
        Self {
            response_type: MsgType::Error as u8,
            payload: vec![code, request_type],
            datagrams: Vec::new(),
        }
    }
}

pub async fn run(config: Config) -> Result<()> {
    let tcp = TcpListener::bind((Config::LOOPBACK, config.control_port))
        .await
        .context("could not bind loopback control port")?;
    let downstream_udp = Arc::new(
        UdpSocket::bind((Config::LOOPBACK, config.stream_port))
            .await
            .context("could not bind loopback stream port")?,
    );
    let discovery = Arc::new(
        UdpSocket::bind((Config::LOOPBACK, config.discovery_port))
            .await
            .context("could not bind loopback discovery port")?,
    );
    let storage = StorageHandle::spawn(config.output_dir.clone(), config.segment_size);
    let storage_errors = storage.subscribe_errors();
    let (epoch, _) = watch::channel(0u64);
    let (fatal, fatal_rx) = watch::channel(None);
    let app = Arc::new(App {
        config,
        upstream: Mutex::new(None),
        metadata: RwLock::new(None),
        state: Mutex::new(SharedState::default()),
        stream_operations: Mutex::new(()),
        downstream_udp: downstream_udp.clone(),
        storage,
        epoch,
        next_generation: AtomicU64::new(1),
        next_client: AtomicU64::new(1),
        fatal,
        reconnect: Notify::new(),
    });

    tokio::spawn(discard_primers(downstream_udp));
    tokio::spawn(discovery_server(app.clone(), discovery));
    tokio::spawn(reconnect_loop(app.clone()));
    tokio::spawn(heartbeat_loop(app.clone()));
    tokio::spawn(storage_failure_monitor(app.clone(), storage_errors));

    tracing::info!(
        address = %SocketAddrV4::new(Config::LOOPBACK, app.config.control_port),
        "broker listening"
    );
    let mut fatal_rx = fatal_rx;
    loop {
        tokio::select! {
            accepted = tcp.accept() => {
                let (stream, peer) = accepted?;
                if !app.upstream_ready().await {
                    drop(stream);
                    continue;
                }
                let app = app.clone();
                tokio::spawn(async move {
                    if let Err(error) = serve_client(app, stream, peer).await {
                        tracing::debug!(%peer, %error, "client disconnected with error");
                    }
                });
            }
            result = tokio::signal::ctrl_c() => {
                result?;
                tracing::info!("shutdown requested");
                app.shutdown().await;
                return Ok(());
            }
            changed = fatal_rx.changed() => {
                changed.context("fatal-state channel closed")?;
                let reason = fatal_rx.borrow().clone();
                if let Some(reason) = reason {
                    app.shutdown().await;
                    bail!("broker stopped after fatal error: {reason}");
                }
            }
        }
    }
}

async fn reconnect_loop(app: Arc<App>) {
    loop {
        if app.upstream_ready().await {
            app.reconnect.notified().await;
            continue;
        }
        match connect_upstream(&app).await {
            Ok((upstream, metadata)) => {
                let generation = upstream.generation;
                let udp = upstream.udp.clone();
                let peer_ip = upstream.peer_ip;
                *app.metadata.write().await = Some(metadata.clone());
                *app.upstream.lock().await = Some(upstream);
                tracing::info!(
                    generation,
                    experiment = %metadata.experiment,
                    firmware = %metadata.firmware,
                    "connected to MCU"
                );
                tokio::spawn(upstream_udp_loop(app.clone(), udp, peer_ip, generation));
            }
            Err(error) => {
                tracing::warn!(%error, "could not connect to MCU; retrying");
                sleep(app.config.reconnect_delay).await;
            }
        }
    }
}

async fn connect_upstream(app: &App) -> Result<(UpstreamControl, DeviceMetadata)> {
    let address = resolve_ipv4(&app.config.mcu_host, app.config.mcu_control_port).await?;
    let stream = timeout(app.config.request_timeout, TcpStream::connect(address))
        .await
        .context("MCU connection timed out")??;
    stream.set_nodelay(true)?;
    let local_ip = match stream.local_addr()?.ip() {
        IpAddr::V4(address) => address,
        IpAddr::V6(_) => bail!("MCU connection selected IPv6; HELIC-DAQ requires IPv4"),
    };
    let udp = Arc::new(UdpSocket::bind((local_ip, 0)).await?);
    let generation = app.next_generation.fetch_add(1, Ordering::Relaxed);
    let mut upstream = UpstreamControl {
        stream,
        sequence: 0,
        generation,
        peer_ip: *address.ip(),
        udp,
    };
    let metadata = discover_device(&mut upstream, app).await?;
    Ok((upstream, metadata))
}

async fn discover_device(upstream: &mut UpstreamControl, app: &App) -> Result<DeviceMetadata> {
    let status = upstream
        .request(MsgType::Status as u8, &[], app.config.request_timeout)
        .await?;
    if status.message_type != MsgType::Status as u8 || status.payload.len() != 12 {
        bail!("invalid MCU Status response");
    }
    if status.payload[0] != helic_proto::VERSION {
        bail!(
            "protocol version mismatch: MCU {}, broker {}",
            status.payload[0],
            helic_proto::VERSION
        );
    }
    let parameter_count = u16::from_le_bytes([status.payload[1], status.payload[2]]);
    let source_count = status.payload[3] as usize;
    let sample_rate_hz = f32::from_le_bytes([
        status.payload[4],
        status.payload[5],
        status.payload[6],
        status.payload[7],
    ]);
    let parameters =
        discover_parameters(upstream, parameter_count, app.config.request_timeout).await?;
    let sources = discover_sources(upstream, source_count, app.config.request_timeout).await?;
    let experiment = read_char_parameter(
        upstream,
        &parameters,
        "experiment",
        app.config.request_timeout,
    )
    .await?;
    let firmware = read_char_parameter(
        upstream,
        &parameters,
        "firmware",
        app.config.request_timeout,
    )
    .await?;
    let arm_index = parameters
        .iter()
        .find(|parameter| parameter.name == "arm")
        .map(|parameter| parameter.index);
    let mac = query_mac(
        upstream.peer_ip,
        upstream.udp.local_addr()?.ip(),
        app.config.mcu_discovery_port,
    )
    .await
    .unwrap_or([0x02, b'H', b'L', 0, 0, 0xB0]);
    Ok(DeviceMetadata {
        sample_rate_hz,
        sources,
        experiment,
        firmware,
        arm_index,
        mac,
    })
}

async fn discover_parameters(
    upstream: &mut UpstreamControl,
    count: u16,
    request_timeout: Duration,
) -> Result<Vec<ParameterDefinition>> {
    let mut result = Vec::with_capacity(count as usize);
    let mut start = 0u16;
    while start < count {
        let response = upstream
            .request(
                MsgType::GetParams as u8,
                &start.to_le_bytes(),
                request_timeout,
            )
            .await?;
        if response.message_type != MsgType::GetParams as u8 || response.payload.len() < 4 {
            bail!("invalid GetParams response");
        }
        let returned = u16::from_le_bytes([response.payload[0], response.payload[1]]);
        let next = u16::from_le_bytes([response.payload[2], response.payload[3]]);
        if returned != start || next <= start || next > count {
            bail!("invalid GetParams page range");
        }
        let mut offset = 4usize;
        let mut index = start;
        while offset < response.payload.len() {
            let end = response.payload[offset..]
                .iter()
                .position(|byte| *byte == 0)
                .map(|relative| offset + relative)
                .context("unterminated parameter name")?;
            if end + 5 > response.payload.len() {
                bail!("truncated parameter definition");
            }
            let name = std::str::from_utf8(&response.payload[offset..end])?.to_string();
            let type_code = response.payload[end + 1];
            let count = u16::from_le_bytes([response.payload[end + 2], response.payload[end + 3]]);
            let definition = ParameterDefinition {
                index,
                name,
                type_code,
                count,
            };
            if definition.size() == 0 {
                bail!("unknown parameter type code");
            }
            result.push(definition);
            index += 1;
            offset = end + 5;
        }
        if index != next {
            bail!("GetParams definition count does not match page range");
        }
        start = next;
    }
    Ok(result)
}

async fn discover_sources(
    upstream: &mut UpstreamControl,
    count: usize,
    request_timeout: Duration,
) -> Result<Vec<SourceMetadata>> {
    let response = upstream
        .request(MsgType::GetSources as u8, &[], request_timeout)
        .await?;
    if response.message_type != MsgType::GetSources as u8 {
        bail!("invalid GetSources response");
    }
    let mut offset = 0usize;
    let mut result = Vec::new();
    while offset < response.payload.len() {
        let (name, next) = decode_nul_ascii(&response.payload, offset)?;
        let (unit, next) = decode_nul_ascii(&response.payload, next)?;
        result.push(SourceMetadata { name, unit });
        offset = next;
    }
    if result.len() != count {
        bail!("GetSources count does not match Status");
    }
    Ok(result)
}

async fn read_char_parameter(
    upstream: &mut UpstreamControl,
    parameters: &[ParameterDefinition],
    name: &str,
    request_timeout: Duration,
) -> Result<String> {
    let parameter = parameters
        .iter()
        .find(|parameter| parameter.name == name)
        .with_context(|| format!("MCU has no {name:?} parameter"))?;
    if parameter.type_code != b'c' {
        bail!("{name:?} parameter is not a character array");
    }
    let response = upstream
        .request(
            MsgType::GetPar as u8,
            &parameter.index.to_le_bytes(),
            request_timeout,
        )
        .await?;
    if response.message_type != MsgType::GetPar as u8 || response.payload.len() != parameter.size()
    {
        bail!("invalid GetPar response for {name:?}");
    }
    let end = response
        .payload
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(response.payload.len());
    Ok(std::str::from_utf8(&response.payload[..end])?.to_string())
}

async fn query_mac(peer: Ipv4Addr, local: IpAddr, port: u16) -> Result<[u8; 6]> {
    let socket = UdpSocket::bind((local, 0)).await?;
    socket.send_to(&BEACON_REQUEST, (peer, port)).await?;
    let mut response = [0; RESPONSE_LEN];
    let (length, _) =
        timeout(Duration::from_millis(500), socket.recv_from(&mut response)).await??;
    let beacon = BeaconResponse::decode(&response[..length])
        .map_err(|error| anyhow::anyhow!("invalid MCU beacon: {error:?}"))?;
    Ok(beacon.mac)
}

async fn resolve_ipv4(host: &str, port: u16) -> Result<SocketAddrV4> {
    lookup_host((host, port))
        .await?
        .find_map(|address| match address {
            SocketAddr::V4(address) => Some(address),
            SocketAddr::V6(_) => None,
        })
        .with_context(|| format!("{host:?} did not resolve to IPv4"))
}

async fn upstream_udp_loop(
    app: Arc<App>,
    socket: Arc<UdpSocket>,
    peer_ip: Ipv4Addr,
    generation: u64,
) {
    let mut epoch = app.epoch.subscribe();
    let starting_epoch = *epoch.borrow();
    let mut buffer = vec![0; 2048];
    loop {
        tokio::select! {
            received = socket.recv_from(&mut buffer) => {
                match received {
                    Ok((length, peer)) if peer.ip() == IpAddr::V4(peer_ip) => {
                        if let Err(error) = app.process_packet(Arc::from(&buffer[..length])).await {
                            let reason = error.to_string();
                            app.fatal.send_replace(Some(reason));
                            return;
                        }
                    }
                    Ok(_) => {}
                    Err(error) => {
                        app.mark_upstream_failed(generation, &error.to_string()).await;
                        return;
                    }
                }
            }
            changed = epoch.changed() => {
                if changed.is_err() || *epoch.borrow() != starting_epoch {
                    return;
                }
            }
        }
    }
}

async fn heartbeat_loop(app: Arc<App>) {
    let mut ticker = interval(HEARTBEAT_INTERVAL);
    loop {
        ticker.tick().await;
        if app.upstream_ready().await {
            let _ = app.request(MsgType::Status as u8, &[]).await;
        }
    }
}

async fn storage_failure_monitor(app: Arc<App>, mut errors: watch::Receiver<Option<String>>) {
    loop {
        if errors.changed().await.is_err() {
            return;
        }
        let error = errors.borrow().clone();
        if let Some(error) = error {
            app.fatal.send_replace(Some(error));
            return;
        }
    }
}

async fn discard_primers(socket: Arc<UdpSocket>) {
    let mut buffer = [0; 64];
    loop {
        if socket.recv_from(&mut buffer).await.is_err() {
            return;
        }
    }
}

async fn discovery_server(app: Arc<App>, socket: Arc<UdpSocket>) {
    let mut buffer = [0; 64];
    loop {
        let Ok((length, peer)) = socket.recv_from(&mut buffer).await else {
            return;
        };
        if buffer[..length] != BEACON_REQUEST {
            continue;
        }
        let metadata = app.metadata.read().await.clone();
        let Some(metadata) = metadata else { continue };
        let response = BeaconResponse {
            version: helic_proto::VERSION,
            control_port: app.config.control_port,
            mac: metadata.mac,
            experiment: fixed_identity(&metadata.experiment),
            firmware: fixed_identity(concat!("helic-broker ", env!("CARGO_PKG_VERSION"))),
        };
        let mut encoded = [0; RESPONSE_LEN];
        response.encode(&mut encoded);
        let _ = socket.send_to(&encoded, peer).await;
    }
}

async fn serve_client(app: Arc<App>, mut stream: TcpStream, peer: SocketAddr) -> Result<()> {
    stream.set_nodelay(true)?;
    let client_id = app.register_client().await;
    tracing::info!(client_id, %peer, "client connected");
    let mut epoch = app.epoch.subscribe();
    let starting_epoch = *epoch.borrow();
    let result = async {
        loop {
            let request = tokio::select! {
                request = read_frame(&mut stream) => request,
                changed = epoch.changed() => {
                    if changed.is_err() || *epoch.borrow() != starting_epoch {
                        return Ok(());
                    }
                    continue;
                }
            };
            let (message_type, sequence, payload) = request?;
            let handled = handle_request(&app, client_id, peer, message_type, &payload).await?;
            write_frame(
                &mut stream,
                handled.response_type,
                sequence,
                &handled.payload,
            )
            .await?;
            for (datagram, target) in handled.datagrams {
                if let Err(error) = app.downstream_udp.send_to(&datagram, target).await {
                    tracing::warn!(client_id, %target, %error, "historical replay send failed");
                }
            }
        }
    }
    .await;
    app.unregister_client(client_id).await;
    tracing::info!(client_id, %peer, "client disconnected");
    result
}

async fn handle_request(
    app: &Arc<App>,
    client_id: u64,
    peer: SocketAddr,
    message_type: u8,
    payload: &[u8],
) -> Result<Handled> {
    match message_type {
        broker::BROKER_INFO => {
            if !payload.is_empty() {
                return Ok(Handled::error(message_type, ErrorCode::BadLength as u8));
            }
            let info = app.broker_info(client_id).await;
            let mut encoded = vec![0; broker::INFO_HEADER_LEN + info.n_sources as usize];
            info.encode(&mut encoded)
                .map_err(|error| anyhow::anyhow!("could not encode BrokerInfo: {error:?}"))?;
            Ok(Handled {
                response_type: message_type,
                payload: encoded,
                datagrams: Vec::new(),
            })
        }
        broker::QUIET_STREAM_START => start_client(app, client_id, peer, payload, true).await,
        broker::SET_CLIENT_QUIET => set_quiet(app, client_id, message_type, payload).await,
        broker::GET_RECENT => get_recent(app, client_id, message_type, payload).await,
        value if value == MsgType::StreamSetup as u8 => stream_setup(app, payload).await,
        value if value == MsgType::StreamStart as u8 => {
            start_client(app, client_id, peer, payload, false).await
        }
        value if value == MsgType::StreamStop as u8 => stream_stop(app, payload).await,
        value if value == MsgType::SetBlock as u8 || value == MsgType::Commit as u8 => {
            table_request(app, client_id, message_type, payload).await
        }
        _ => Ok(Handled::response(app.request(message_type, payload).await?)),
    }
}

async fn stream_setup(app: &App, payload: &[u8]) -> Result<Handled> {
    let _operation = app.stream_operations.lock().await;
    if app.state.lock().await.running {
        return Ok(Handled::error(
            MsgType::StreamSetup as u8,
            ErrorCode::Busy as u8,
        ));
    }
    let configuration = match StreamConfiguration::decode(payload) {
        Ok(configuration) => configuration,
        Err(_) => {
            return Ok(Handled::error(
                MsgType::StreamSetup as u8,
                ErrorCode::BadValue as u8,
            ));
        }
    };
    let response = app.request(MsgType::StreamSetup as u8, payload).await?;
    if response.message_type == MsgType::StreamSetup as u8 {
        app.state.lock().await.configuration = Some(configuration);
    }
    Ok(Handled::response(response))
}

async fn start_client(
    app: &App,
    client_id: u64,
    peer: SocketAddr,
    payload: &[u8],
    quiet: bool,
) -> Result<Handled> {
    let request_type = if quiet {
        broker::QUIET_STREAM_START
    } else {
        MsgType::StreamStart as u8
    };
    if payload.len() != 2 {
        return Ok(Handled::error(request_type, ErrorCode::BadLength as u8));
    }
    let port = u16::from_le_bytes([payload[0], payload[1]]);
    if port == 0 {
        return Ok(Handled::error(request_type, ErrorCode::BadValue as u8));
    }
    let target = SocketAddr::new(peer.ip(), port);
    let _operation = app.stream_operations.lock().await;
    let running = app.state.lock().await.running;
    if running {
        let mut state = app.state.lock().await;
        let client = state
            .clients
            .get_mut(&client_id)
            .context("client state disappeared")?;
        client.endpoint = Some(target);
        client.quiet = quiet;
        return Ok(Handled::empty(request_type));
    }

    let Some(configuration) = app.state.lock().await.configuration.clone() else {
        return Ok(Handled::error(request_type, ErrorCode::BadValue as u8));
    };
    let metadata = app
        .metadata
        .read()
        .await
        .clone()
        .context("MCU metadata unavailable")?;
    if configuration
        .sources
        .iter()
        .any(|source| *source as usize >= metadata.sources.len())
    {
        return Ok(Handled::error(request_type, ErrorCode::BadValue as u8));
    }
    let selected_sources = configuration
        .sources
        .iter()
        .map(|source| metadata.sources[*source as usize].clone())
        .collect();
    app.storage
        .start(SessionMetadata {
            experiment: metadata.experiment,
            firmware: metadata.firmware,
            sample_rate_hz: metadata.sample_rate_hz,
            decimation: configuration.decimation,
            configured_count: configuration.count,
            sources: selected_sources,
            started_utc_ns: utc_now_ns(),
        })
        .await?;
    {
        let capacity = ((metadata.sample_rate_hz as f64 * app.config.history.as_secs_f64()
            / configuration.decimation as f64)
            .ceil() as usize)
            .max(1);
        let mut state = app.state.lock().await;
        state.running = true;
        state.received_records = 0;
        state.history.reset(capacity);
        let client = state
            .clients
            .get_mut(&client_id)
            .context("client state disappeared")?;
        client.endpoint = Some(target);
        client.quiet = quiet;
    }
    let (udp, peer_ip, _) = app.current_udp().await.context("MCU is not connected")?;
    udp.send_to(PRIMER, (peer_ip, app.config.mcu_stream_port))
        .await?;
    let upstream_port = udp.local_addr()?.port();
    let response = app
        .request(MsgType::StreamStart as u8, &upstream_port.to_le_bytes())
        .await?;
    if response.message_type != MsgType::StreamStart as u8 {
        let mut state = app.state.lock().await;
        state.running = false;
        state.history.clear();
        state.detach_all();
        drop(state);
        let _ = app.storage.stop(CloseReason::UpstreamLost).await;
        return Ok(Handled {
            response_type: response.message_type,
            payload: response.payload,
            datagrams: Vec::new(),
        });
    }
    Ok(Handled::empty(request_type))
}

async fn stream_stop(app: &App, payload: &[u8]) -> Result<Handled> {
    if !payload.is_empty() {
        return Ok(Handled::error(
            MsgType::StreamStop as u8,
            ErrorCode::BadLength as u8,
        ));
    }
    let _operation = app.stream_operations.lock().await;
    let response = app.request(MsgType::StreamStop as u8, &[]).await?;
    if response.message_type == MsgType::StreamStop as u8 {
        let was_running = {
            let mut state = app.state.lock().await;
            let was_running = state.running;
            state.running = false;
            state.received_records = 0;
            state.history.clear();
            state.detach_all();
            was_running
        };
        if was_running {
            app.storage.stop(CloseReason::StreamStop).await?;
        }
    }
    Ok(Handled::response(response))
}

async fn set_quiet(app: &App, client_id: u64, message_type: u8, payload: &[u8]) -> Result<Handled> {
    if payload.len() != 1 {
        return Ok(Handled::error(message_type, ErrorCode::BadLength as u8));
    }
    if payload[0] > 1 {
        return Ok(Handled::error(message_type, ErrorCode::BadValue as u8));
    }
    let mut state = app.state.lock().await;
    let client = state
        .clients
        .get_mut(&client_id)
        .context("client state disappeared")?;
    if client.endpoint.is_none() {
        return Ok(Handled::error(message_type, BrokerError::NotAttached as u8));
    }
    client.quiet = payload[0] != 0;
    Ok(Handled::empty(message_type))
}

async fn get_recent(
    app: &App,
    client_id: u64,
    message_type: u8,
    payload: &[u8],
) -> Result<Handled> {
    if payload.len() != 4 {
        return Ok(Handled::error(message_type, ErrorCode::BadLength as u8));
    }
    let requested = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
    if requested == 0 {
        return Ok(Handled::error(message_type, ErrorCode::BadValue as u8));
    }
    let (endpoint, packets) = {
        let state = app.state.lock().await;
        if !state.running {
            return Ok(Handled::error(
                message_type,
                BrokerError::NoActiveStream as u8,
            ));
        }
        let client = state
            .clients
            .get(&client_id)
            .context("client state disappeared")?;
        let Some(endpoint) = client.endpoint else {
            return Ok(Handled::error(message_type, BrokerError::NotAttached as u8));
        };
        if !client.quiet {
            return Ok(Handled::error(message_type, BrokerError::NotQuiet as u8));
        }
        let Some(packets) = state.history.recent(requested as usize) else {
            return Ok(Handled::error(
                message_type,
                BrokerError::InsufficientHistory as u8,
            ));
        };
        (endpoint, packets)
    };
    Ok(Handled {
        response_type: message_type,
        payload: requested.to_le_bytes().to_vec(),
        datagrams: packets
            .into_iter()
            .map(|packet| (packet, endpoint))
            .collect(),
    })
}

async fn table_request(
    app: &App,
    client_id: u64,
    message_type: u8,
    payload: &[u8],
) -> Result<Handled> {
    {
        let mut state = app.state.lock().await;
        state.expire_table_owner();
        if state
            .table_owner
            .as_ref()
            .is_some_and(|owner| owner.client_id != client_id)
        {
            return Ok(Handled::error(message_type, ErrorCode::Busy as u8));
        }
    }
    let response = app.request(message_type, payload).await?;
    let mut state = app.state.lock().await;
    if message_type == MsgType::SetBlock as u8 && response.message_type == message_type {
        state.table_owner = Some(TableOwner {
            client_id,
            touched: Instant::now(),
        });
    } else if message_type == MsgType::Commit as u8 {
        let is_busy = response.message_type == MsgType::Error as u8
            && response.payload.first() == Some(&(ErrorCode::Busy as u8));
        if is_busy {
            if let Some(owner) = state.table_owner.as_mut() {
                owner.touched = Instant::now();
            }
        } else {
            state.table_owner = None;
        }
    }
    Ok(Handled::response(response))
}

async fn read_frame(stream: &mut TcpStream) -> Result<(u8, u8, Vec<u8>)> {
    let mut header = [0; HEADER_LEN];
    stream.read_exact(&mut header).await?;
    if header[0..2] != MAGIC.to_le_bytes() {
        bail!("bad control-frame magic");
    }
    let length = u16::from_le_bytes([header[4], header[5]]) as usize;
    if length > MAX_PAYLOAD {
        bail!("oversized control frame");
    }
    let mut frame_bytes = Vec::with_capacity(HEADER_LEN + length + TRAILER_LEN);
    frame_bytes.extend_from_slice(&header);
    frame_bytes.resize(HEADER_LEN + length + TRAILER_LEN, 0);
    stream.read_exact(&mut frame_bytes[HEADER_LEN..]).await?;
    let (message_type, sequence, payload) = frame::decode(&frame_bytes)
        .map_err(|error| anyhow::anyhow!("bad control frame: {error:?}"))?;
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
        .map_err(|error| anyhow::anyhow!("could not encode response: {error:?}"))?;
    stream.write_all(&encoded).await?;
    Ok(())
}

fn decode_nul_ascii(payload: &[u8], offset: usize) -> Result<(String, usize)> {
    let end = payload[offset..]
        .iter()
        .position(|byte| *byte == 0)
        .map(|relative| offset + relative)
        .context("unterminated discovery string")?;
    Ok((
        std::str::from_utf8(&payload[offset..end])?.to_string(),
        end + 1,
    ))
}

fn fixed_identity(value: &str) -> [u8; 16] {
    let mut result = [0; 16];
    let bytes = value.as_bytes();
    let length = bytes.len().min(result.len());
    result[..length].copy_from_slice(&bytes[..length]);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_configuration_is_strict() {
        let payload = [4, 0, 0, 0, 0, 0, 2, 0, 12];
        let decoded = StreamConfiguration::decode(&payload).unwrap();
        assert_eq!(decoded.decimation, 4);
        assert_eq!(decoded.sources, vec![0, 12]);
        assert!(StreamConfiguration::decode(&payload[..8]).is_err());
    }

    #[test]
    fn identities_are_fixed_width() {
        assert_eq!(&fixed_identity("cbc-rig")[..8], b"cbc-rig\0");
        assert_eq!(
            fixed_identity("0123456789abcdefghijkl"),
            *b"0123456789abcdef"
        );
    }
}
