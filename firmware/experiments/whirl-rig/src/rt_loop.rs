//! Concrete task wrapper for the whirl rig's generic real-time loop.

use helic_fw_common::rt_loop::{run_rt_loop, CommandConsumer, RecordProducer};

use crate::board::SensorParts;
use crate::config;

pub use helic_fw_common::rt_loop::{Record, RtCommand, COMMAND_QUEUE_LEN, RECORD_QUEUE_LEN};

#[embassy_executor::task]
pub async fn rt_loop(
    sensors: SensorParts,
    controller: config::ActiveController,
    commands: CommandConsumer,
    records: RecordProducer,
) -> ! {
    let (rig, tick) = sensors.build(config::SAMPLE_RATE);
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
