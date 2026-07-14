//! Concrete task wrapper for the encoder rig's generic real-time loop.
//!
//! Embassy tasks cannot be generic, so the experiment supplies this concrete
//! wrapper. All scheduling, generation, command and record behaviour stays in
//! `firmware/common`; SSI ownership and measurement stay in `board.rs`.

use helic_fw_common::rt_loop::{run_rt_loop, CommandConsumer, RecordProducer};

use crate::board::AnalogParts;
use crate::config;

pub use helic_fw_common::rt_loop::{Record, RtCommand, COMMAND_QUEUE_LEN, RECORD_QUEUE_LEN};

/// Assemble core-1 hardware and enter the shared loop permanently.
#[embassy_executor::task]
pub async fn rt_loop(
    analog: AnalogParts,
    controller: config::ActiveController,
    commands: CommandConsumer,
    records: RecordProducer,
) -> ! {
    // Construction occurs after the parts have moved to core 1, preserving the
    // single-core assumptions of the analogue bus and SSI state machine.
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
