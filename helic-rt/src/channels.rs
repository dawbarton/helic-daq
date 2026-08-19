//! Bounded command and record types exchanged between the firmware cores.

use heapless::spsc::{Consumer, Producer};
use helic_core::generator::FourierCoeffs;
use helic_core::{Active, CommitToken, Staging};

use crate::{DEFAULT_HARMONICS, MAX_SOURCES};

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
        /// Feature-gated WCET probe. Carries a scalar, like every production
        /// command, so the burst it measures is the burst a rig can actually
        /// receive.
        pub const DIAGNOSTIC_BURST: u16 = u16::MAX;
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
}

/// Address-independent data carried by an [`RtCommand`].
///
/// Deliberately not `Copy` or `Clone`: `Buffer` contains a linear token whose
/// ownership must travel back to the staging endpoint if enqueueing fails.
#[derive(Debug)]
pub enum Payload {
    Unit,
    F32(f32),
    U32(u32),
    Buffer(CommitToken),
}

/// One bounded, component-addressed update applied at a sample boundary.
#[derive(Debug)]
pub struct RtCommand {
    pub domain: u8,
    pub id: u16,
    pub payload: Payload,
}

// Four pointer widths: 16 bytes on the 32-bit target, 32 on a 64-bit host,
// because `CommitToken` carries its owner address. Tight enough that adding
// any inline array to `Payload` fails the build rather than the timing gate.
const _: () = assert!(core::mem::size_of::<RtCommand>() <= 4 * core::mem::size_of::<usize>());

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
pub type CoeffStaging<const H: usize = DEFAULT_HARMONICS> = Staging<FourierCoeffs<H>>;
pub type ActiveCoeffs<const H: usize = DEFAULT_HARMONICS> = Active<FourierCoeffs<H>>;
pub type ActiveTable<const N: usize = { helic_core::MAX_TABLE_LEN }> =
    Active<helic_core::WaveTable<N>>;

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
pub struct RtChannels<const H: usize = DEFAULT_HARMONICS> {
    pub command_tx: CommandProducer,
    pub command_rx: CommandConsumer,
    pub record_tx: RecordProducer,
    pub record_rx: RecordConsumer,
    pub target_staging: CoeffStaging<H>,
    pub target_active: ActiveCoeffs<H>,
    pub forcing_staging: CoeffStaging<H>,
    pub forcing_active: ActiveCoeffs<H>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_envelope_stays_within_reviewed_sram_bound() {
        assert!(core::mem::size_of::<RtCommand>() <= 4 * core::mem::size_of::<usize>());
    }

    #[test]
    fn reviewed_force_vector_uses_the_buffered_capacity() {
        const ACTUATORS: usize = 4;
        const HARMONICS: usize = 16;
        assert_eq!(ACTUATORS * (1 + 2 * HARMONICS), MAX_FORCE_VALUES);
    }
}
