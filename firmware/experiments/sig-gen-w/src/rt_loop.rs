//! Concrete Pico 2W assembly wrapper for the generic real-time loop.
//!
//! Wireless transport remains a core-0 concern and does not enter the control
//! tick.

use helic_fw_common::rt_loop::{CommandConsumer, RecordProducer};

use crate::board::PicoDacParts;
use crate::config;

pub use helic_fw_common::rt_loop::{Record, RtCommand, COMMAND_QUEUE_LEN, RECORD_QUEUE_LEN};

/// Assemble the DAC on core 1, then own the core with the synchronous
/// SRAM-resident real-time loop.
pub fn run(
    parts: PicoDacParts,
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
