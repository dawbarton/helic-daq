//! Concrete wrappers for the Pico 2W rig's generic real-time loop.
//!
//! Wireless transport remains a core-0 concern and does not enter the control
//! tick.

#[cfg(not(feature = "rt-sync"))]
use helic_fw_common::rt_loop::run_rt_loop;
use helic_fw_common::rt_loop::{CommandConsumer, RecordProducer};

use crate::board::AnalogParts;
use crate::config;

pub use helic_fw_common::rt_loop::{Record, RtCommand, COMMAND_QUEUE_LEN, RECORD_QUEUE_LEN};

/// Assemble core-1 hardware and run the shared pipeline forever.
#[cfg(not(feature = "rt-sync"))]
#[embassy_executor::task]
pub async fn rt_loop(
    analog: AnalogParts,
    controller: config::ActiveController,
    commands: CommandConsumer,
    records: RecordProducer,
) -> ! {
    // Build only after ownership has moved to core 1.
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

/// Assemble the DAC on core 1, then own the core with the synchronous
/// SRAM-resident real-time loop.
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
