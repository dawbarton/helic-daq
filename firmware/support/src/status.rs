//! Periodic core-0 status logging from the injected real-time state.

use core::sync::atomic::Ordering;

use defmt::info;
use embassy_time::{Duration, Ticker};
use helic_rt::RtShared;

pub async fn status_run(shared: &'static RtShared) -> ! {
    let mut ticker = Ticker::every(Duration::from_secs(1));
    loop {
        ticker.next().await;
        info!(
            "ticks {} | loop {}/{} us | jitter {} us | overruns {} | tick timeouts {} | dropped {} | cmd backlog {} | armed {} tripped {} clamp {} quiet {}",
            shared.live.ticks.load(Ordering::Relaxed),
            shared.live.loop_time_last_us.load(Ordering::Relaxed),
            shared.diagnostics.loop_time_max_us.load(Ordering::Relaxed),
            shared.diagnostics.clock_jitter_us.load(Ordering::Relaxed),
            shared.diagnostics.overruns.load(Ordering::Relaxed),
            shared.diagnostics.tick_timeouts.load(Ordering::Relaxed),
            shared.diagnostics.records_dropped.load(Ordering::Relaxed),
            shared.diagnostics.command_backlog_max.load(Ordering::Relaxed),
            shared.safety.load_inputs().armed as u32,
            shared.safety.load_inputs().tripped as u32,
            shared.diagnostics.safety_clamp_ticks.load(Ordering::Relaxed),
            shared.diagnostics.safety_quiet_ticks.load(Ordering::Relaxed),
        );
    }
}
