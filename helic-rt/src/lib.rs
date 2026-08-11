//! Portable real-time contracts and shared state for HELIC-DAQ.
//!
//! This crate is deliberately independent of Embassy and RP2350 peripherals,
//! so the contracts between core 0 and core 1 remain host-testable.

#![no_std]

#[cfg(test)]
extern crate std;

pub mod channels;
pub mod params;
pub mod rig;
mod sample_rate;
mod shared;

pub use channels::{
    command_id, CommandConsumer, CommandProducer, Payload, Record, RecordConsumer, RecordProducer,
    RtChannels, RtCommand, COMMANDS_PER_TICK, COMMAND_QUEUE_LEN, DOMAIN_CONTROLLER,
    DOMAIN_GENERATOR, DOMAIN_RIG, DOMAIN_TABLE, MAX_FORCE_VALUES, MAX_RT_VALUES, RECORD_QUEUE_LEN,
};
pub use rig::{source, source_count, validate_sources, Rig, TickSource, MAX_SOURCES};
pub use sample_rate::SampleRate;
pub use shared::{Diagnostics, Live, RebootShared, RtShared, Safety, SafetyInputs};

/// Number of harmonics in the current periodic target and forcing generators.
pub const HARMONICS: usize = 16;
