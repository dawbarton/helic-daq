//! Concrete CBC assembly wrapper for the generic real-time loop.
//!
//! `run_rt_loop` contains the reusable bounded pipeline. This small wrapper is
//! the only place which joins the CBC peripheral bundle to that pipeline.

use helic_fw_common::rt_loop::{CommandConsumer, RecordProducer};

use crate::board::CbcParts;
use crate::config;

/// Assemble the hardware after it has moved to core 1, then run forever.
pub fn run(
    parts: CbcParts,
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
