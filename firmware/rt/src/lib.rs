//! Mandatory RP2350 core-1 mechanisms for HELIC-DAQ.
//!
//! Everything reachable per tick is bounded, synchronous, and SRAM-resident.
//! This crate must not depend on Embassy executors, time, networking, or any
//! core-0 support crate.

#![no_std]

pub mod analog_spi;
pub mod pulse_pio;
pub mod raw_pio;
pub mod rig;
pub mod rt_loop;
mod rt_mem;
pub mod ssi_pio;
