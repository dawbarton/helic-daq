//! Board definition for the W5500/W6100-EVB-Pico2: the single place where
//! peripherals meet pins. The complete pin map is kept below.
//!
//! Reserved by the board itself (WIZnet chip on SPI0): GP16 MISO, GP17 CSn,
//! GP18 SCK, GP19 MOSI, GP20 RSTn, GP21 INTn, GP25 LED.
//!
//! Assignments made here:
//! - GP0/GP1: UART0 TX/RX, optoNCDT laser (core 0)
//! - GP2/GP3/GP4: AD7609 OS0/OS1/OS2
//! - GP5: AD7609 RANGE, GP6: RESET, GP7: BUSY (input)
//! - GP8: AD7609 CONVST, PWM slice 4 output A, the hardware sample clock
//! - GP9: AD5064 ~SYNC (CS), GP15: AD5064 ~LDAC (held low)
//! - GP10/GP11/GP12: SPI1 SCK/MOSI/MISO (shared: AD7609 + AD5064)
//! - GP13: AD7609 ~CS, GP14: tick-timing debug pin
//!
//! The analog SPI bus is used only by the real-time loop on core 1, so
//! [`AnalogParts`] is moved to core 1 and assembled there. The shared-bus
//! mutex can then be the zero-cost `NoopRawMutex` (which is `!Sync` and
//! could not be shared across cores anyway).
//!
//! The reusable behaviour is expressed through the `Rig` trait below. A new
//! experiment should keep this module to ownership, pins and thin adaptation;
//! reusable device behaviour belongs in `helic-drivers`. See "Swapping
//! peripherals" in `docs/developer_guide.md`.

use core::cell::RefCell;
use core::sync::atomic::Ordering;

use defmt::warn;
use embassy_embedded_hal::shared_bus::blocking::spi::SpiDeviceWithConfig;
use embassy_rp::gpio::{Input, Level, Output, Pull};
use embassy_rp::peripherals::{CORE1, DMA_CH1, PIN_1, PIN_8, PWM_SLICE4, SPI1, UART0};
use embassy_rp::pwm::{self, Pwm};
use embassy_rp::spi::{self, Async, Blocking, Spi};
use embassy_rp::{Peri, Peripherals};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::blocking_mutex::Mutex;
use fixed::traits::ToFixed;
use helic_drivers::ad5064::{Ad5064, ChannelPolarity};
use helic_drivers::ad7609::{Ad7609, ConfigPins};
use helic_drivers::AnalogIn;
use helic_fw_common::net::wiznet::EthernetParts;
use helic_fw_common::rig::{BusyEdgeTick, Rig};
use helic_fw_common::SampleRate;
use static_cell::StaticCell;

use crate::config::{ActiveController, LASER_RANGE_MM as DEFAULT_LASER_RANGE_MM, OUTPUT_CHANNEL};
use crate::{ADC_ERRORS, LASER_RANGE_MM, LASER_VALUE};

/// DAC reference voltage (ADR-series reference on the analog board).
pub const DAC_VREF: f32 = 4.096;

/// Output-stage polarity per DAC channel. The target design has two bipolar
/// (via inverting op-amp stages) and two unipolar per AGENTS.md, but the
/// interim older rtc board in use for bring-up has **all four unipolar**
/// (0..Vref, no inverting stages).
pub const DAC_POLARITY: [ChannelPolarity; 4] = [
    ChannelPolarity::Unipolar,
    ChannelPolarity::Unipolar,
    ChannelPolarity::Unipolar,
    ChannelPolarity::Unipolar,
];

// These aliases make otherwise long generic hardware types readable. The
// lifetime `'static` means the peripheral remains valid for the firmware's
// entire run, not that it is globally accessible from both cores.
type SpiBus = Mutex<NoopRawMutex, RefCell<Spi<'static, SPI1, Blocking>>>;
type SpiDev =
    SpiDeviceWithConfig<'static, NoopRawMutex, Spi<'static, SPI1, Blocking>, Output<'static>>;

/// Concrete driver types (embassy tasks cannot be generic).
pub type Adc = Ad7609<SpiDev, Output<'static>>;
pub type Dac = Ad5064<SpiDev>;

// The shared ADC/DAC bus requires stable storage because each chip-select
// device borrows it. StaticCell initialises that storage exactly once.
static SPI_BUS: StaticCell<SpiBus> = StaticCell::new();

/// Top-level ownership bundle returned after assigning every board peripheral.
///
/// Rust peripherals are unique values. Moving a field into a task proves no
/// other task can configure or use the same hardware concurrently.
pub struct Board {
    /// Board LED (GP25), core-0 heartbeat.
    pub led: Output<'static>,
    /// Everything the real-time loop owns, to be moved to core 1.
    pub analog: AnalogParts,
    /// optoNCDT laser UART, owned by core 0.
    pub laser: LaserParts,
    /// On-board WIZnet Ethernet controller (SPI0), owned by core 0.
    pub eth: EthernetParts,
    /// Second core, handed to `spawn_core1`.
    pub core1: Peri<'static, CORE1>,
}

/// Unconstructed UART0 RX for the laser sensor (assembled in `main`, where
/// the interrupt bindings live). GP0 stays reserved for possible future
/// sensor commands.
pub struct LaserParts {
    pub uart: Peri<'static, UART0>,
    pub rx: Peri<'static, PIN_1>,
    pub rx_dma: Peri<'static, DMA_CH1>,
}

/// The core-1 peripherals in unassembled (`Send`) form.
pub struct AnalogParts {
    /// Timing-debug pin (GP14): high while the RT tick body runs.
    pub tick_pin: Output<'static>,
    /// AD7609 BUSY (GP7): falls when conversion data is ready.
    pub adc_busy: Input<'static>,
    /// AD5064 ~LDAC (GP15), held low: write-and-update per channel.
    pub dac_ldac: Output<'static>,
    spi: Spi<'static, SPI1, Blocking>,
    adc_cs: Output<'static>,
    adc_pins: ConfigPins<Output<'static>>,
    dac_cs: Output<'static>,
    convst_slice: Peri<'static, PWM_SLICE4>,
    convst_pin: Peri<'static, PIN_8>,
}

/// The assembled core-1 analogue subsystem and its persistent RT state.
///
/// Failed ADC reads deliberately retain `adc_last`; injecting zero into a
/// controller would be a less safe and less observable failure mode.
pub struct RtAnalog {
    pub adc: Adc,
    pub dac: Dac,
    /// Tick-timing debug pin.
    pub tick_pin: Output<'static>,
    convst: Option<(Peri<'static, PWM_SLICE4>, Peri<'static, PIN_8>)>,
    convst_pwm: Option<Pwm<'static>>,
    sample_rate: SampleRate,
    adc_scale: f32,
    adc_last: [i32; 8],
    output_channel: usize,
}

impl AnalogParts {
    /// Assemble the shared-bus SPI devices and drivers. Call **on core 1**;
    /// the bus mutex is a `NoopRawMutex`, sound only because everything it
    /// guards lives on that single core.
    pub fn build(self, sample_rate: SampleRate) -> (RtAnalog, BusyEdgeTick) {
        let tick = BusyEdgeTick::new(self.adc_busy, sample_rate);
        let bus: &'static SpiBus = SPI_BUS.init(Mutex::new(RefCell::new(self.spi)));

        // AD7609 reads in SPI mode 2 (clock idles high, data captured on
        // the falling edge). 12 MHz: 18 bytes in ~12 µs. The datasheet
        // allows faster; verify on scope before raising.
        let mut adc_config = spi::Config::default();
        adc_config.frequency = 12_000_000;
        adc_config.polarity = spi::Polarity::IdleHigh;
        adc_config.phase = spi::Phase::CaptureOnFirstTransition;
        let adc_spi = SpiDeviceWithConfig::new(bus, self.adc_cs, adc_config);

        // AD5064 latches data on falling SCLK: SPI mode 1, write-only.
        // 16 MHz: one 32-bit word in 2 µs.
        let mut dac_config = spi::Config::default();
        dac_config.frequency = 16_000_000;
        dac_config.polarity = spi::Polarity::IdleLow;
        dac_config.phase = spi::Phase::CaptureOnSecondTransition;
        let dac_spi = SpiDeviceWithConfig::new(bus, self.dac_cs, dac_config);

        // ~LDAC stays low for the lifetime of the firmware (write-and-update
        // addressing); leak the pin driver so it is never deconfigured.
        core::mem::forget(self.dac_ldac);

        let rig = RtAnalog {
            adc: Ad7609::new(adc_spi, self.adc_pins),
            dac: Ad5064::new(dac_spi, DAC_POLARITY, DAC_VREF),
            tick_pin: self.tick_pin,
            convst: Some((self.convst_slice, self.convst_pin)),
            convst_pwm: None,
            sample_rate,
            adc_scale: 0.0,
            adc_last: [0; 8],
            output_channel: OUTPUT_CHANNEL,
        };
        (rig, tick)
    }
}

impl Rig for RtAnalog {
    // These names and units become the first entries in source discovery.
    // `measure()` must fill its slice in exactly this order. The common loop
    // appends controller telemetry and generated/output signals automatically.
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

    // Associated types select concrete implementations at compile time. There
    // is no run-time trait-object lookup inside a sample tick.
    type Tick = BusyEdgeTick;
    type Ctrl = ActiveController;

    fn init(&mut self) {
        // `init` runs once on core 1 before the first tick. It is the correct
        // place for slow reset delays and fail-safe output initialisation.
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

    fn measure(&mut self, values: &mut [f32]) {
        // This method is on the bounded RT path. It performs no allocation and
        // uses f32 so arithmetic is accelerated by the Cortex-M33 FPU.
        match self.adc.read_frame() {
            Ok(frame) => self.adc_last = frame,
            Err(_) => {
                ADC_ERRORS.fetch_add(1, Ordering::Relaxed);
            }
        }
        for (value, raw) in values[..8].iter_mut().zip(self.adc_last) {
            *value = raw as f32 * self.adc_scale;
        }
        values[8] = f32::from_bits(LASER_VALUE.load(Ordering::Relaxed));
    }

    fn actuate(&mut self, out: f32) {
        // The driver clamps or maps according to this channel's declared
        // polarity. DAC_POLARITY must match the fitted output stage.
        let _ = self.dac.write_volts(self.output_channel, out);
    }

    fn tick_start(&mut self) {
        self.tick_pin.set_high();
    }

    fn tick_end(&mut self) {
        self.tick_pin.set_low();
    }

    fn param_names() -> &'static [&'static str] {
        // The common registry prefixes these rig controls after base and extra
        // parameters. Keep names, defaults and match-arm IDs in one order.
        &["rig_laser_range", "rig_out_channel"]
    }

    fn param_defaults() -> &'static [f32] {
        &[DEFAULT_LASER_RANGE_MM, OUTPUT_CHANNEL as f32]
    }

    fn normalise_param(id: u16, value: f32) -> Option<f32> {
        // Returning None rejects a host write before acknowledgement. Channel
        // values must be exact integers representable by the DAC array index.
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
        // Commands reach here only at a sample boundary. Large or structured
        // updates use the queue/table mechanisms described in the developer
        // guide rather than shared mutable state.
        match id {
            0 => LASER_RANGE_MM.store(value.to_bits(), Ordering::Relaxed),
            1 => self.output_channel = value as usize,
            _ => {}
        }
    }
}

impl RtAnalog {
    /// Start the hardware sample clock: CONVST as a free-running PWM output.
    /// Conversion starts on each rising edge and is crystal-timed; software jitter
    /// cannot move the sampling instant. Call after ADC init; the returned
    /// handle must be kept alive.
    pub fn start_convst_pwm(&mut self, divider: u8, top: u16) -> Pwm<'static> {
        let (slice, pin) = self.convst.take().expect("CONVST PWM already started");
        let mut cfg = pwm::Config::default();
        cfg.divider = divider.to_fixed();
        cfg.top = top;
        cfg.compare_a = top / 2; // 50% duty; only the rising edge matters
        Pwm::new_output_a(slice, pin, cfg)
    }
}

impl Board {
    /// Consume the singleton peripheral set and assign each item exactly once.
    ///
    /// Keep all pin numbers in this constructor and in the module pin map so a
    /// reviewer can audit wiring without searching algorithmic code.
    pub fn new(p: Peripherals) -> Self {
        // SPI1 is blocking because only core 1 uses it in the bounded tick.
        // SPI0 is asynchronous and DMA-backed so network transfers yield core
        // 0 while the WIZnet controller is active.
        let spi = Spi::new_blocking(p.SPI1, p.PIN_10, p.PIN_11, p.PIN_12, spi::Config::default());
        let mut eth_config = spi::Config::default();
        eth_config.frequency = 40_000_000;
        let eth_spi: Spi<'static, _, Async> = Spi::new(
            p.SPI0,
            p.PIN_18,
            p.PIN_19,
            p.PIN_16,
            p.DMA_CH2,
            p.DMA_CH3,
            crate::Irqs,
            eth_config,
        );

        Self {
            led: Output::new(p.PIN_25, Level::Low),
            analog: AnalogParts {
                tick_pin: Output::new(p.PIN_14, Level::Low),
                // Pull BUSY down so a missing ADC reads as "not converting".
                adc_busy: Input::new(p.PIN_7, Pull::Down),
                dac_ldac: Output::new(p.PIN_15, Level::Low),
                spi,
                adc_cs: Output::new(p.PIN_13, Level::High),
                adc_pins: ConfigPins {
                    os0: Output::new(p.PIN_2, Level::Low),
                    os1: Output::new(p.PIN_3, Level::Low),
                    os2: Output::new(p.PIN_4, Level::Low),
                    range: Output::new(p.PIN_5, Level::Low),
                    reset: Output::new(p.PIN_6, Level::Low),
                },
                dac_cs: Output::new(p.PIN_9, Level::High),
                convst_slice: p.PWM_SLICE4,
                convst_pin: p.PIN_8,
            },
            laser: LaserParts {
                uart: p.UART0,
                rx: p.PIN_1,
                rx_dma: p.DMA_CH1,
            },
            eth: EthernetParts {
                spi: eth_spi,
                cs: Output::new(p.PIN_17, Level::High),
                int: Input::new(p.PIN_21, Pull::Up),
                rst: Output::new(p.PIN_20, Level::High),
            },
            core1: p.CORE1,
        }
    }
}
