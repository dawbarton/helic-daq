//! Peripheral drivers for HELIC-DAQ, generic over `embedded-hal` 1.0 traits so
//! the frame encode/decode logic is unit-tested on the host and future part
//! swaps (AD7606B, AD5764) only implement the same traits.

#![cfg_attr(not(test), no_std)]

pub mod ad5064;
pub mod ad7609;
pub mod optoncdt;
pub mod pwm_out;
pub mod ssi;

/// A simultaneous-sampling analog input device with `N` channels.
///
/// Conversion start is external to the trait (hardware-timed CONVST); this
/// covers the readout path only.
pub trait AnalogIn<const N: usize> {
    type Error;

    /// Read one frame of raw signed conversion results (call once conversion
    /// is complete, i.e. after BUSY falls).
    fn read_frame(&mut self) -> Result<[i32; N], Self::Error>;
}

/// An analog output device with `N` channels addressed by raw DAC code.
pub trait AnalogOut<const N: usize> {
    type Error;

    /// Write and update one channel.
    fn write(&mut self, channel: usize, code: u16) -> Result<(), Self::Error>;
}

#[cfg(test)]
pub(crate) mod mock {
    //! Minimal embedded-hal mocks for driver tests.
    use core::convert::Infallible;
    use embedded_hal::digital::{ErrorType as PinErrorType, OutputPin};
    use embedded_hal::spi::{ErrorType as SpiErrorType, Operation, SpiDevice};

    /// Records written bytes; serves reads from a canned buffer.
    #[derive(Default)]
    pub struct MockSpi {
        pub written: Vec<u8>,
        pub to_read: Vec<u8>,
        pub read_pos: usize,
    }

    impl SpiErrorType for MockSpi {
        type Error = Infallible;
    }

    impl SpiDevice for MockSpi {
        fn transaction(&mut self, operations: &mut [Operation<'_, u8>]) -> Result<(), Infallible> {
            for op in operations {
                match op {
                    Operation::Write(data) => self.written.extend_from_slice(data),
                    Operation::Read(buf) => {
                        for b in buf.iter_mut() {
                            *b = self.to_read.get(self.read_pos).copied().unwrap_or(0);
                            self.read_pos += 1;
                        }
                    }
                    Operation::Transfer(read, write) => {
                        self.written.extend_from_slice(write);
                        for b in read.iter_mut() {
                            *b = self.to_read.get(self.read_pos).copied().unwrap_or(0);
                            self.read_pos += 1;
                        }
                    }
                    Operation::TransferInPlace(buf) => {
                        self.written.extend_from_slice(buf);
                        for b in buf.iter_mut() {
                            *b = self.to_read.get(self.read_pos).copied().unwrap_or(0);
                            self.read_pos += 1;
                        }
                    }
                    Operation::DelayNs(_) => {}
                }
            }
            Ok(())
        }
    }

    /// Records the level history of a pin.
    #[derive(Default)]
    pub struct MockPin {
        pub history: Vec<bool>,
    }

    impl MockPin {
        pub fn level(&self) -> bool {
            *self.history.last().unwrap_or(&false)
        }
    }

    impl PinErrorType for MockPin {
        type Error = Infallible;
    }

    impl OutputPin for MockPin {
        fn set_low(&mut self) -> Result<(), Infallible> {
            self.history.push(false);
            Ok(())
        }
        fn set_high(&mut self) -> Result<(), Infallible> {
            self.history.push(true);
            Ok(())
        }
    }

    /// No-op delay.
    pub struct MockDelay;

    impl embedded_hal::delay::DelayNs for MockDelay {
        fn delay_ns(&mut self, _ns: u32) {}
    }
}
