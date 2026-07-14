//! Concrete Embassy wrapper around the generic real-time loop.
//!
//! The concrete `AnalogParts` type is required by Embassy's task macro. Keep
//! experiment logic in the `Rig` implementation and reusable logic in the
//! common/core crates.

use helic_fw_common::rt_loop::{run_rt_loop, CommandConsumer, RecordProducer};

use crate::board::AnalogParts;
use crate::config;

pub use helic_fw_common::rt_loop::{Record, RtCommand, COMMAND_QUEUE_LEN, RECORD_QUEUE_LEN};

/// Assemble the PWM rig on core 1 and run indefinitely.
#[embassy_executor::task]
pub async fn rt_loop(
    analog: AnalogParts,
    controller: config::ActiveController,
    commands: CommandConsumer,
    records: RecordProducer,
) -> ! {
    // Construction here preserves exclusive core-1 ownership.
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
