//! Universal core-0 support used by every HELIC-DAQ firmware application.
//!
//! A module belongs here only when it runs on core 0 and every rig uses it.
//! Optional network backends implement one universal transport contract; a
//! sensor or other integration used by only some rigs belongs in a dedicated
//! integration crate instead.

#![no_std]

pub mod comms;
pub mod identity;
pub mod net;
pub mod status;
pub mod time_watchdog;
