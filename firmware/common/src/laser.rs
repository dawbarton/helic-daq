//! optoNCDT reader shared by experiments that claim the laser UART.

use core::sync::atomic::{AtomicU32, Ordering};

use embassy_rp::uart::{Async, UartRx, UartTx};
use embassy_time::{Duration, Timer, WithTimeout};
use helic_drivers::optoncdt::{CommandReply, CommandReplyParser, DistanceScale, Parser, Reading};

const OUTPUT_NONE: &[u8] = b"OUTPUT NONE\n";
const NO_OUTPUT_REDUCTION: &[u8] = b"OUTREDUCEDEVICE NONE\n";
const DISTANCE_ONLY: &[u8] = b"OUTADD_RS422 NONE\n";
const OUTPUT_RS422: &[u8] = b"OUTPUT RS422\n";

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
    let commands: [&[u8]; 5] = [
        OUTPUT_NONE,
        measrate_command,
        NO_OUTPUT_REDUCTION,
        DISTANCE_ONLY,
        OUTPUT_RS422,
    ];
    for command in commands {
        if tx.write(command).await.is_err() {
            return false;
        }
        match wait_for_reply(rx)
            .with_timeout(Duration::from_millis(500))
            .await
        {
            Ok(Ok(CommandReply::Ok)) => {}
            Ok(Ok(CommandReply::Error(_))) | Ok(Err(())) | Err(_) => return false,
        }
    }
    true
}

async fn wait_for_reply(rx: &mut UartRx<'static, Async>) -> Result<CommandReply, ()> {
    let mut parser = CommandReplyParser::new();
    let mut byte = [0u8; 1];
    loop {
        rx.read(&mut byte).await.map_err(|_| ())?;
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
                if let Reading::InRange(mm) = scale.convert(value.value) {
                    destination.store(mm.to_bits(), Ordering::Relaxed);
                }
            }
        }
    }
}
