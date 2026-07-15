//! CBC acquisition, actuation, parameters, and core-1 hardware assembly.

use core::cell::RefCell;
use core::sync::atomic::Ordering;

use defmt::warn;
use embassy_embedded_hal::shared_bus::blocking::spi::SpiDeviceWithConfig;
use embassy_rp::gpio::Output;
use embassy_rp::peripherals::{PIN_8, PWM_SLICE4, SPI1};
use embassy_rp::pwm::{self, Pwm};
use embassy_rp::spi::{self, Blocking, Spi};
use embassy_rp::{pac, Peri};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::blocking_mutex::Mutex;
use fixed::traits::ToFixed;
use helic_drivers::ad5064::{Ad5064, ChannelPolarity};
use helic_drivers::ad7609::Ad7609;
use helic_fw_common::analog_spi::{HotSpiConfig, RawSpiDevice, SramAd5064};
use helic_fw_common::rig::{BusyEdgeSpinTick, Rig};
use helic_fw_common::SampleRate;
use static_cell::StaticCell;

use crate::board::CbcParts;
use crate::config::{ActiveController, LASER_RANGE_MM as DEFAULT_LASER_RANGE_MM, OUTPUT_CHANNEL};
use crate::{LASER_RANGE_MM, LASER_VALUE};

/// DAC reference voltage fitted to the interim analogue board.
pub const DAC_VREF: f32 = 4.096;

// Raw chip-select access is the one place pin identity cannot be recovered
// from Embassy's erased Output. Keep these beside the unsafe construction and
// in lockstep with board.rs's auditable pin map.
const ADC_CS_PIN: u8 = 13;
const DAC_CS_PIN: u8 = 9;

/// Output-stage polarity per DAC channel. The fitted interim board has four
/// unipolar outputs; this must change with the physical output stages.
pub const DAC_POLARITY: [ChannelPolarity; 4] = [
    ChannelPolarity::Unipolar,
    ChannelPolarity::Unipolar,
    ChannelPolarity::Unipolar,
    ChannelPolarity::Unipolar,
];

type SpiBus = Mutex<NoopRawMutex, RefCell<Spi<'static, SPI1, Blocking>>>;
type SpiDevice =
    SpiDeviceWithConfig<'static, NoopRawMutex, Spi<'static, SPI1, Blocking>, Output<'static>>;
type Adc = Ad7609<SpiDevice, Output<'static>>;
type Dac = Ad5064<SpiDevice>;

// Both devices borrow the bus for the firmware lifetime. The NoopRawMutex is
// sound because CbcParts is assembled only after it has moved to core 1.
static SPI_BUS: StaticCell<SpiBus> = StaticCell::new();

pub type Tick = BusyEdgeSpinTick;

/// CBC-specific mutable state reached by the generic bounded RT pipeline.
pub struct CbcRig {
    adc: Adc,
    dac: Dac,
    tick_pin: Output<'static>,
    adc_raw: RawSpiDevice,
    dac_raw: SramAd5064,
    convst: Option<(Peri<'static, PWM_SLICE4>, Peri<'static, PIN_8>)>,
    convst_pwm: Option<Pwm<'static>>,
    pwm_divider: u32,
    sample_rate: SampleRate,
    adc_scale: f32,
    adc_last: [i32; 8],
    output_channel: usize,
}

impl CbcParts {
    /// Assemble the core-1-only shared bus and its typed device drivers.
    pub fn build(self, sample_rate: SampleRate) -> (CbcRig, Tick) {
        let tick = BusyEdgeSpinTick::new(self.adc_busy, sample_rate);
        let bus: &'static SpiBus = SPI_BUS.init(Mutex::new(RefCell::new(self.spi)));

        // AD7609 mode 2 at 12 MHz transfers an 18-byte frame in about 12 µs.
        let adc_config = HotSpiConfig::new(
            12_000_000,
            spi::Polarity::IdleHigh,
            spi::Phase::CaptureOnFirstTransition,
        );
        let adc_spi = SpiDeviceWithConfig::new(bus, self.adc_cs, adc_config.embassy());

        // AD5064 mode 1 at 16 MHz transfers one 32-bit word in 2 µs.
        let dac_config = HotSpiConfig::new(
            16_000_000,
            spi::Polarity::IdleLow,
            spi::Phase::CaptureOnSecondTransition,
        );
        let dac_spi = SpiDeviceWithConfig::new(bus, self.dac_cs, dac_config.embassy());

        // SAFETY: board.rs configures GP13 and GP9 as the live chip selects for
        // these SPI1 devices. The whole bundle moves to core 1, and no other
        // task accesses SPI1 or either CS output.
        let adc_raw = unsafe { RawSpiDevice::new(pac::SPI1, adc_config, ADC_CS_PIN) };
        let dac_raw = unsafe { RawSpiDevice::new(pac::SPI1, dac_config, DAC_CS_PIN) };

        // LDAC is a hardware strap in this operating mode. Keeping the Output
        // alive prevents its drop implementation from deconfiguring the pin.
        core::mem::forget(self.dac_ldac);

        let rig = CbcRig {
            adc: Ad7609::new(adc_spi, self.adc_pins),
            dac: Ad5064::new(dac_spi, DAC_POLARITY, DAC_VREF),
            tick_pin: self.tick_pin,
            adc_raw,
            dac_raw: SramAd5064::new(dac_raw, DAC_POLARITY, DAC_VREF),
            convst: Some((self.convst_slice, self.convst_pin)),
            convst_pwm: None,
            pwm_divider: sample_rate.pwm_params().0 as u32,
            sample_rate,
            adc_scale: 0.0,
            adc_last: [0; 8],
            output_channel: OUTPUT_CHANNEL,
        };
        (rig, tick)
    }
}

impl Rig for CbcRig {
    // `measure` fills this exact order. The common loop appends controller
    // telemetry and generated signals without experiment-specific indices.
    const INPUTS: &'static [(&'static str, &'static str)] = &[
        ("adc0", "V"),
        ("adc1", "V"),
        ("adc2", "V"),
        ("adc3", "V"),
        ("adc4", "V"),
        ("adc5", "V"),
        ("adc6", "V"),
        ("adc7", "V"),
        ("laser", "mm"),
    ];

    type Ctrl = ActiveController;

    fn init(&mut self) {
        // Slow reset delays and fail-safe zeroing happen before the sample
        // clock starts, never on the bounded per-tick path.
        self.adc.init(
            helic_drivers::ad7609::InputRange::Bipolar10V,
            helic_drivers::ad7609::Oversampling::for_sample_rate(self.sample_rate.hz()),
            &mut embassy_time::Delay,
        );
        self.adc_scale = self.adc.scale();
        if self
            .dac
            .zero_all_with_delay(&mut embassy_time::Delay)
            .is_err()
        {
            warn!("DAC zeroing failed");
        }
        let (divider, top) = self.sample_rate.pwm_params();
        self.convst_pwm = Some(self.start_convst_pwm(divider, top));
    }

    #[unsafe(link_section = ".data.ram_func")]
    fn measure(&mut self, values: &mut [f32]) {
        #[cfg(not(feature = "diag-skip-adc"))]
        {
            let mut raw = [0u8; 18];
            self.adc_raw.transfer(&mut raw);
            self.adc_last = helic_drivers::ad7609::decode_frame(&raw);
        }
        for (value, raw) in values[..8].iter_mut().zip(self.adc_last) {
            *value = raw as f32 * self.adc_scale;
        }
        values[8] = f32::from_bits(LASER_VALUE.load(Ordering::Relaxed));
    }

    #[unsafe(link_section = ".data.ram_func")]
    fn actuate(&mut self, out: f32) {
        #[cfg(feature = "diag-skip-dac")]
        let _ = out;
        #[cfg(not(feature = "diag-skip-dac"))]
        self.dac_raw.write_volts(self.output_channel, out);
    }

    #[unsafe(link_section = ".data.ram_func")]
    fn tick_start(&mut self) {
        self.tick_pin.set_high();
    }

    #[unsafe(link_section = ".data.ram_func")]
    fn tick_phase_us(&self) -> Option<u32> {
        // PWM slice 4 wraps at CONVST's rising edge. With a 150 MHz system
        // clock, the divider converts its counter directly to elapsed µs.
        let ctr = pac::PWM.ch(4).ctr().read().ctr() as u32;
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

    fn set_param(&mut self, id: u16, value: f32) {
        match id {
            0 => LASER_RANGE_MM.store(value.to_bits(), Ordering::Relaxed),
            1 => self.output_channel = value as usize,
            _ => {}
        }
    }
}

impl CbcRig {
    /// Start the crystal-timed CONVST output after ADC and DAC setup.
    fn start_convst_pwm(&mut self, divider: u8, top: u16) -> Pwm<'static> {
        let (slice, pin) = self.convst.take().expect("CONVST PWM already started");
        let mut config = pwm::Config::default();
        config.divider = divider.to_fixed();
        config.top = top;
        config.compare_a = top / 2;
        Pwm::new_output_a(slice, pin, config)
    }
}
