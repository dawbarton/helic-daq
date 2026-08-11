//! Type-checked mapping from Embassy PIO ownership to matching PAC registers.

use embassy_rp::pac;
use embassy_rp::peripherals::{PIO0, PIO1, PIO2};
use embassy_rp::pio::Instance;

/// PIO instances whose raw register block is fixed by their concrete type.
///
/// Keeping this sealed by concrete implementations prevents a driver from
/// accepting a typed PIO0 state machine alongside an unrelated PIO1 PAC token.
pub trait RawPioInstance: Instance {
    fn raw() -> pac::pio::Pio;
}

impl RawPioInstance for PIO0 {
    fn raw() -> pac::pio::Pio {
        pac::PIO0
    }
}

impl RawPioInstance for PIO1 {
    fn raw() -> pac::pio::Pio {
        pac::PIO1
    }
}

impl RawPioInstance for PIO2 {
    fn raw() -> pac::pio::Pio {
        pac::PIO2
    }
}
