//! Segmented, append-only HDF5 storage driven by a bounded worker queue.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use helic_proto::stream::STREAM_HEADER_LEN;
use rust_hdf5::swmr::SwmrFileWriter;
use rust_hdf5::H5File;
use time::{OffsetDateTime, UtcOffset};
use tokio::sync::{mpsc, oneshot, watch};
use uuid::Uuid;

use crate::history::Packet;

const RECORD_CHUNK: u64 = 256;
const PACKET_CHUNK: u64 = 64;
const QUEUE_CAPACITY: usize = 256;

#[derive(Clone, Debug)]
pub struct SourceMetadata {
    pub name: String,
    pub unit: String,
}

#[derive(Clone, Debug)]
pub struct SessionMetadata {
    pub experiment: String,
    pub firmware: String,
    pub sample_rate_hz: f32,
    pub decimation: u16,
    pub configured_count: u32,
    pub sources: Vec<SourceMetadata>,
    pub started_utc_ns: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CloseReason {
    SegmentLimit = 1,
    StreamStop = 2,
    CountComplete = 3,
    UpstreamLost = 4,
    /// Reserved for readers: a writer failure leaves the `.partial` file
    /// unfinalised instead of writing this value.
    StorageError = 5,
    BrokerShutdown = 6,
    StartRejected = 7,
}

impl CloseReason {
    fn completes_session(self) -> bool {
        matches!(
            self,
            Self::StreamStop | Self::CountComplete | Self::BrokerShutdown
        )
    }
}

#[derive(Debug)]
enum Command {
    Start(SessionMetadata, oneshot::Sender<Result<(), String>>),
    Packet(Packet),
    Stop(CloseReason, oneshot::Sender<Result<(), String>>),
    Shutdown,
}

#[derive(Clone, Debug)]
pub struct StorageHandle {
    tx: mpsc::Sender<Command>,
    errors: watch::Receiver<Option<String>>,
}

impl StorageHandle {
    pub fn spawn(output_dir: PathBuf, segment_size: u64) -> Self {
        let (tx, mut rx) = mpsc::channel(QUEUE_CAPACITY);
        let (error_tx, errors) = watch::channel(None);
        tokio::task::spawn_blocking(move || {
            let mut writer = SegmentSession::new(output_dir, segment_size);
            while let Some(command) = rx.blocking_recv() {
                let (result, acknowledgement) = match command {
                    Command::Start(metadata, acknowledgement) => {
                        (writer.start(metadata), Some(acknowledgement))
                    }
                    Command::Packet(packet) => (writer.append(&packet), None),
                    Command::Stop(reason, acknowledgement) => {
                        (writer.stop(reason), Some(acknowledgement))
                    }
                    Command::Shutdown => {
                        let _ = writer.stop(CloseReason::BrokerShutdown);
                        break;
                    }
                };
                let result = result.map_err(|error| error.to_string());
                if let Some(acknowledgement) = acknowledgement {
                    let _ = acknowledgement.send(result.clone());
                }
                if let Err(error) = result {
                    tracing::error!(%error, "HDF5 storage failed");
                    writer.abandon();
                    error_tx.send_replace(Some(error));
                    break;
                }
            }
        });
        Self { tx, errors }
    }

    pub async fn start(&self, metadata: SessionMetadata) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Command::Start(metadata, tx))
            .await
            .map_err(|_| anyhow::anyhow!("storage worker stopped"))?;
        rx.await
            .map_err(|_| anyhow::anyhow!("storage worker stopped"))?
            .map_err(anyhow::Error::msg)
    }

    pub fn packet(&self, packet: Packet) -> Result<()> {
        self.tx
            .try_send(Command::Packet(packet))
            .map_err(|error| anyhow::anyhow!("storage queue cannot accept stream data: {error}"))
    }

    pub async fn stop(&self, reason: CloseReason) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Command::Stop(reason, tx))
            .await
            .map_err(|_| anyhow::anyhow!("storage worker stopped"))?;
        rx.await
            .map_err(|_| anyhow::anyhow!("storage worker stopped"))?
            .map_err(anyhow::Error::msg)
    }

    pub async fn shutdown(&self) {
        let _ = self.tx.send(Command::Shutdown).await;
    }

    pub fn subscribe_errors(&self) -> watch::Receiver<Option<String>> {
        self.errors.clone()
    }
}

struct OpenSegment {
    writer: SwmrFileWriter,
    partial_path: PathBuf,
    final_path: PathBuf,
    values: usize,
    sample_index: usize,
    packet_sequence: usize,
    packet_first_index: usize,
    packet_dropped: usize,
    packet_record_offset: usize,
    packet_record_count: usize,
    packet_received_utc_ns: usize,
    last_flush: Instant,
}

struct SegmentSession {
    output_dir: PathBuf,
    segment_size: u64,
    session_id: Option<Uuid>,
    metadata: Option<SessionMetadata>,
    prefix: String,
    segment_index: u32,
    session_record_offset: u64,
    segment: Option<OpenSegment>,
}

impl SegmentSession {
    fn new(output_dir: PathBuf, segment_size: u64) -> Self {
        Self {
            output_dir,
            segment_size,
            session_id: None,
            metadata: None,
            prefix: String::new(),
            segment_index: 0,
            session_record_offset: 0,
            segment: None,
        }
    }

    fn start(&mut self, metadata: SessionMetadata) -> Result<()> {
        if self.segment.is_some() {
            bail!("storage session is already active");
        }
        if metadata.sources.is_empty() {
            bail!("cannot store a stream with no sources");
        }
        let timestamp = OffsetDateTime::from_unix_timestamp_nanos(metadata.started_utc_ns as i128)
            .unwrap_or(OffsetDateTime::UNIX_EPOCH)
            .to_offset(UtcOffset::UTC)
            .format(&time::macros::format_description!(
                "[year][month][day]T[hour][minute][second].[subsecond digits:3]Z"
            ))?;
        self.prefix = format!("{timestamp}_{}", sanitise(&metadata.experiment));
        self.session_id = Some(Uuid::new_v4());
        self.metadata = Some(metadata);
        self.segment_index = 0;
        self.session_record_offset = 0;
        self.open_segment()
    }

    fn open_segment(&mut self) -> Result<()> {
        let metadata = self
            .metadata
            .as_ref()
            .context("no active storage metadata")?;
        let base = format!("{}_{:04}.h5", self.prefix, self.segment_index);
        let final_path = self.output_dir.join(&base);
        let partial_path = self.output_dir.join(format!("{base}.partial"));
        if partial_path.exists() || final_path.exists() {
            bail!("capture segment already exists: {}", final_path.display());
        }

        let mut writer = SwmrFileWriter::create(&partial_path)?;
        writer.create_group("/", "sources")?;
        writer.create_group("/", "records")?;
        writer.create_group("/", "packets")?;

        writer.set_group_attr_string("/", "format", "HELIC-DAQ broker capture")?;
        writer.set_group_attr_numeric("/", "format_version", &1u32)?;
        writer.set_group_attr_string(
            "/",
            "session_id",
            &self.session_id.context("no session id")?.to_string(),
        )?;
        writer.set_group_attr_numeric("/", "segment_index", &self.segment_index)?;
        writer.set_group_attr_numeric("/", "session_record_offset", &self.session_record_offset)?;
        writer.set_group_attr_string("/", "experiment", &metadata.experiment)?;
        writer.set_group_attr_string("/", "firmware", &metadata.firmware)?;
        writer.set_group_attr_string(
            "/",
            "broker_firmware",
            concat!("helic-broker ", env!("CARGO_PKG_VERSION")),
        )?;
        writer.set_group_attr_numeric("/", "stream_protocol_version", &helic_proto::VERSION)?;
        writer.set_group_attr_numeric("/", "sample_rate_hz", &metadata.sample_rate_hz)?;
        writer.set_group_attr_numeric("/", "decimation", &metadata.decimation)?;
        writer.set_group_attr_numeric("/", "configured_count", &metadata.configured_count)?;
        writer.set_group_attr_numeric("/", "started_utc_ns", &metadata.started_utc_ns)?;

        let names: Vec<&str> = metadata
            .sources
            .iter()
            .map(|source| source.name.as_str())
            .collect();
        let units: Vec<&str> = metadata
            .sources
            .iter()
            .map(|source| source.unit.as_str())
            .collect();
        let names_ds = writer.write_string_dataset("names", &names)?;
        writer.assign_dataset_to_group("/sources", names_ds)?;
        let units_ds = writer.write_string_dataset("units", &units)?;
        writer.assign_dataset_to_group("/sources", units_ds)?;

        let values = writer.create_streaming_dataset_chunked::<f32>(
            "values",
            &[metadata.sources.len() as u64],
            &[RECORD_CHUNK, metadata.sources.len() as u64],
        )?;
        writer.assign_dataset_to_group("/records", values)?;
        let sample_index =
            scalar_stream::<u32>(&mut writer, "sample_index", "/records", RECORD_CHUNK)?;
        let packet_sequence =
            scalar_stream::<u32>(&mut writer, "sequence", "/packets", PACKET_CHUNK)?;
        let packet_first_index =
            scalar_stream::<u32>(&mut writer, "first_index", "/packets", PACKET_CHUNK)?;
        let packet_dropped =
            scalar_stream::<u32>(&mut writer, "dropped", "/packets", PACKET_CHUNK)?;
        let packet_record_offset =
            scalar_stream::<u64>(&mut writer, "record_offset", "/packets", PACKET_CHUNK)?;
        let packet_record_count =
            scalar_stream::<u16>(&mut writer, "record_count", "/packets", PACKET_CHUNK)?;
        let packet_received_utc_ns =
            scalar_stream::<i64>(&mut writer, "received_utc_ns", "/packets", PACKET_CHUNK)?;

        writer.write_dataset::<i64>("ended_utc_ns", &[], &[0])?;
        writer.write_dataset::<u8>("close_reason", &[], &[0])?;
        writer.write_dataset::<u8>("clean_close", &[], &[0])?;
        writer.write_dataset::<u8>("session_complete", &[], &[0])?;
        writer.start_swmr()?;

        self.segment = Some(OpenSegment {
            writer,
            partial_path,
            final_path,
            values,
            sample_index,
            packet_sequence,
            packet_first_index,
            packet_dropped,
            packet_record_offset,
            packet_record_count,
            packet_received_utc_ns,
            last_flush: Instant::now(),
        });
        Ok(())
    }

    fn append(&mut self, packet: &Packet) -> Result<()> {
        let metadata = self
            .metadata
            .as_ref()
            .context("stream packet arrived without storage session")?;
        let segment = self
            .segment
            .as_mut()
            .context("stream packet arrived without open segment")?;
        if packet.header.n_sources as usize != metadata.sources.len() {
            bail!("stream source count changed inside a storage session");
        }
        let row_bytes = 4 * metadata.sources.len();
        for row in 0..packet.header.n_records as usize {
            let start = STREAM_HEADER_LEN + row * row_bytes;
            segment
                .writer
                .append_frame(segment.values, &packet.wire[start..start + row_bytes])?;
            let index = packet
                .header
                .first_index
                .wrapping_add(row as u32 * packet.header.decimation as u32);
            segment
                .writer
                .append_frame(segment.sample_index, &index.to_le_bytes())?;
        }
        segment
            .writer
            .append_frame(segment.packet_sequence, &packet.header.seq.to_le_bytes())?;
        segment.writer.append_frame(
            segment.packet_first_index,
            &packet.header.first_index.to_le_bytes(),
        )?;
        segment
            .writer
            .append_frame(segment.packet_dropped, &packet.header.dropped.to_le_bytes())?;
        segment.writer.append_frame(
            segment.packet_record_offset,
            &self.session_record_offset.to_le_bytes(),
        )?;
        segment.writer.append_frame(
            segment.packet_record_count,
            &packet.header.n_records.to_le_bytes(),
        )?;
        segment.writer.append_frame(
            segment.packet_received_utc_ns,
            &packet.received_utc_ns.to_le_bytes(),
        )?;
        self.session_record_offset += packet.header.n_records as u64;

        if segment.last_flush.elapsed() >= Duration::from_secs(1) {
            segment.writer.flush()?;
            segment.last_flush = Instant::now();
            if fs::metadata(&segment.partial_path)?.len() >= self.segment_size {
                self.rotate()?;
            }
        }
        Ok(())
    }

    fn rotate(&mut self) -> Result<()> {
        self.close_segment(CloseReason::SegmentLimit, false)?;
        self.segment_index += 1;
        self.open_segment()
    }

    fn stop(&mut self, reason: CloseReason) -> Result<()> {
        if self.segment.is_none() {
            return Ok(());
        }
        self.close_segment(reason, reason.completes_session())?;
        self.metadata = None;
        self.session_id = None;
        Ok(())
    }

    fn close_segment(&mut self, reason: CloseReason, session_complete: bool) -> Result<()> {
        let segment = self.segment.take().context("no open segment")?;
        segment.writer.close()?;
        finalise_file(&segment.partial_path, reason, session_complete)?;
        fs::rename(&segment.partial_path, &segment.final_path).with_context(|| {
            format!(
                "could not rename {} to {}",
                segment.partial_path.display(),
                segment.final_path.display()
            )
        })?;
        Ok(())
    }

    fn abandon(&mut self) {
        self.segment = None;
        self.metadata = None;
        self.session_id = None;
    }
}

fn scalar_stream<T: rust_hdf5::H5Type>(
    writer: &mut SwmrFileWriter,
    name: &str,
    group: &str,
    chunk: u64,
) -> Result<usize> {
    let dataset = writer.create_streaming_dataset_chunked::<T>(name, &[1], &[chunk, 1])?;
    writer.assign_dataset_to_group(group, dataset)?;
    Ok(dataset)
}

fn finalise_file(path: &Path, reason: CloseReason, session_complete: bool) -> Result<()> {
    let file = H5File::open_rw(path)?;
    file.dataset_writer("ended_utc_ns")?
        .write_raw(&[utc_now_ns()])?;
    file.dataset_writer("close_reason")?
        .write_raw(&[reason as u8])?;
    file.dataset_writer("clean_close")?.write_raw(&[1u8])?;
    file.dataset_writer("session_complete")?
        .write_raw(&[session_complete as u8])?;
    file.close()?;
    Ok(())
}

pub fn utc_now_ns() -> i64 {
    let nanos = OffsetDateTime::now_utc().unix_timestamp_nanos();
    i64::try_from(nanos).unwrap_or_else(|_| {
        if nanos.is_negative() {
            i64::MIN
        } else {
            i64::MAX
        }
    })
}

fn sanitise(value: &str) -> String {
    let out: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() {
        "device".into()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use helic_proto::stream::StreamHeader;
    use rust_hdf5::swmr::SwmrFileReader;
    use tempfile::tempdir;

    fn metadata() -> SessionMetadata {
        SessionMetadata {
            experiment: "cbc-rig".into(),
            firmware: "helic-daq test".into(),
            sample_rate_hz: 8000.0,
            decimation: 1,
            configured_count: 0,
            sources: vec![
                SourceMetadata {
                    name: "adc0".into(),
                    unit: "V".into(),
                },
                SourceMetadata {
                    name: "out".into(),
                    unit: "V".into(),
                },
            ],
            started_utc_ns: utc_now_ns(),
        }
    }

    fn packet() -> Packet {
        let header = StreamHeader {
            n_sources: 2,
            seq: 7,
            first_index: 100,
            dropped: 3,
            decimation: 1,
            n_records: 2,
        };
        let mut wire = vec![0; STREAM_HEADER_LEN];
        header.encode(&mut wire);
        for value in [1.0f32, 2.0, 3.0, 4.0] {
            wire.extend_from_slice(&value.to_le_bytes());
        }
        Packet::decode(Arc::from(wire), utc_now_ns()).unwrap()
    }

    #[test]
    fn hdf5_session_round_trips_through_pure_rust_reader() {
        let directory = tempdir().unwrap();
        let mut session = SegmentSession::new(directory.path().to_path_buf(), 1 << 30);
        session.start(metadata()).unwrap();
        session.append(&packet()).unwrap();
        session.stop(CloseReason::StreamStop).unwrap();
        let path = fs::read_dir(directory.path())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(
            path.extension().and_then(|value| value.to_str()),
            Some("h5")
        );
        let mut reader = SwmrFileReader::open(&path).unwrap();
        assert_eq!(reader.dataset_shape("records/values").unwrap(), vec![2, 2]);
        assert_eq!(
            reader.read_dataset::<f32>("records/values").unwrap(),
            vec![1.0, 2.0, 3.0, 4.0]
        );
        assert_eq!(
            reader.read_dataset::<u32>("records/sample_index").unwrap(),
            vec![100, 101]
        );
        assert_eq!(
            reader.read_dataset::<u32>("packets/sequence").unwrap(),
            vec![7]
        );
    }

    #[test]
    fn size_threshold_starts_a_linked_segment() {
        let directory = tempdir().unwrap();
        let mut session = SegmentSession::new(directory.path().to_path_buf(), 1);
        session.start(metadata()).unwrap();
        session.segment.as_mut().unwrap().last_flush = Instant::now() - Duration::from_secs(2);
        session.append(&packet()).unwrap();
        assert_eq!(session.segment_index, 1);
        session.stop(CloseReason::StreamStop).unwrap();

        let mut paths = fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        paths.sort();
        assert_eq!(paths.len(), 2);
        let first = H5File::open(&paths[0]).unwrap();
        assert_eq!(
            first
                .dataset("close_reason")
                .unwrap()
                .read_raw::<u8>()
                .unwrap(),
            vec![CloseReason::SegmentLimit as u8]
        );
        assert_eq!(
            first
                .dataset("session_complete")
                .unwrap()
                .read_raw::<u8>()
                .unwrap(),
            vec![0]
        );
    }
}
