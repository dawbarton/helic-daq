//! optoNCDT UART integration shared by rigs that fit the sensor.

#![no_std]

mod laser;

pub use laser::{configured_laser_run, laser_run, LaserCounters};
