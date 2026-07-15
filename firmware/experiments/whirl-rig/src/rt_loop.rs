//! Concrete whirl assembly wrapper for the generic real-time loop.

use helic_fw_common::rt_loop::{CommandConsumer, RecordProducer};

use crate::board::WhirlParts;
use crate::config;

pub use helic_fw_common::rt_loop::{Record, RtCommand, COMMAND_QUEUE_LEN, RECORD_QUEUE_LEN};

/// Assemble the sensors on core 1, then own the core with the synchronous
/// SRAM-resident real-time loop.
pub fn run(
    parts: WhirlParts,
    controller: config::ActiveController,
    commands: CommandConsumer,
    records: RecordProducer,
) -> ! {
    let (rig, tick) = parts.build(config::SAMPLE_RATE);
    helic_fw_common::rt_loop::run_rt_loop(
        rig,
        tick,
        controller,
        config::SAMPLE_RATE,
        commands,
        records,
    )
}
