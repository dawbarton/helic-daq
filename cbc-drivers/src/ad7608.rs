//! AD7608 driver: 8-channel, 18-bit simultaneous-sampling ADC.
//!
//! Configuration (range, oversampling) is via logic inputs — individual GPIO
//! pins, per the current hardware. The future AD7606B variant will configure
//! the same behaviour over SPI behind the same [`AnalogIn`] trait.
//!
//! Conversion is triggered externally (hardware-timed CONVST from a PWM
//! slice); readout is 8 × 18 bits = 144 bits (18 bytes) on DOUTA over SPI
//! mode 2 after BUSY falls.

use crate::AnalogIn;
use embedded_hal::delay::DelayNs;
use embedded_hal::digital::OutputPin;
use embedded_hal::spi::SpiDevice;

pub const CHANNELS: usize = 8;
pub const BITS: u32 = 18;

/// Input range, set by the RANGE logic pin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputRange {
    /// ±5 V (RANGE pin low).
    Bipolar5V,
    /// ±10 V (RANGE pin high).
    Bipolar10V,
}

impl InputRange {
    /// Full-scale range in volts (span).
    pub const fn fsr(self) -> f32 {
        match self {
            Self::Bipolar5V => 10.0,
            Self::Bipolar10V => 20.0,
        }
    }

    /// Volts per LSB: multiply raw codes by this to get volts.
    pub const fn scale(self) -> f32 {
        self.fsr() / (1 << BITS) as f32
    }

    const fn pin_high(self) -> bool {
        matches!(self, Self::Bipolar10V)
    }
}

/// Oversampling ratio, set by the OS[2:0] logic pins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Oversampling {
    Os1 = 0b000,
    Os2 = 0b001,
    Os4 = 0b010,
    Os8 = 0b011,
    Os16 = 0b100,
    Os32 = 0b101,
    Os64 = 0b110,
}

impl Oversampling {
    /// Highest ratio whose conversion time still fits the sample period,
    /// following the mapping proven in the previous rtc implementation.
    pub fn for_sample_rate(fs: f32) -> Self {
        if fs > 13_700.0 {
            Self::Os2
        } else if fs > 10_300.0 {
            Self::Os4
        } else if fs > 6_000.0 {
            Self::Os8
        } else if fs > 3_000.0 {
            Self::Os16
        } else if fs > 1_500.0 {
            Self::Os32
        } else {
            Self::Os64
        }
    }
}

/// Unpack a 144-bit read (MSB first, channel 1 first) into 8 sign-extended
/// 18-bit values. Pure function, unit-tested on the host.
pub fn decode_frame(raw: &[u8; 18]) -> [i32; CHANNELS] {
    let mut out = [0i32; CHANNELS];
    for (ch, v) in out.iter_mut().enumerate() {
        let mut acc: u32 = 0;
        for bit_idx in ch * 18..(ch + 1) * 18 {
            let bit = (raw[bit_idx / 8] >> (7 - (bit_idx % 8))) & 1;
            acc = (acc << 1) | bit as u32;
        }
        // Sign-extend from bit 17.
        *v = ((acc << 14) as i32) >> 14;
    }
    out
}

/// The GPIO-configured control pins of the AD7608.
pub struct ConfigPins<P> {
    pub os0: P,
    pub os1: P,
    pub os2: P,
    pub range: P,
    pub reset: P,
}

/// AD7608 driver. `SPI` is an `SpiDevice` (chip select managed by the HAL);
/// STBY is assumed tied high in hardware, CONVST and BUSY are owned by the
/// real-time loop.
pub struct Ad7608<SPI, P> {
    spi: SPI,
    pins: ConfigPins<P>,
    range: InputRange,
}

impl<SPI, P, E> Ad7608<SPI, P>
where
    SPI: SpiDevice<Error = E>,
    P: OutputPin,
{
    pub fn new(spi: SPI, pins: ConfigPins<P>) -> Self {
        Self {
            spi,
            pins,
            range: InputRange::Bipolar5V,
        }
    }

    /// Configure pins and issue a reset pulse. Call once at startup, before
    /// the first conversion is triggered.
    pub fn init(&mut self, range: InputRange, os: Oversampling, delay: &mut impl DelayNs) {
        self.set_range(range);
        self.set_oversampling(os);
        // RESET pulse: t_RESET ≥ 50 ns high; allow generous settling after.
        let _ = self.pins.reset.set_high();
        delay.delay_us(1);
        let _ = self.pins.reset.set_low();
        delay.delay_us(1);
    }

    pub fn set_range(&mut self, range: InputRange) {
        self.range = range;
        let _ = self.pins.range.set_state(range.pin_high().into());
    }

    pub fn range(&self) -> InputRange {
        self.range
    }

    /// Volts per LSB at the current range.
    pub fn scale(&self) -> f32 {
        self.range.scale()
    }

    pub fn set_oversampling(&mut self, os: Oversampling) {
        let bits = os as u8;
        let _ = self.pins.os0.set_state((bits & 0b001 != 0).into());
        let _ = self.pins.os1.set_state((bits & 0b010 != 0).into());
        let _ = self.pins.os2.set_state((bits & 0b100 != 0).into());
    }
}

impl<SPI, P, E> AnalogIn<CHANNELS> for Ad7608<SPI, P>
where
    SPI: SpiDevice<Error = E>,
    P: OutputPin,
{
    type Error = E;

    fn read_frame(&mut self) -> Result<[i32; CHANNELS], E> {
        let mut raw = [0u8; 18];
        self.spi.read(&mut raw)?;
        Ok(decode_frame(&raw))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::{MockDelay, MockPin, MockSpi};

    /// Pack 8 × 18-bit values MSB-first into 18 bytes (the inverse of
    /// `decode_frame`).
    fn pack_frame(values: [i32; 8]) -> [u8; 18] {
        let mut raw = [0u8; 18];
        for (ch, v) in values.iter().enumerate() {
            let code = (*v as u32) & 0x3FFFF;
            for b in 0..18 {
                let bit = (code >> (17 - b)) & 1;
                let idx = ch * 18 + b;
                raw[idx / 8] |= (bit as u8) << (7 - (idx % 8));
            }
        }
        raw
    }

    #[test]
    fn decode_recovers_packed_values() {
        let values = [131071, -131072, 0, -1, 1, 42_i32, -31337, 65536];
        assert_eq!(decode_frame(&pack_frame(values)), values);
    }

    #[test]
    fn decode_all_zero_and_all_one() {
        assert_eq!(decode_frame(&[0u8; 18]), [0i32; 8]);
        assert_eq!(decode_frame(&[0xFF; 18]), [-1i32; 8]);
    }

    #[test]
    fn scale_matches_lsb_size() {
        assert_eq!(InputRange::Bipolar5V.scale(), 10.0 / 262144.0);
        assert_eq!(InputRange::Bipolar10V.scale(), 20.0 / 262144.0);
        // Full-scale code maps to (almost) +5 V.
        let v = 131_071.0 * InputRange::Bipolar5V.scale();
        assert!((v - 5.0).abs() < 1e-4);
    }

    #[test]
    fn read_frame_decodes_spi_bytes() {
        let values = [1000, -1000, 500, -500, 0, 131071, -131072, 77];
        let spi = MockSpi {
            to_read: pack_frame(values).to_vec(),
            ..Default::default()
        };
        let pins = ConfigPins {
            os0: MockPin::default(),
            os1: MockPin::default(),
            os2: MockPin::default(),
            range: MockPin::default(),
            reset: MockPin::default(),
        };
        let mut adc = Ad7608::new(spi, pins);
        assert_eq!(adc.read_frame().unwrap(), values);
    }

    #[test]
    fn init_sets_pins_and_pulses_reset() {
        let pins = ConfigPins {
            os0: MockPin::default(),
            os1: MockPin::default(),
            os2: MockPin::default(),
            range: MockPin::default(),
            reset: MockPin::default(),
        };
        let mut adc = Ad7608::new(MockSpi::default(), pins);
        adc.init(InputRange::Bipolar10V, Oversampling::Os8, &mut MockDelay);
        // Os8 = 0b011: os0 high, os1 high, os2 low; range high for ±10 V.
        assert!(adc.pins.os0.level());
        assert!(adc.pins.os1.level());
        assert!(!adc.pins.os2.level());
        assert!(adc.pins.range.level());
        // Reset pulsed high then low.
        assert_eq!(adc.pins.reset.history, vec![true, false]);
    }

    #[test]
    fn oversampling_for_preset_rates() {
        assert_eq!(Oversampling::for_sample_rate(8000.0), Oversampling::Os8);
        assert_eq!(Oversampling::for_sample_rate(4000.0), Oversampling::Os16);
        assert_eq!(Oversampling::for_sample_rate(2000.0), Oversampling::Os32);
        assert_eq!(Oversampling::for_sample_rate(1000.0), Oversampling::Os64);
    }
}
