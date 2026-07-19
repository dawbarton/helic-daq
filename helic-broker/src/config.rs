//! Command-line configuration and validated byte/duration parsers.

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::Parser;

#[derive(Clone, Debug, Parser)]
#[command(
    name = "helic-broker",
    about = "Shared HELIC-DAQ stream broker with optional HDF5 recording"
)]
pub struct Config {
    /// MCU hostname or IPv4 address.
    #[arg(long)]
    pub mcu_host: String,

    /// Optional directory receiving timestamped HDF5 stream segments.
    #[arg(long)]
    pub output_dir: Option<PathBuf>,

    #[arg(long, default_value_t = helic_proto::CONTROL_PORT)]
    pub mcu_control_port: u16,

    #[arg(long, default_value_t = helic_proto::STREAM_PORT)]
    pub mcu_stream_port: u16,

    #[arg(long, default_value_t = helic_proto::DISCOVERY_PORT)]
    pub mcu_discovery_port: u16,

    /// Loopback TCP control port exposed to clients.
    #[arg(long, default_value_t = helic_proto::CONTROL_PORT)]
    pub control_port: u16,

    /// Loopback UDP stream port exposed to clients.
    #[arg(long, default_value_t = helic_proto::STREAM_PORT)]
    pub stream_port: u16,

    /// Loopback UDP discovery port exposed to clients.
    #[arg(long, default_value_t = helic_proto::DISCOVERY_PORT)]
    pub discovery_port: u16,

    /// Shared recent-history retention.
    #[arg(long, default_value = "10s", value_parser = parse_duration)]
    pub history: Duration,

    /// Soft HDF5 segment size threshold.
    #[arg(long, default_value = "1GiB", value_parser = parse_size)]
    pub segment_size: u64,

    #[arg(long, default_value = "5s", value_parser = parse_duration)]
    pub request_timeout: Duration,

    #[arg(long, default_value = "1s", value_parser = parse_duration)]
    pub reconnect_delay: Duration,

    /// Logging filter, using tracing-subscriber syntax.
    #[arg(long, default_value = "info")]
    pub log_level: String,
}

impl Config {
    pub const LOOPBACK: Ipv4Addr = Ipv4Addr::LOCALHOST;

    pub fn validate(&self) -> Result<()> {
        if self.history.is_zero() {
            bail!("--history must be positive");
        }
        if self.segment_size < 1024 * 1024 {
            bail!("--segment-size must be at least 1MiB");
        }
        if self.request_timeout.is_zero() || self.reconnect_delay.is_zero() {
            bail!("timeouts must be positive");
        }
        if let Some(output_dir) = &self.output_dir {
            std::fs::create_dir_all(output_dir).with_context(|| {
                format!("could not create output directory {}", output_dir.display())
            })?;
        }
        Ok(())
    }

    pub fn recording_notice(&self) -> String {
        match &self.output_dir {
            Some(output_dir) => {
                format!("Captures are being saved to {}.", output_dir.display())
            }
            None => "Captures are not being saved.".into(),
        }
    }
}

fn parse_duration(value: &str) -> Result<Duration, String> {
    let (number, scale) = if let Some(number) = value.strip_suffix("ms") {
        (number, 0.001)
    } else if let Some(number) = value.strip_suffix('s') {
        (number, 1.0)
    } else if let Some(number) = value.strip_suffix('m') {
        (number, 60.0)
    } else if let Some(number) = value.strip_suffix('h') {
        (number, 3600.0)
    } else {
        return Err("duration must end in ms, s, m, or h".into());
    };
    let number: f64 = number
        .parse()
        .map_err(|_| format!("invalid duration {value:?}"))?;
    if !number.is_finite() || number <= 0.0 {
        return Err("duration must be finite and positive".into());
    }
    Ok(Duration::from_secs_f64(number * scale))
}

fn parse_size(value: &str) -> Result<u64, String> {
    const UNITS: [(&str, u64); 6] = [
        ("GiB", 1024 * 1024 * 1024),
        ("MiB", 1024 * 1024),
        ("KiB", 1024),
        ("GB", 1000 * 1000 * 1000),
        ("MB", 1000 * 1000),
        ("KB", 1000),
    ];
    for (suffix, multiplier) in UNITS {
        if let Some(number) = value.strip_suffix(suffix) {
            let parsed: u64 = number
                .parse()
                .map_err(|_| format!("invalid size {value:?}"))?;
            return parsed
                .checked_mul(multiplier)
                .ok_or_else(|| "size is too large".into());
        }
    }
    value
        .parse()
        .map_err(|_| "size must be bytes or end in KiB, MiB, GiB, KB, MB, or GB".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn duration_and_size_parsers_are_explicit() {
        assert_eq!(parse_duration("10s").unwrap(), Duration::from_secs(10));
        assert_eq!(parse_duration("250ms").unwrap(), Duration::from_millis(250));
        assert!(parse_duration("10").is_err());
        assert_eq!(parse_size("1GiB").unwrap(), 1 << 30);
        assert_eq!(parse_size("2MB").unwrap(), 2_000_000);
    }

    #[test]
    fn recording_is_optional_and_reported_clearly() {
        let without = Config::try_parse_from(["helic-broker", "--mcu-host", "device"]).unwrap();
        assert!(without.output_dir.is_none());
        assert_eq!(without.recording_notice(), "Captures are not being saved.");

        let parent = tempdir().unwrap();
        let output = parent.path().join("captures");
        let with = Config::try_parse_from([
            "helic-broker",
            "--mcu-host",
            "device",
            "--output-dir",
            output.to_str().unwrap(),
        ])
        .unwrap();
        with.validate().unwrap();
        assert!(output.is_dir());
        assert_eq!(
            with.recording_notice(),
            format!("Captures are being saved to {}.", output.display())
        );
    }
}
