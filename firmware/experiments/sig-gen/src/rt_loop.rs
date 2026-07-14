//! Concrete task wrapper for the signal generator's generic RT loop.
//!
//! Embassy tasks require concrete argument types. The reusable bounded loop is
//! kept in `firmware/common`; this wrapper only selects this experiment's rig,
//! controller and sample clock.

use helic_fw_common::rt_loop::{run_rt_loop, CommandConsumer, RecordProducer};

use crate::board::AnalogParts;
use crate::config;

pub use helic_fw_common::rt_loop::{Record, RtCommand, COMMAND_QUEUE_LEN, RECORD_QUEUE_LEN};

/// Build core-1 hardware, then run the shared pipeline forever.
#[embassy_executor::task]
pub async fn rt_loop(
    analog: AnalogParts,
    controller: config::ActiveController,
    commands: CommandConsumer,
    records: RecordProducer,
) -> ! {
    // Build after ownership has moved to core 1.
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
