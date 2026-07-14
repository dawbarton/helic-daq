//! optoNCDT reader shared by experiments that claim the laser UART.

use core::sync::atomic::{AtomicU32, Ordering};

use embassy_rp::uart::{Async, UartRx};
use embassy_time::Timer;
use helic_drivers::optoncdt::{DistanceScale, Parser, Reading};

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
