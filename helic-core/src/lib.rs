//! Hardware-independent DSP for control-based continuation.
//!
//! Everything in this crate is `no_std`, allocation-free `f32` code that runs
//! identically on the RP2350's Cortex-M33 FPU and on the host, where it is
//! unit-tested with `cargo test`. See `docs/implementation_plan.md` §5.

#![cfg_attr(not(test), no_std)]

pub mod controller;
pub mod filter;
pub mod fourier;
pub mod generator;
pub mod lut;
pub mod phase;
pub mod pid;

pub use controller::{Controller, PassThrough, PidController};
pub use filter::{BiquadCoeffs, SosFilter};
pub use fourier::FourierEstimator;
pub use generator::{
    ArbMode, ArbState, ArbitraryGenerator, FourierCoeffs, GenSample, PeriodicGenerator,
};
pub use lut::SinLut;
pub use phase::PhaseAccumulator;
pub use pid::{Pid, PidConfig};
