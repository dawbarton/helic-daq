//! Portable computation owned exclusively by the whirl rig.
//!
//! Keeping this dependency-free library target beside the firmware preserves
//! host testing without scattering one rig across repository-level packages.

#![no_std]

mod rpm;

pub use rpm::{RpmEstimate, RpmEstimator};
