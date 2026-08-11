//! Bounded command and record types exchanged between the firmware cores.

use heapless::spsc::{Consumer, Producer};
use helic_core::generator::FourierCoeffs;
use helic_core::table::{TableInterpolation, TableMode};

use crate::{HARMONICS, MAX_SOURCES};

#[derive(Clone, Copy, Debug)]
pub enum RtCommand {
    SetIncrement(u32),
    SetTargetCoeffs(FourierCoeffs<HARMONICS>),
    SetForcingCoeffs(FourierCoeffs<HARMONICS>),
    SetTableIncrement(u32),
    SetTableGain(f32),
    SetTableInterpolation(TableInterpolation),
    SetTableMode(TableMode),
    SetTableMultiplier(u32),
    SetTablePhase(u32),
    TriggerTable,
    UseTable(u8),
    ResetController,
    SetCtrlParam(u16, f32),
    SetRigParam(u16, f32),
}

pub const COMMAND_QUEUE_LEN: usize = 32;
/// Maximum number of host commands applied at one sample boundary.
///
/// A finite queue alone is not a useful 125 µs WCET bound: draining a burst of
/// coefficient sets could consume the whole period. Remaining commands stay in
/// FIFO order for the next tick and are observable through the backlog maximum.
pub const COMMANDS_PER_TICK: usize = 2;
pub type CommandProducer = Producer<'static, RtCommand>;
pub type CommandConsumer = Consumer<'static, RtCommand>;

#[derive(Clone, Copy, Debug, Default)]
pub struct Record {
    pub index: u32,
    pub n: u8,
    pub values: [f32; MAX_SOURCES],
}

pub const RECORD_QUEUE_LEN: usize = 256;
pub type RecordProducer = Producer<'static, Record>;
pub type RecordConsumer = Consumer<'static, Record>;

/// The four uniquely owned queue endpoints connecting the two cores.
pub struct RtChannels {
    pub command_tx: CommandProducer,
    pub command_rx: CommandConsumer,
    pub record_tx: RecordProducer,
    pub record_rx: RecordConsumer,
}
