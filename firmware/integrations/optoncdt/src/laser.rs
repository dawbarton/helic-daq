//! optoNCDT reader shared by experiments that claim the laser UART.

use core::sync::atomic::{AtomicU32, Ordering};

use defmt::{info, warn};
use embassy_rp::uart::{BufferedUart, BufferedUartRx};
use embassy_time::{Duration, Timer, WithTimeout};
use embedded_io_async::{Read, Write};
use helic_drivers::optoncdt::{
    CommandReply, CommandReplyParser, DistanceScale, ParseEvent, Parser, Reading,
};

const OUTPUT_NONE: &[u8] = b"OUTPUT NONE\n";
const GET_USER_LEVEL: &[u8] = b"GETUSERLEVEL\n";
const NO_OUTPUT_REDUCTION: &[u8] = b"OUTREDUCEDEVICE NONE\n";
const DISTANCE_ONLY: &[u8] = b"OUTADD_RS422 NONE\n";
const OUTPUT_RS422: &[u8] = b"OUTPUT RS422\n";
const REQUIRED_BAUD: u32 = 921_600;
const SYNC_FRAMES: u8 = 8;
const SUPPORTED_BAUDS: [u32; 11] = [
    921_600, 1_000_000, 691_200, 460_800, 256_000, 230_400, 128_000, 115_200, 56_000, 19_200, 9_600,
];

#[derive(Debug, Default)]
struct ReplyTrace {
    bytes_received: u32,
    trailing: [u8; 4],
}

impl ReplyTrace {
    fn push(&mut self, byte: u8) {
        self.bytes_received = self.bytes_received.saturating_add(1);
        self.trailing.rotate_left(1);
        self.trailing[3] = byte;
    }
}

/// Monotonic diagnostics for one optoNCDT receive task.
#[derive(Clone, Copy)]
pub struct LaserCounters {
    frames_received: &'static AtomicU32,
    uart_errors: &'static AtomicU32,
    parse_errors: &'static AtomicU32,
    invalid_frames: &'static AtomicU32,
    unexpected_values: &'static AtomicU32,
    sync_errors: &'static AtomicU32,
}

impl LaserCounters {
    pub const fn new(
        frames_received: &'static AtomicU32,
        uart_errors: &'static AtomicU32,
        parse_errors: &'static AtomicU32,
        invalid_frames: &'static AtomicU32,
        unexpected_values: &'static AtomicU32,
        sync_errors: &'static AtomicU32,
    ) -> Self {
        Self {
            frames_received,
            uart_errors,
            parse_errors,
            invalid_frames,
            unexpected_values,
            sync_errors,
        }
    }
}

/// Configure an optoNCDT command channel, then publish its binary measurements.
///
/// Replies and binary values share the RS422 receive stream. Each command is
/// therefore followed by a bounded wait for the documented `->` prompt; any
/// interleaved measurements are discarded until configuration is complete.
pub async fn configured_laser_run(
    mut uart: BufferedUart,
    measrate_command: &'static [u8],
    range_mm: &'static AtomicU32,
    destination: &'static AtomicU32,
    counters: LaserCounters,
) -> ! {
    loop {
        let Some(baudrate) = detect_baudrate(&mut uart).await else {
            warn!("laser: no reply at any supported baud rate");
            Timer::after_secs(2).await;
            continue;
        };
        if baudrate != REQUIRED_BAUD {
            warn!(
                "laser: sensor replied at {} baud; {} is required for 8 kHz output",
                baudrate, REQUIRED_BAUD
            );
            Timer::after_secs(2).await;
            continue;
        }
        if configure(&mut uart, measrate_command).await {
            break;
        }
        // The sensor can still be booting when the RP2350 task starts. Retry
        // slowly enough to avoid monopolising core 0 when it is disconnected.
        Timer::after_millis(250).await;
    }
    info!("laser: configuration complete; parsing binary measurements");
    let (_, rx) = uart.split();
    laser_run(rx, range_mm, destination, counters).await
}

async fn detect_baudrate(uart: &mut BufferedUart) -> Option<u32> {
    // On an RP2350 restart the sensor may still be streaming. The first byte
    // seen after UART enable can then begin mid-character and report a framing
    // error even at the correct baud. Retry the stop command at the required
    // rate before trying any other baud: once the sensor receives it, the
    // next reply is quiet ASCII and the normal configuration can proceed.
    uart.set_baudrate(REQUIRED_BAUD);
    for attempt in 1..=3 {
        info!(
            "laser: stopping old stream at {} baud (attempt {})",
            REQUIRED_BAUD, attempt
        );
        if uart.write_all(OUTPUT_NONE).await.is_err() {
            continue;
        }

        let mut trace = ReplyTrace::default();
        match wait_for_reply(uart, &mut trace)
            .with_timeout(Duration::from_millis(200))
            .await
        {
            Ok(Ok(CommandReply::Ok | CommandReply::Error(_))) => {
                info!(
                    "laser: detected {} baud after {} reply bytes; trailing {:?}",
                    REQUIRED_BAUD, trace.bytes_received, trace.trailing
                );
                return Some(REQUIRED_BAUD);
            }
            Ok(Err(error)) => {
                warn!(
                    "laser: stop attempt {} receive error {:?} after {} bytes",
                    attempt, error, trace.bytes_received
                );
            }
            Err(_) => {}
        }
    }

    for baudrate in SUPPORTED_BAUDS {
        uart.set_baudrate(baudrate);
        info!("laser: probing {} baud", baudrate);
        if uart.write_all(GET_USER_LEVEL).await.is_err() {
            continue;
        }

        let mut trace = ReplyTrace::default();
        match wait_for_reply(uart, &mut trace)
            .with_timeout(Duration::from_millis(200))
            .await
        {
            Ok(Ok(CommandReply::Ok)) => {
                info!(
                    "laser: detected {} baud after {} reply bytes; trailing {:?}",
                    baudrate, trace.bytes_received, trace.trailing
                );
                return Some(baudrate);
            }
            Ok(Ok(CommandReply::Error(code))) => {
                warn!(
                    "laser: baud probe at {} returned E{} after {} bytes",
                    baudrate, code, trace.bytes_received
                );
                return Some(baudrate);
            }
            Ok(Err(error)) => {
                warn!(
                    "laser: baud probe at {} receive error {:?} after {} bytes",
                    baudrate, error, trace.bytes_received
                );
            }
            Err(_) => {}
        }
    }
    None
}

async fn configure(uart: &mut BufferedUart, measrate_command: &'static [u8]) -> bool {
    // Stop any existing binary stream first so subsequent ASCII replies are
    // quiet and unambiguous. The reply parser still tolerates values already
    // in flight, as required when the sensor retained an earlier RS422 setup.
    let commands: [(&str, &[u8]); 5] = [
        ("OUTPUT NONE", OUTPUT_NONE),
        ("MEASRATE", measrate_command),
        ("OUTREDUCEDEVICE NONE", NO_OUTPUT_REDUCTION),
        ("OUTADD_RS422 NONE", DISTANCE_ONLY),
        ("OUTPUT RS422", OUTPUT_RS422),
    ];
    for (name, command) in commands {
        info!("laser: sending {}", name);
        if let Err(error) = uart.write_all(command).await {
            warn!("laser: {} transmit error: {:?}", name, error);
            return false;
        }

        let mut trace = ReplyTrace::default();
        match wait_for_reply(uart, &mut trace)
            .with_timeout(Duration::from_millis(500))
            .await
        {
            Ok(Ok(CommandReply::Ok)) => {
                info!(
                    "laser: {} accepted after {} bytes; trailing {:?}",
                    name, trace.bytes_received, trace.trailing
                );
                if name == "OUTPUT NONE" {
                    drain_stopped_stream(uart).await;
                }
            }
            Ok(Ok(CommandReply::Error(code))) => {
                warn!(
                    "laser: {} rejected with E{} after {} bytes; trailing {:?}",
                    name, code, trace.bytes_received, trace.trailing
                );
                return false;
            }
            Ok(Err(error)) => {
                warn!(
                    "laser: {} receive error {:?} after {} bytes; trailing {:?}",
                    name, error, trace.bytes_received, trace.trailing
                );
                return false;
            }
            Err(_) => {
                warn!(
                    "laser: {} reply timeout after {} bytes; trailing {:?}",
                    name, trace.bytes_received, trace.trailing
                );
                return false;
            }
        }
    }
    true
}

async fn drain_stopped_stream(uart: &mut BufferedUart) {
    // Buffered RX deliberately preserves bytes and error flags while core 0
    // is descheduled. Once OUTPUT NONE has stopped the sensor, consume any
    // binary tail and pre-command framing state so measurement counters begin
    // at the clean OUTPUT RS422 boundary.
    let mut buf = [0u8; 32];
    loop {
        match uart
            .read(&mut buf)
            .with_timeout(Duration::from_millis(2))
            .await
        {
            Ok(Ok(_) | Err(_)) => {}
            Err(_) => return,
        }
    }
}

async fn wait_for_reply(
    uart: &mut BufferedUart,
    trace: &mut ReplyTrace,
) -> Result<CommandReply, embassy_rp::uart::Error> {
    let mut parser = CommandReplyParser::new();
    let mut byte = [0u8; 1];
    loop {
        uart.read(&mut byte).await?;
        trace.push(byte[0]);
        if let Some(reply) = parser.push(byte[0]) {
            return Ok(reply);
        }
    }
}

pub async fn laser_run(
    mut rx: BufferedUartRx,
    range_mm: &'static AtomicU32,
    destination: &'static AtomicU32,
    counters: LaserCounters,
) -> ! {
    let mut parser = Parser::new();
    let mut buf = [0u8; 32];
    let mut first_distance_logged = false;
    let mut synchronised = false;
    let mut clean_frames = 0u8;
    loop {
        let received = match rx.read(&mut buf).await {
            Ok(received) => received,
            Err(_) => {
                if synchronised {
                    counters.uart_errors.fetch_add(1, Ordering::Relaxed);
                } else {
                    counters.sync_errors.fetch_add(1, Ordering::Relaxed);
                    clean_frames = 0;
                }
                parser = Parser::new();
                // A floating disconnected line can generate enough
                // framing-error interrupts to starve core 0; retain the
                // hardware pull-up and back off after errors as defence in
                // depth.
                Timer::after_millis(10).await;
                continue;
            }
        };
        for &byte in &buf[..received] {
            let value = match parser.push_event(byte) {
                ParseEvent::Pending => continue,
                ParseEvent::Resynchronised => {
                    if synchronised {
                        counters.parse_errors.fetch_add(1, Ordering::Relaxed);
                    } else {
                        counters.sync_errors.fetch_add(1, Ordering::Relaxed);
                        clean_frames = 0;
                    }
                    continue;
                }
                ParseEvent::Value(value) => value,
            };
            if !value.first {
                if synchronised {
                    counters.unexpected_values.fetch_add(1, Ordering::Relaxed);
                } else {
                    counters.sync_errors.fetch_add(1, Ordering::Relaxed);
                    clean_frames = 0;
                }
                continue;
            }

            if !synchronised {
                clean_frames = clean_frames.saturating_add(1);
                if clean_frames == SYNC_FRAMES {
                    synchronised = true;
                    info!("laser: binary stream synchronised");
                }
            }
            if synchronised {
                counters.frames_received.fetch_add(1, Ordering::Relaxed);
            }
            let scale = DistanceScale::new(f32::from_bits(range_mm.load(Ordering::Relaxed)));
            match scale.convert(value.value) {
                Reading::InRange(mm) => {
                    if !first_distance_logged {
                        info!(
                            "laser: first in-range measurement raw={} distance={} mm",
                            value.value, mm
                        );
                        first_distance_logged = true;
                    }
                    destination.store(mm.to_bits(), Ordering::Relaxed);
                }
                Reading::BelowRange => {
                    if synchronised {
                        counters.invalid_frames.fetch_add(1, Ordering::Relaxed);
                    }
                    if !first_distance_logged {
                        warn!("laser: first distance raw={} is below range", value.value);
                        first_distance_logged = true;
                    }
                }
                Reading::AboveRange => {
                    if synchronised {
                        counters.invalid_frames.fetch_add(1, Ordering::Relaxed);
                    }
                    if !first_distance_logged {
                        warn!("laser: first distance raw={} is above range", value.value);
                        first_distance_logged = true;
                    }
                }
                Reading::Error(code) => {
                    if synchronised {
                        counters.invalid_frames.fetch_add(1, Ordering::Relaxed);
                    }
                    if !first_distance_logged {
                        warn!("laser: first distance is sensor error {}", code);
                        first_distance_logged = true;
                    }
                }
            }
        }
    }
}
