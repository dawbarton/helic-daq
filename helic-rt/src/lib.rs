//! Portable real-time contracts and shared state for HELIC-DAQ.
//!
//! This crate is deliberately independent of Embassy and RP2350 peripherals,
//! so the contracts between core 0 and core 1 remain host-testable.

#![no_std]

mod shared;

pub use shared::{Diagnostics, Live, RebootShared, RtShared, Safety, SafetyInputs};
