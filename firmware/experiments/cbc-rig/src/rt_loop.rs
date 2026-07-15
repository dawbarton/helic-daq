//! Concrete task wrapper for the CBC rig's generic real-time loop.
//!
//! `run_rt_loop` contains the reusable bounded pipeline. This small wrapper is
//! necessary because Embassy task functions must have concrete argument types.
//! Keeping the wrapper thin is deliberate; DSP belongs in `helic-core`, driver
//! logic in `helic-drivers`, and RP2350 plumbing in `firmware/common`.

#[cfg(not(feature = "rt-sync"))]
use helic_fw_common::rt_loop::run_rt_loop;
use helic_fw_common::rt_loop::{CommandConsumer, RecordProducer};

use crate::board::AnalogParts;
use crate::config;

pub use helic_fw_common::rt_loop::{Record, RtCommand, COMMAND_QUEUE_LEN, RECORD_QUEUE_LEN};

/// Assemble the hardware after it has moved to core 1, then run forever.
///
/// `async fn` may wait for hardware without blocking the executor. The `!`
/// return type states that the real-time loop must never finish normally.
#[cfg(not(feature = "rt-sync"))]
#[embassy_executor::task]
pub async fn rt_loop(
    analog: AnalogParts,
    controller: config::ActiveController,
    commands: CommandConsumer,
    records: RecordProducer,
) -> ! {
    // Building here is important: the SPI bus uses a single-core mutex and
    // must never be assembled on core 0 and subsequently shared.
    let (rig, tick) = analog.build(config::SAMPLE_RATE);
    run_rt_loop(
        rig,
        tick,
        controller,
        config::SAMPLE_RATE,
        commands,
        records,
    )
    .await
}

/// `rt-sync`: same assembly, but the loop owns core 1 outright — no executor
/// runs on the core and every per-tick instruction executes from SRAM.
#[cfg(feature = "rt-sync")]
pub fn rt_loop_sync(
    analog: AnalogParts,
    controller: config::ActiveController,
    commands: CommandConsumer,
    records: RecordProducer,
) -> ! {
    let (rig, tick) = analog.build(config::SAMPLE_RATE);
    helic_fw_common::rt_loop::run_rt_loop_sync(
        rig,
        tick,
        controller,
        config::SAMPLE_RATE,
        commands,
        records,
    )
}
