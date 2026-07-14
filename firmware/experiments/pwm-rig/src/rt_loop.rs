use helic_fw_common::rt_loop::{run_rt_loop, CommandConsumer, RecordProducer};

use crate::board::AnalogParts;
use crate::config;

pub use helic_fw_common::rt_loop::{Record, RtCommand, COMMAND_QUEUE_LEN, RECORD_QUEUE_LEN};

#[embassy_executor::task]
pub async fn rt_loop(analog: AnalogParts, commands: CommandConsumer, records: RecordProducer) -> ! {
    let (rig, tick) = analog.build(config::SAMPLE_RATE);
    run_rt_loop(
        rig,
        tick,
        config::make_controller(),
        config::SAMPLE_RATE,
        commands,
        records,
    )
    .await
}
