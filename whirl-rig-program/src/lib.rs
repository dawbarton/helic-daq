//! Hardware-independent programme and DSP components specific to the whirl rig.

#![cfg_attr(not(test), no_std)]

mod rpm;

pub use rpm::{RpmEstimate, RpmEstimator};
