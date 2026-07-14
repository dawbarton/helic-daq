//! Concrete task wrapper for the signal generator's generic RT loop.
//!
//! This RT wrapper is intentionally identical to the wired variant: transport
//! is a core-0 concern and does not enter the control tick.

use helic_fw_common::rt_loop::{run_rt_loop, CommandConsumer, RecordProducer};

use crate::board::AnalogParts;
use crate::config;

pub use helic_fw_common::rt_loop::{Record, RtCommand, COMMAND_QUEUE_LEN, RECORD_QUEUE_LEN};

/// Assemble core-1 hardware and run the shared pipeline forever.
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
