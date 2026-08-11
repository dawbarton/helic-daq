//! Bounded command and record types exchanged between the firmware cores.

use heapless::spsc::{Consumer, Producer};
use helic_core::CommitToken;

use crate::MAX_SOURCES;

/// Maximum number of scalar values copied inline by one RT command.
///
/// This accommodates one mean plus sixteen sine/cosine harmonic pairs. Wider
/// vectors use an owner-checked [`helic_core::ValueBuffer`], because hardware
/// timing rejected copying 132 values through this envelope. Changing this
/// bound changes queue SRAM use and command WCET, so it is breaking.
#[cfg(not(feature = "diag-wide-command-payload"))]
pub const MAX_RT_VALUES: usize = 33;
#[cfg(feature = "diag-wide-command-payload")]
pub const MAX_RT_VALUES: usize = 132;
/// Widest reviewed buffered force vector: four actuators at sixteen harmonics.
pub const MAX_FORCE_VALUES: usize = 132;

/// Command domain reserved for experiment hardware.
pub const DOMAIN_RIG: u8 = 0;
/// Standard programme's Fourier generator domain.
pub const DOMAIN_GENERATOR: u8 = 1;
/// Standard programme's waveform-table player domain.
pub const DOMAIN_TABLE: u8 = 2;
/// Standard programme's controller domain.
pub const DOMAIN_CONTROLLER: u8 = 3;

/// Component-local command identifiers used by the standard programme.
pub mod command_id {
    pub mod generator {
        pub const SET_INCREMENT: u16 = 0;
        pub const SET_TARGET: u16 = 1;
        pub const SET_FORCING: u16 = 2;
        /// Feature-gated WCET probe which materialises every copied value.
        pub const DIAGNOSTIC_VALUES: u16 = u16::MAX;
    }

    pub mod table {
        pub const ACTIVATE: u16 = 0;
        pub const SET_INCREMENT: u16 = 1;
        pub const SET_GAIN: u16 = 2;
        pub const SET_INTERPOLATION: u16 = 3;
        pub const SET_MODE: u16 = 4;
        pub const SET_MULTIPLIER: u16 = 5;
        pub const SET_PHASE: u16 = 6;
        pub const TRIGGER: u16 = 7;
    }

    pub mod controller {
        /// Controller parameter identifiers occupy their natural `u16` range;
        /// reset therefore uses the otherwise unreachable terminal value.
        pub const RESET: u16 = u16::MAX;
    }
}

/// Address-independent data carried by an [`RtCommand`].
///
/// Deliberately not `Copy` or `Clone`: `Buffer` contains a linear token whose
/// ownership must travel back to the staging endpoint if enqueueing fails.
#[derive(Debug)]
#[cfg_attr(
    feature = "diag-wide-command-payload",
    allow(clippy::large_enum_variant)
)]
pub enum Payload {
    Unit,
    F32(f32),
    U32(u32),
    Values { len: u8, data: [f32; MAX_RT_VALUES] },
    Buffer(CommitToken),
}

/// One bounded, component-addressed update applied at a sample boundary.
#[derive(Debug)]
pub struct RtCommand {
    pub domain: u8,
    pub id: u16,
    pub payload: Payload,
}

#[cfg(not(feature = "diag-wide-command-payload"))]
const _: () = assert!(core::mem::size_of::<RtCommand>() <= 160);
const _: () = assert!(core::mem::size_of::<RtCommand>() <= 560);

pub const COMMAND_QUEUE_LEN: usize = 32;
/// Maximum number of host commands applied at one sample boundary.
///
/// A finite queue alone is not a useful 125 µs WCET bound: draining a burst of
/// maximum-width updates could consume the whole period. Remaining commands
/// stay in FIFO order for the next tick and are observable through the backlog
/// maximum.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_envelope_stays_within_reviewed_sram_bound() {
        if cfg!(feature = "diag-wide-command-payload") {
            assert_eq!(MAX_RT_VALUES, 132);
        } else {
            assert_eq!(MAX_RT_VALUES, 33);
            assert!(core::mem::size_of::<RtCommand>() <= 160);
        }
    }

    #[test]
    fn reviewed_force_vector_uses_the_buffered_capacity() {
        const ACTUATORS: usize = 4;
        const HARMONICS: usize = 16;
        assert_eq!(ACTUATORS * (1 + 2 * HARMONICS), MAX_FORCE_VALUES);
        if !cfg!(feature = "diag-wide-command-payload") {
            assert_eq!(1 + 2 * HARMONICS, MAX_RT_VALUES);
        }
    }
}
