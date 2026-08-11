//! RP2350-specific plumbing shared by every HELIC-DAQ experiment: the
//! real-time loop, tick sources, PIO transports,
//! network transports, protocol servers and sample-rate presets.

#![no_std]

pub mod analog_spi;
pub mod comms;
pub mod identity;
pub mod laser;
pub mod net;
pub mod pulse_pio;
pub mod raw_pio;
pub mod rig;
pub mod rt_loop;
mod rt_mem;
pub mod ssi_pio;
pub mod time_watchdog;
