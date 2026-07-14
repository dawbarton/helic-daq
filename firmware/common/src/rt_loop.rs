//! Cross-core real-time mailboxes, records and diagnostics.

use core::sync::atomic::{AtomicU32, Ordering};

use defmt::info;
use embassy_time::{Duration, Ticker};
use heapless::spsc::{Consumer, Producer};
use helic_core::generator::FourierCoeffs;

use crate::HARMONICS;

/// Commands applied by the real-time loop at sample boundaries.
#[derive(Clone, Copy, Debug)]
pub enum RtCommand {
    SetIncrement(u32),
    SetTargetCoeffs(FourierCoeffs<HARMONICS>),
    SetForcingCoeffs(FourierCoeffs<HARMONICS>),
    ResetController,
    SetCtrlParam(u16, f32),
}

pub const COMMAND_QUEUE_LEN: usize = 32;
pub type CommandProducer = Producer<'static, RtCommand>;
pub type CommandConsumer = Consumer<'static, RtCommand>;

#[derive(Clone, Copy, Debug, Default)]
pub struct Record {
    pub index: u32,
    pub adc: [f32; 8],
    pub laser: f32,
    pub target: f32,
    pub forcing: f32,
    pub out: f32,
}

pub const RECORD_QUEUE_LEN: usize = 256;
pub type RecordProducer = Producer<'static, Record>;
pub type RecordConsumer = Consumer<'static, Record>;

pub static LOOP_TIME_LAST_US: AtomicU32 = AtomicU32::new(0);
pub static LOOP_TIME_MAX_US: AtomicU32 = AtomicU32::new(0);
pub static OVERRUNS: AtomicU32 = AtomicU32::new(0);
pub static CLOCK_JITTER_US: AtomicU32 = AtomicU32::new(0);
pub static BUSY_TIMEOUTS: AtomicU32 = AtomicU32::new(0);
pub static RECORDS_DROPPED: AtomicU32 = AtomicU32::new(0);
pub static TICKS: AtomicU32 = AtomicU32::new(0);

/// Latest laser reading as `f32::to_bits()`, published by core 0.
pub static LASER_VALUE: AtomicU32 = AtomicU32::new(0);

pub async fn status_run() -> ! {
    let mut ticker = Ticker::every(Duration::from_secs(1));
    loop {
        ticker.next().await;
        info!(
            "ticks {} | loop {}/{} us | jitter {} us | overruns {} | busy timeouts {} | dropped {} | laser {} mm",
            TICKS.load(Ordering::Relaxed),
            LOOP_TIME_LAST_US.load(Ordering::Relaxed),
            LOOP_TIME_MAX_US.load(Ordering::Relaxed),
            CLOCK_JITTER_US.load(Ordering::Relaxed),
            OVERRUNS.load(Ordering::Relaxed),
            BUSY_TIMEOUTS.load(Ordering::Relaxed),
            RECORDS_DROPPED.load(Ordering::Relaxed),
            f32::from_bits(LASER_VALUE.load(Ordering::Relaxed)),
        );
    }
}
