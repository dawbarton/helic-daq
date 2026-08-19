//! Hardware-independent DSP for control-based continuation.
//!
//! Everything in this crate is `no_std`, allocation-free `f32` code that runs
//! identically on the RP2350's Cortex-M33 FPU and on the host, where it is
//! unit-tested with `cargo test`.

#![cfg_attr(not(test), no_std)]

pub mod filter;
pub mod fourier;
pub mod generator;
mod harmonics;
pub mod lut;
pub mod phase;
pub mod pid;
mod pll;
pub mod safety;
pub mod table;
mod table_buffer;

pub use filter::{BiquadCoeffs, SosFilter};
pub use fourier::FourierEstimator;
pub use generator::FourierCoeffs;
pub use harmonics::{HarmonicFrame, HarmonicGenerator};
pub use lut::SinLut;
pub use phase::PhaseAccumulator;
pub use pid::{Pid, PidConfig};
pub use pll::{Pll, PllConfig, PllState};
pub use safety::StaleCounter;
pub use table::{TableInterpolation, TableMode, TablePlayer, WaveTable, MAX_TABLE_LEN};
pub use table_buffer::{
    Active, ActiveTable, ActiveValues, BufferError, CommitToken, DoubleBuffer, Staging,
    TableBuffer, ValueBuffer, ValueStaging,
};
