//! Out-of-workspace RP2350 firmware which composes the core-0 services.
//!
//! `fw-fixture-rig` deliberately links no `helic-fw-support`, so it proves that
//! the real-time platform stands alone but leaves the services every production
//! rig actually uses unexercised across a repository boundary. Those services
//! are the generic, macro- and build-script-bearing half of the platform, so
//! this fixture instantiates them from an independent workspace: `control_run`
//! over locally defined `Rig` and `Program` types, UDP streaming, discovery,
//! status, the time watchdog, and locally derived build identity.
//!
//! It is a compile, link and layout fixture. Nothing here is flashed, and no
//! claim is made about the electrical behaviour of the pin map below.

#![no_std]
#![no_main]

use defmt::{info, unwrap};
use defmt_rtt as _;
use embassy_executor::{Executor, Spawner};
use embassy_rp::bind_interrupts;
use embassy_rp::block::ImageDef;
use embassy_rp::gpio::{Input, Level, Output, Pull};
use embassy_rp::multicore::{spawn_core1, Stack as CoreStack};
use embassy_rp::peripherals::{DMA_CH2, DMA_CH3};
use embassy_rp::spi::{self, Async, Spi};
use embassy_rp::Peripherals;
use fixture_rig_program::FixtureController;
use helic_core::{DoubleBuffer, FourierCoeffs, TableBuffer};
use helic_fw_rt::rig::PwmWrapSpinTick;
use helic_fw_rt::rt_loop as shared_rt;
use helic_fw_support::identity::Identity;
use helic_fw_support::net::wiznet::EthernetParts;
use helic_fw_support::net::NetConfig;
use helic_fw_support::{comms, net};
use helic_rt::params::{
    GeneratorGroup, ParamStore, PlatformGroup, RigGroup, ScalarControlGroup, TableGroup,
    TelemetryGroup,
};
use helic_rt::{Program, RecordConsumer, Rig, RtShared, SampleRate, StandardProgram};
use panic_probe as _;
use static_cell::{ConstStaticCell, StaticCell};

const EXPERIMENT: &str = "fixture-service-rig";
const SAMPLE_RATE: SampleRate = SampleRate::Hz1000;
/// Deliberately smaller than production, to exercise capacity const generics
/// being chosen by a consumer rather than fixed by the platform.
const HARMONICS: usize = 2;
const TABLE_CAPACITY: usize = 64;
const MAC_ADDR: [u8; 6] = [0x02, 0x48, 0x4C, 0x00, 0x00, 0x7F];
const NET_CONFIG: NetConfig = NetConfig::Static {
    address: [192, 168, 1, 240],
    prefix: 24,
};

type ActiveProgram = StandardProgram<FixtureController, HARMONICS, TABLE_CAPACITY>;
type Store = ParamStore;

/// Build identity of this application, not of the shared platform crates.
const IDENTITY: Identity = helic_fw_support::firmware_identity!();

#[unsafe(link_section = ".start_block")]
#[used]
pub static IMAGE_DEF: ImageDef = ImageDef::secure_exe();

bind_interrupts!(pub struct Irqs {
    TIMER0_IRQ_1 => helic_fw_support::time_watchdog::TimeWatchdogHandler;
    DMA_IRQ_0 => embassy_rp::dma::InterruptHandler<DMA_CH2>,
        embassy_rp::dma::InterruptHandler<DMA_CH3>;
});

static CORE1_STACK: StaticCell<CoreStack<16384>> = StaticCell::new();
static EXECUTOR0: StaticCell<Executor> = StaticCell::new();
static RT_SHARED: RtShared = RtShared::new();
static TABLE: ConstStaticCell<TableBuffer<TABLE_CAPACITY>> =
    ConstStaticCell::new(TableBuffer::new());
static TARGET_COEFFS: ConstStaticCell<DoubleBuffer<FourierCoeffs<HARMONICS>>> =
    ConstStaticCell::new(DoubleBuffer::from_banks(
        FourierCoeffs::zero(),
        FourierCoeffs::zero(),
    ));
static FORCING_COEFFS: ConstStaticCell<DoubleBuffer<FourierCoeffs<HARMONICS>>> =
    ConstStaticCell::new(DoubleBuffer::from_banks(
        FourierCoeffs::zero(),
        FourierCoeffs::zero(),
    ));
static PLATFORM_GROUP: StaticCell<PlatformGroup> = StaticCell::new();
static GENERATOR_GROUP: StaticCell<GeneratorGroup<HARMONICS>> = StaticCell::new();
static TABLE_GROUP: StaticCell<TableGroup<TABLE_CAPACITY>> = StaticCell::new();
static CONTROLLER_GROUP: StaticCell<ScalarControlGroup<FixtureController, HARMONICS>> =
    StaticCell::new();
static RIG_GROUP: StaticCell<RigGroup<ServiceRig>> = StaticCell::new();
static TELEMETRY_GROUP: StaticCell<TelemetryGroup> = StaticCell::new();

/// A rig with no sensors, present so that the generic services have concrete
/// `Rig` types to instantiate over.
struct ServiceRig {
    tick_pin: Output<'static>,
}

impl Rig for ServiceRig {
    const INPUTS: &'static [(&'static str, &'static str)] = &[("sense", "V")];
    const ACTUATORS: &'static [(&'static str, &'static str)] = &[("drive", "V")];

    #[unsafe(link_section = ".data.ram_func")]
    fn init(&mut self) {
        self.tick_pin.set_low();
    }

    #[unsafe(link_section = ".data.ram_func")]
    fn measure(&mut self, values: &mut [f32]) {
        values[0] = 0.0;
    }

    #[unsafe(link_section = ".data.ram_func")]
    fn actuate(&mut self, outputs: &[f32]) {
        core::hint::black_box(outputs);
    }

    #[unsafe(link_section = ".data.ram_func")]
    fn prepare_reboot(&mut self, _step: u8) -> bool {
        true
    }
}

/// Pin map for a W5500-EVB-Pico2, matching the wired production boards.
struct Board {
    led: Output<'static>,
    eth: EthernetParts,
    core1: embassy_rp::Peri<'static, embassy_rp::peripherals::CORE1>,
    tick_pin: Output<'static>,
    tick_slice: embassy_rp::Peri<'static, embassy_rp::peripherals::PWM_SLICE4>,
}

impl Board {
    fn new(p: Peripherals) -> Self {
        let mut eth_config = spi::Config::default();
        eth_config.frequency = 40_000_000;
        let eth_spi: Spi<'static, embassy_rp::peripherals::SPI0, Async> = Spi::new(
            p.SPI0, p.PIN_18, p.PIN_19, p.PIN_16, p.DMA_CH2, p.DMA_CH3, Irqs, eth_config,
        );

        Self {
            led: Output::new(p.PIN_25, Level::Low),
            eth: EthernetParts {
                spi: eth_spi,
                cs: Output::new(p.PIN_17, Level::High),
                int: Input::new(p.PIN_21, Pull::Up),
                rst: Output::new(p.PIN_20, Level::High),
            },
            core1: p.CORE1,
            tick_pin: Output::new(p.PIN_14, Level::Low),
            tick_slice: p.PWM_SLICE4,
        }
    }
}

#[cortex_m_rt::entry]
fn main() -> ! {
    let p = embassy_rp::init(Default::default());
    info!("boot: {} (platform {})", IDENTITY.banner, IDENTITY.platform);
    let b = Board::new(p);

    let channels = shared_rt::init_channels(TARGET_COEFFS.take(), FORCING_COEFFS.take());
    let (table_staging, active_table) = TABLE.take().split();
    let controller = FixtureController::new();
    let mut store = Store::new(channels.command_tx, &RT_SHARED, SAMPLE_RATE);
    store.push(PLATFORM_GROUP.init(PlatformGroup::new(
        &RT_SHARED,
        SAMPLE_RATE,
        IDENTITY.version,
        EXPERIMENT,
    )));
    store.push(GENERATOR_GROUP.init(GeneratorGroup::new(
        channels.target_staging,
        channels.forcing_staging,
        SAMPLE_RATE,
    )));
    store.push(TABLE_GROUP.init(TableGroup::new(table_staging, SAMPLE_RATE)));
    store.push(CONTROLLER_GROUP.init(ScalarControlGroup::new(
        &controller,
        ServiceRig::INPUTS.len(),
    )));
    store.push(RIG_GROUP.init(RigGroup::<ServiceRig>::new()));
    store.push(TELEMETRY_GROUP.init(TelemetryGroup::new(&[])));
    store.validate(<ActiveProgram as Program>::DOMAINS);
    helic_rt::validate_sources::<ServiceRig, ActiveProgram>();
    let program = StandardProgram::new(
        controller,
        channels.target_active,
        channels.forcing_active,
        active_table,
        &RT_SHARED,
    );

    let tick_pin = b.tick_pin;
    let tick_slice = b.tick_slice;
    spawn_core1(b.core1, CORE1_STACK.init(CoreStack::new()), move || {
        let rig = ServiceRig { tick_pin };
        let tick = PwmWrapSpinTick::new(tick_slice, SAMPLE_RATE);
        shared_rt::run_rt_loop(
            rig,
            tick,
            program,
            SAMPLE_RATE,
            &RT_SHARED,
            channels.command_rx,
            channels.record_tx,
        )
    });

    helic_fw_support::time_watchdog::start();

    let executor0 = EXECUTOR0.init(Executor::new());
    executor0.run(|spawner| {
        spawner.spawn(unwrap!(core0_main(
            spawner,
            b.eth,
            store,
            channels.record_rx
        )));
        spawner.spawn(unwrap!(blink(b.led)));
        spawner.spawn(unwrap!(status_task()));
    });
}

#[embassy_executor::task]
async fn core0_main(spawner: Spawner, eth: EthernetParts, store: Store, records: RecordConsumer) {
    let stack = net::wiznet::init(spawner, eth, MAC_ADDR, NET_CONFIG).await;
    spawner.spawn(unwrap!(control_task(stack, store)));
    spawner.spawn(unwrap!(comms::udp::stream_task(stack, records, &RT_SHARED)));
    spawner.spawn(unwrap!(comms::beacon::beacon_task(
        stack,
        MAC_ADDR,
        EXPERIMENT,
        IDENTITY.version,
    )));
}

#[embassy_executor::task]
async fn control_task(stack: embassy_net::Stack<'static>, store: Store) -> ! {
    comms::tcp::control_run::<ServiceRig, ActiveProgram>(stack, store).await
}

#[embassy_executor::task]
async fn blink(mut led: Output<'static>) -> ! {
    loop {
        led.toggle();
        embassy_time::Timer::after_millis(500).await;
    }
}

#[embassy_executor::task]
async fn status_task() -> ! {
    helic_fw_support::status::status_run(&RT_SHARED).await
}
