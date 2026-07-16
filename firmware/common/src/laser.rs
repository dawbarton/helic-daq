//! optoNCDT reader shared by experiments that claim the laser UART.

use core::sync::atomic::{AtomicU32, Ordering};

use defmt::{info, warn};
use embassy_rp::uart::{Async, UartRx, UartTx};
use embassy_time::{Duration, Timer, WithTimeout};
use helic_drivers::optoncdt::{CommandReply, CommandReplyParser, DistanceScale, Parser, Reading};

const OUTPUT_NONE: &[u8] = b"OUTPUT NONE\n";
const NO_OUTPUT_REDUCTION: &[u8] = b"OUTREDUCEDEVICE NONE\n";
const DISTANCE_ONLY: &[u8] = b"OUTADD_RS422 NONE\n";
const OUTPUT_RS422: &[u8] = b"OUTPUT RS422\n";

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

/// Configure an optoNCDT command channel, then publish its binary measurements.
///
/// Replies and binary values share the RS422 receive stream. Each command is
/// therefore followed by a bounded wait for the documented `->` prompt; any
/// interleaved measurements are discarded until configuration is complete.
pub async fn configured_laser_run(
    mut tx: UartTx<'static, Async>,
    mut rx: UartRx<'static, Async>,
    measrate_command: &'static [u8],
    range_mm: &'static AtomicU32,
    destination: &'static AtomicU32,
) -> ! {
    loop {
        if configure(&mut tx, &mut rx, measrate_command).await {
            break;
        }
        // The sensor can still be booting when the RP2350 task starts. Retry
        // slowly enough to avoid monopolising core 0 when it is disconnected.
        Timer::after_millis(250).await;
    }
    info!("laser: configuration complete; parsing binary measurements");
    laser_run(rx, range_mm, destination).await
}

async fn configure(
    tx: &mut UartTx<'static, Async>,
    rx: &mut UartRx<'static, Async>,
    measrate_command: &'static [u8],
) -> bool {
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
        if let Err(error) = tx.write(command).await {
            warn!("laser: {} transmit error: {:?}", name, error);
            return false;
        }

        let mut trace = ReplyTrace::default();
        match wait_for_reply(rx, &mut trace)
            .with_timeout(Duration::from_millis(500))
            .await
        {
            Ok(Ok(CommandReply::Ok)) => {
                info!(
                    "laser: {} accepted after {} bytes; trailing {:?}",
                    name, trace.bytes_received, trace.trailing
                );
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

async fn wait_for_reply(
    rx: &mut UartRx<'static, Async>,
    trace: &mut ReplyTrace,
) -> Result<CommandReply, embassy_rp::uart::Error> {
    let mut parser = CommandReplyParser::new();
    let mut byte = [0u8; 1];
    loop {
        rx.read(&mut byte).await?;
        trace.push(byte[0]);
        if let Some(reply) = parser.push(byte[0]) {
            return Ok(reply);
        }
    }
}

pub async fn laser_run(
    mut rx: UartRx<'static, Async>,
    range_mm: &'static AtomicU32,
    destination: &'static AtomicU32,
) -> ! {
    let mut parser = Parser::new();
    let mut buf = [0u8; 3];
    let mut first_distance_logged = false;
    loop {
        if rx.read(&mut buf).await.is_err() {
            // A floating disconnected line can generate enough framing-error
            // interrupts to starve core 0; retain the hardware pull-up and
            // back off after errors as defence in depth.
            Timer::after_millis(10).await;
            continue;
        }
        for byte in buf {
            let Some(value) = parser.push(byte) else {
                continue;
            };
            if value.first {
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
                    Reading::BelowRange if !first_distance_logged => {
                        warn!("laser: first distance raw={} is below range", value.value);
                        first_distance_logged = true;
                    }
                    Reading::AboveRange if !first_distance_logged => {
                        warn!("laser: first distance raw={} is above range", value.value);
                        first_distance_logged = true;
                    }
                    Reading::Error(code) if !first_distance_logged => {
                        warn!("laser: first distance is sensor error {}", code);
                        first_distance_logged = true;
                    }
                    Reading::BelowRange | Reading::AboveRange | Reading::Error(_) => {}
                }
            }
        }
    }
}
