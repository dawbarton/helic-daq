//! Board definition for the W5500-EVB-Pico2: the single place where
//! peripherals meet pins. See `docs/implementation_plan.md` §4 for the full
//! pin map.
//!
//! Reserved by the board itself (W5500 on SPI0): GP16 MISO, GP17 CSn,
//! GP18 SCK, GP19 MOSI, GP20 RSTn, GP21 INTn, GP25 LED.
//!
//! Assignments made here:
//! - GP0/GP1: UART0 TX/RX, optoNCDT laser (core 0)
//! - GP2/GP3/GP4: AD7609 OS0/OS1/OS2
//! - GP5: AD7609 RANGE, GP6: RESET, GP7: BUSY (input)
//! - GP8: AD7609 CONVST — PWM slice 4 output A, the hardware sample clock
//! - GP9: AD5064 ~SYNC (CS), GP15: AD5064 ~LDAC (held low)
//! - GP10/GP11/GP12: SPI1 SCK/MOSI/MISO (shared: AD7609 + AD5064)
//! - GP13: AD7609 ~CS, GP14: tick-timing debug pin
//!
//! The analog SPI bus is used only by the real-time loop on core 1, so
//! [`AnalogParts`] is moved to core 1 and assembled there — the shared-bus
//! mutex can then be the zero-cost `NoopRawMutex` (which is `!Sync` and
//! could not be shared across cores anyway).

use core::cell::RefCell;

use cbc_drivers::ad5064::{Ad5064, ChannelPolarity};
use cbc_drivers::ad7609::{Ad7609, ConfigPins};
use embassy_embedded_hal::shared_bus::blocking::spi::SpiDeviceWithConfig;
use embassy_rp::gpio::{Input, Level, Output, Pull};
use embassy_rp::peripherals::{
    CORE1, DMA_CH1, DMA_CH2, DMA_CH3, PIN_1, PIN_16, PIN_18, PIN_19, PIN_8, PWM_SLICE4, SPI0, SPI1,
    UART0,
};
use embassy_rp::pwm::{self, Pwm};
use embassy_rp::spi::{self, Blocking, Spi};
use embassy_rp::{Peri, Peripherals};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::blocking_mutex::Mutex;
use fixed::traits::ToFixed;
use static_cell::StaticCell;

/// DAC reference voltage (ADR-series reference on the analog board).
pub const DAC_VREF: f32 = 4.096;

/// Output-stage polarity per DAC channel: two bipolar (via inverting op-amp
/// stages), two unipolar, per AGENTS.md.
pub const DAC_POLARITY: [ChannelPolarity; 4] = [
    ChannelPolarity::Bipolar,
    ChannelPolarity::Unipolar,
    ChannelPolarity::Bipolar,
    ChannelPolarity::Unipolar,
];

type SpiBus = Mutex<NoopRawMutex, RefCell<Spi<'static, SPI1, Blocking>>>;
type SpiDev =
    SpiDeviceWithConfig<'static, NoopRawMutex, Spi<'static, SPI1, Blocking>, Output<'static>>;

/// Concrete driver types (embassy tasks cannot be generic).
pub type Adc = Ad7609<SpiDev, Output<'static>>;
pub type Dac = Ad5064<SpiDev>;

static SPI_BUS: StaticCell<SpiBus> = StaticCell::new();

pub struct Board {
    /// Board LED (GP25), core-0 heartbeat.
    pub led: Output<'static>,
    /// Everything the real-time loop owns, to be moved to core 1.
    pub analog: AnalogParts,
    /// optoNCDT laser UART, owned by core 0.
    pub laser: LaserParts,
    /// On-board W5500 Ethernet controller (SPI0), owned by core 0.
    pub eth: EthParts,
    /// Second core, handed to `spawn_core1`.
    pub core1: Peri<'static, CORE1>,
}

/// Unconstructed W5500 interface, per the W5500-EVB-Pico2 wiring.
pub struct EthParts {
    pub spi: Peri<'static, SPI0>,
    pub clk: Peri<'static, PIN_18>,
    pub mosi: Peri<'static, PIN_19>,
    pub miso: Peri<'static, PIN_16>,
    pub cs: Output<'static>,
    pub int: Input<'static>,
    pub rst: Output<'static>,
    pub tx_dma: Peri<'static, DMA_CH2>,
    pub rx_dma: Peri<'static, DMA_CH3>,
}

/// Unconstructed UART0 RX for the laser sensor (assembled in `main`, where
/// the interrupt bindings live). GP0 stays reserved for TX (sensor commands,
/// later milestone).
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

/// The assembled core-1 analog subsystem.
pub struct RtAnalog {
    pub adc: Adc,
    pub dac: Dac,
    /// AD7609 BUSY: falls when conversion data is ready.
    pub adc_busy: Input<'static>,
    /// Tick-timing debug pin.
    pub tick_pin: Output<'static>,
    convst: Option<(Peri<'static, PWM_SLICE4>, Peri<'static, PIN_8>)>,
}

impl AnalogParts {
    /// Assemble the shared-bus SPI devices and drivers. Call **on core 1**;
    /// the bus mutex is a `NoopRawMutex`, sound only because everything it
    /// guards lives on that single core.
    pub fn build(self) -> RtAnalog {
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

        RtAnalog {
            adc: Ad7609::new(adc_spi, self.adc_pins),
            dac: Ad5064::new(dac_spi, DAC_POLARITY, DAC_VREF),
            adc_busy: self.adc_busy,
            tick_pin: self.tick_pin,
            convst: Some((self.convst_slice, self.convst_pin)),
        }
    }
}

impl RtAnalog {
    /// Start the hardware sample clock: CONVST as a free-running PWM output.
    /// Conversion starts on each rising edge, crystal-timed — software jitter
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
    pub fn new(p: Peripherals) -> Self {
        let spi = Spi::new_blocking(p.SPI1, p.PIN_10, p.PIN_11, p.PIN_12, spi::Config::default());

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
            eth: EthParts {
                spi: p.SPI0,
                clk: p.PIN_18,
                mosi: p.PIN_19,
                miso: p.PIN_16,
                cs: Output::new(p.PIN_17, Level::High),
                int: Input::new(p.PIN_21, Pull::Up),
                rst: Output::new(p.PIN_20, Level::High),
                tx_dma: p.DMA_CH2,
                rx_dma: p.DMA_CH3,
            },
            core1: p.CORE1,
        }
    }
}
