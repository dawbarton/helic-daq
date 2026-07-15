//! AD5064 driver: 4-channel, 16-bit DAC, 32-bit SPI input shift register.
//!
//! Word layout: `[31:28] don't care | [27:24] command | [23:20] address |
//! [19:4] data | [3:0] don't care`. The board wires two channels through
//! inverting op-amp stages to make them bipolar (0–Vref DAC output →
//! ±Vref at the connector); [`ChannelPolarity`] captures the volts↔code
//! mapping per channel.
//!
//! Timing note carried over from rtc: the AD5064 requires ~3 µs between
//! sequential words — the RT loop should space channel writes out rather
//! than writing back-to-back.

use crate::AnalogOut;
use embedded_hal::delay::DelayNs;
use embedded_hal::spi::SpiDevice;

pub const CHANNELS: usize = 4;

/// AD5064 command codes (bits 27:24).
#[derive(Clone, Copy, Debug)]
#[repr(u8)]
pub enum Command {
    WriteInputReg = 0b0000,
    UpdateDacReg = 0b0001,
    WriteInputRegUpdateAll = 0b0010,
    WriteAndUpdate = 0b0011,
    PowerUpDown = 0b0100,
    LdacMaskReg = 0b0101,
    SoftwareReset = 0b0110,
    DaisyChainDisable = 0b1000,
}

/// Encode one 32-bit input-register word. Pure function, host-tested.
#[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
pub fn frame(command: Command, channel: u8, data: u16) -> [u8; 4] {
    debug_assert!(channel < CHANNELS as u8);
    [
        command as u8,
        (channel << 4) | (data >> 12) as u8,
        (data >> 4) as u8,
        (data << 4) as u8,
    ]
}

/// How a board output channel maps volts to DAC codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelPolarity {
    /// Direct DAC output: 0..Vref.
    Unipolar,
    /// Through the inverting op-amp stage: −Vref..+Vref at the connector.
    Bipolar,
}

/// Volts→code conversion for one channel. `vref` is the DAC reference
/// (4.096 V on this board). Input is clamped to the representable range;
/// non-finite inputs (an upstream fault) map to the safe 0 V code rather
/// than whatever `NaN as u16` yields (code 0 = negative full-scale on a
/// bipolar channel).
#[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
pub fn code_for_volts(volts: f32, polarity: ChannelPolarity, vref: f32) -> u16 {
    let volts = if volts.is_finite() { volts } else { 0.0 };
    let normalized = match polarity {
        ChannelPolarity::Unipolar => volts / vref,
        ChannelPolarity::Bipolar => (volts + vref) / (2.0 * vref),
    };
    let clamped = normalized.clamp(0.0, 1.0);
    // Round to nearest code.
    (clamped * 65535.0 + 0.5) as u16
}

/// AD5064 driver. `SPI` is an `SpiDevice`; ~SYNC is the chip select and
/// ~LDAC is tied low (write-and-update per channel), ~CLR tied high.
pub struct Ad5064<SPI> {
    spi: SPI,
    /// Per-channel polarity of the board's output stages.
    pub polarity: [ChannelPolarity; CHANNELS],
    pub vref: f32,
}

impl<SPI, E> Ad5064<SPI>
where
    SPI: SpiDevice<Error = E>,
{
    pub fn new(spi: SPI, polarity: [ChannelPolarity; CHANNELS], vref: f32) -> Self {
        Self {
            spi,
            polarity,
            vref,
        }
    }

    /// Write and update one channel with a raw code.
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn write_code(&mut self, channel: usize, code: u16) -> Result<(), E> {
        self.spi
            .write(&frame(Command::WriteAndUpdate, channel as u8, code))
    }

    /// Write and update one channel in volts (clamped to the channel range).
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn write_volts(&mut self, channel: usize, volts: f32) -> Result<(), E> {
        let code = code_for_volts(volts, self.polarity[channel], self.vref);
        self.write_code(channel, code)
    }

    /// Set every channel to 0 V (mid-scale for bipolar channels). Call at
    /// startup so the outputs are defined before the control loop runs.
    pub fn zero_all(&mut self) -> Result<(), E> {
        for ch in 0..CHANNELS {
            self.write_volts(ch, 0.0)?;
        }
        Ok(())
    }

    /// Set every channel to 0 V, spacing consecutive SPI words for parts
    /// that require an inter-word settling time.
    pub fn zero_all_with_delay(&mut self, delay: &mut impl DelayNs) -> Result<(), E> {
        for ch in 0..CHANNELS {
            self.write_volts(ch, 0.0)?;
            if ch + 1 < CHANNELS {
                delay.delay_us(3);
            }
        }
        Ok(())
    }
}

impl<SPI, E> AnalogOut<CHANNELS> for Ad5064<SPI>
where
    SPI: SpiDevice<Error = E>,
{
    type Error = E;

    fn write(&mut self, channel: usize, code: u16) -> Result<(), E> {
        self.write_code(channel, code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockSpi;

    #[test]
    fn frame_layout_matches_datasheet() {
        // Write-and-update (0b0011) channel 2 with data 0xABCD:
        // bits: 0000 | 0011 | 0010 | 1010 1011 1100 1101 | 0000
        assert_eq!(
            frame(Command::WriteAndUpdate, 2, 0xABCD),
            [0x03, 0x2A, 0xBC, 0xD0]
        );
        assert_eq!(
            frame(Command::SoftwareReset, 0, 0),
            [0x06, 0x00, 0x00, 0x00]
        );
    }

    #[test]
    fn unipolar_volts_mapping() {
        assert_eq!(code_for_volts(0.0, ChannelPolarity::Unipolar, 4.096), 0);
        assert_eq!(
            code_for_volts(4.096, ChannelPolarity::Unipolar, 4.096),
            65535
        );
        assert_eq!(
            code_for_volts(2.048, ChannelPolarity::Unipolar, 4.096),
            32768
        );
        // Out of range clamps.
        assert_eq!(code_for_volts(-1.0, ChannelPolarity::Unipolar, 4.096), 0);
        assert_eq!(code_for_volts(9.9, ChannelPolarity::Unipolar, 4.096), 65535);
    }

    #[test]
    fn bipolar_volts_mapping() {
        assert_eq!(code_for_volts(-4.096, ChannelPolarity::Bipolar, 4.096), 0);
        assert_eq!(
            code_for_volts(4.096, ChannelPolarity::Bipolar, 4.096),
            65535
        );
        assert_eq!(code_for_volts(0.0, ChannelPolarity::Bipolar, 4.096), 32768);
    }

    #[test]
    fn non_finite_volts_map_to_zero_volts() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(code_for_volts(bad, ChannelPolarity::Bipolar, 4.096), 32768);
            assert_eq!(code_for_volts(bad, ChannelPolarity::Unipolar, 4.096), 0);
        }
    }

    #[test]
    fn write_volts_sends_expected_frame() {
        let mut dac = Ad5064::new(
            MockSpi::default(),
            [
                ChannelPolarity::Bipolar,
                ChannelPolarity::Unipolar,
                ChannelPolarity::Bipolar,
                ChannelPolarity::Unipolar,
            ],
            4.096,
        );
        dac.write_volts(0, 0.0).unwrap();
        // Bipolar 0 V = mid-scale 0x8000 on channel 0.
        assert_eq!(dac.spi.written, vec![0x03, 0x08, 0x00, 0x00]);
    }

    #[test]
    fn zero_all_writes_every_channel() {
        let mut dac = Ad5064::new(MockSpi::default(), [ChannelPolarity::Unipolar; 4], 4.096);
        dac.zero_all().unwrap();
        assert_eq!(dac.spi.written.len(), 16);
        // Channel addresses 0..3 in successive frames.
        for ch in 0..4u8 {
            assert_eq!(dac.spi.written[ch as usize * 4 + 1] >> 4, ch);
        }
    }

    #[derive(Default)]
    struct RecordingDelay {
        calls_us: Vec<u32>,
    }

    impl DelayNs for RecordingDelay {
        fn delay_ns(&mut self, ns: u32) {
            self.calls_us.push(ns / 1_000);
        }
    }

    #[test]
    fn zero_all_with_delay_spaces_successive_words() {
        let mut dac = Ad5064::new(MockSpi::default(), [ChannelPolarity::Unipolar; 4], 4.096);
        let mut delay = RecordingDelay::default();
        dac.zero_all_with_delay(&mut delay).unwrap();
        assert_eq!(dac.spi.written.len(), 16);
        assert_eq!(delay.calls_us, vec![3, 3, 3]);
    }
}
