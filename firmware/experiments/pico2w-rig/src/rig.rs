//! Pico 2W DAC rig assembly, actuation, and experiment parameters.

use core::sync::atomic::Ordering;

use defmt::warn;
use embassy_rp::gpio::Output;
use embassy_rp::pac;
use embassy_rp::peripherals::SPI1;
use embassy_rp::pwm::Slice;
use embassy_rp::spi::{self, Blocking, Spi};
use embassy_time::Delay;
use embedded_hal_bus::spi::ExclusiveDevice;
use helic_drivers::ad5064::{Ad5064, ChannelPolarity};
use helic_fw_common::analog_spi::{HotSpiConfig, RawSpiDevice, SramAd5064};
use helic_fw_common::rig::{PwmWrapSpinTick, Rig};
use helic_fw_common::SampleRate;

use crate::board::PicoDacParts;
use crate::config::{ActiveController, LASER_RANGE_MM as DEFAULT_LASER_RANGE_MM, OUTPUT_CHANNEL};
use crate::telemetry::{LASER_RANGE_MM, LASER_VALUE};

const DAC_VREF: f32 = 4.096;
const DAC_CS_PIN: u8 = 9;
pub(crate) const DAC_SPI_CONFIG: HotSpiConfig = HotSpiConfig::new(
    16_000_000,
    spi::Polarity::IdleLow,
    spi::Phase::CaptureOnSecondTransition,
);
const DAC_POLARITY: [ChannelPolarity; 4] = [
    ChannelPolarity::Unipolar,
    ChannelPolarity::Unipolar,
    ChannelPolarity::Unipolar,
    ChannelPolarity::Unipolar,
];

type DacSpi = ExclusiveDevice<Spi<'static, SPI1, Blocking>, Output<'static>, Delay>;
type Dac = Ad5064<DacSpi>;
pub type Tick = PwmWrapSpinTick;

/// Pico 2W experiment state reached by the generic real-time pipeline.
pub struct PicoDacRig {
    dac: Dac,
    tick_pin: Output<'static>,
    output_channel: usize,
    dac_raw: SramAd5064,
    pwm_slice: usize,
    pwm_divider: u32,
}

impl PicoDacParts {
    pub fn build(self, sample_rate: SampleRate) -> (PicoDacRig, Tick) {
        let spi = ExclusiveDevice::new(self.spi, self.dac_cs, Delay)
            .expect("AD5064 SPI device construction failed");

        // SAFETY: board.rs configures GP9 as SPI1's only chip select. The SPI,
        // output, and DAC move together to core 1 and have no concurrent user.
        let dac_raw = unsafe { RawSpiDevice::new(pac::SPI1, DAC_SPI_CONFIG, DAC_CS_PIN) };

        // LDAC remains low for write-and-update operation throughout the run.
        core::mem::forget(self.dac_ldac);
        let rig = PicoDacRig {
            dac: Ad5064::new(spi, DAC_POLARITY, DAC_VREF),
            tick_pin: self.tick_pin,
            output_channel: OUTPUT_CHANNEL,
            dac_raw: SramAd5064::new(dac_raw, DAC_POLARITY, DAC_VREF),
            pwm_slice: self.tick_slice.number(),
            pwm_divider: sample_rate.pwm_params().0 as u32,
        };

        // Do not expose the first wrap until the complete DAC transport exists.
        let tick = PwmWrapSpinTick::new(self.tick_slice, sample_rate);
        (rig, tick)
    }
}

impl Rig for PicoDacRig {
    const INPUTS: &'static [(&'static str, &'static str)] = &[("laser", "mm")];

    type Ctrl = ActiveController;

    fn init(&mut self) {
        if self
            .dac
            .zero_all_with_delay(&mut embassy_time::Delay)
            .is_err()
        {
            warn!("DAC zeroing failed");
        }
    }

    #[unsafe(link_section = ".data.ram_func")]
    fn measure(&mut self, values: &mut [f32]) {
        values[0] = f32::from_bits(LASER_VALUE.load(Ordering::Relaxed));
    }

    #[unsafe(link_section = ".data.ram_func")]
    fn actuate(&mut self, out: f32) {
        self.dac_raw.write_volts(self.output_channel, out);
    }

    #[unsafe(link_section = ".data.ram_func")]
    fn tick_start(&mut self) {
        self.tick_pin.set_high();
    }

    #[unsafe(link_section = ".data.ram_func")]
    fn tick_phase_us(&self) -> Option<u32> {
        // The tick PWM slice (board.rs `tick_slice`) wraps at the sample
        // instant. With a 150 MHz system clock, the divider converts its
        // counter directly to elapsed µs.
        let ctr = pac::PWM.ch(self.pwm_slice).ctr().read().ctr() as u32;
        Some(ctr * self.pwm_divider / 150)
    }

    #[unsafe(link_section = ".data.ram_func")]
    fn tick_end(&mut self) {
        self.tick_pin.set_low();
    }

    fn param_names() -> &'static [&'static str] {
        &["rig_laser_range", "rig_out_channel"]
    }

    fn param_defaults() -> &'static [f32] {
        &[DEFAULT_LASER_RANGE_MM, OUTPUT_CHANNEL as f32]
    }

    fn normalise_param(id: u16, value: f32) -> Option<f32> {
        match id {
            0 if value.is_finite() && value > 0.0 => Some(value),
            1 if value.is_finite()
                && value >= 0.0
                && value < DAC_POLARITY.len() as f32
                && value == value as usize as f32 =>
            {
                Some(value)
            }
            _ => None,
        }
    }

    #[unsafe(link_section = ".data.ram_func")]
    fn set_param(&mut self, id: u16, value: f32) {
        match id {
            0 => LASER_RANGE_MM.store(value.to_bits(), Ordering::Relaxed),
            1 => self.output_channel = value as usize,
            _ => {}
        }
    }
}
