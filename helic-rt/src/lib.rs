//! Portable real-time contracts and shared state for HELIC-DAQ.
//!
//! This crate is deliberately independent of Embassy and RP2350 peripherals,
//! so the contracts between core 0 and core 1 remain host-testable.

#![no_std]

#[cfg(test)]
extern crate std;

pub mod channels;
mod control;
pub mod params;
mod program;
pub mod rig;
mod safety;
mod sample_rate;
mod shared;

pub use channels::{
    command_id, ActiveCoeffs, ActiveTable, CoeffStaging, CommandConsumer, CommandProducer, Payload,
    Record, RecordConsumer, RecordProducer, RtChannels, RtCommand, COMMANDS_PER_TICK,
    COMMAND_QUEUE_LEN, DOMAIN_CONTROLLER, DOMAIN_GENERATOR, DOMAIN_RIG, DOMAIN_TABLE,
    MAX_FORCE_VALUES, RECORD_QUEUE_LEN,
};
pub use control::{
    ControlStep, PassThrough, PidController, StandardControl, StandardControlInputs,
};
pub use program::{Program, StandardProgram, StepCtx};
pub use rig::{
    source, source_count, validate_sources, Rig, TickSource, MAX_ACTUATORS, MAX_SOURCES,
};
pub use safety::{safety_decide, SafetyOutcome};
pub use sample_rate::SampleRate;
pub use shared::{Diagnostics, Live, RebootShared, RtShared, Safety, SafetyInputs};

/// Default and maximum standard Fourier harmonic count.
pub const DEFAULT_HARMONICS: usize = 16;
pub const MAX_HARMONICS: usize = 16;
