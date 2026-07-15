//! HELIC-DAQ firmware entry point for the dual-encoder whirl rig.
//!
//! Core 1 owns sampling and estimation; core 0 owns Ethernet control,
//! streaming, discovery, diagnostics and the heartbeat LED.

#![no_std]
#![no_main]

use defmt::{info, unwrap};
use defmt_rtt as _;
use embassy_executor::{Executor, Spawner};
use embassy_rp::bind_interrupts;
use embassy_rp::block::ImageDef;
use embassy_rp::gpio::Output;
use embassy_rp::multicore::{spawn_core1, Stack as CoreStack};
use embassy_rp::peripherals::{DMA_CH2, DMA_CH3, PIO0};
use embassy_rp::pio;
use embassy_time::Timer;
use helic_fw_common::comms;
use helic_fw_common::net;
use helic_fw_common::net::wiznet::EthernetParts;
use helic_fw_common::params::{self, ParamStore};
use helic_fw_common::rt_loop as shared_rt;
use panic_probe as _;
use static_cell::StaticCell;

mod board;
mod config;
mod rig;
mod rt_loop;
mod telemetry;

use rig::WhirlRig;

type Store = ParamStore<config::ActiveController, WhirlRig>;
const _: () =
    assert!(helic_fw_common::rig::source_count::<WhirlRig>() <= helic_fw_common::rig::MAX_SOURCES);

#[unsafe(link_section = ".start_block")]
#[used]
pub static IMAGE_DEF: ImageDef = ImageDef::secure_exe();

bind_interrupts!(pub struct Irqs {
    PIO0_IRQ_0 => pio::InterruptHandler<PIO0>;
    TIMER0_IRQ_1 => helic_fw_common::time_watchdog::TimeWatchdogHandler;
    DMA_IRQ_0 => embassy_rp::dma::InterruptHandler<DMA_CH2>,
        embassy_rp::dma::InterruptHandler<DMA_CH3>;
});

static CORE1_STACK: StaticCell<CoreStack<16384>> = StaticCell::new();
static EXECUTOR0: StaticCell<Executor> = StaticCell::new();

#[cortex_m_rt::entry]
fn main() -> ! {
    let p = embassy_rp::init(Default::default());
    info!("helic-daq firmware boot: {}", params::FIRMWARE_BANNER);
    let b = board::Board::new(p);

    let channels = shared_rt::init_channels();
    let controller = config::make_controller();
    let store = Store::new(
        channels.command_tx,
        config::SAMPLE_RATE,
        config::EXPERIMENT,
        telemetry::EXTRA_PARAMS,
        &controller,
    );

    spawn_core1(b.core1, CORE1_STACK.init(CoreStack::new()), move || {
        rt_loop::run(b.rt, controller, channels.command_rx, channels.record_tx)
    });

    helic_fw_common::time_watchdog::start();

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
async fn core0_main(
    spawner: Spawner,
    eth: EthernetParts,
    store: Store,
    records: shared_rt::RecordConsumer,
) {
    let stack = net::wiznet::init(spawner, eth, config::MAC_ADDR, config::NET_CONFIG).await;
    spawner.spawn(unwrap!(control_task(stack, store)));
    spawner.spawn(unwrap!(comms::udp::stream_task(stack, records)));
    spawner.spawn(unwrap!(comms::beacon::beacon_task(
        stack,
        config::MAC_ADDR,
        config::EXPERIMENT,
    )));
}

#[embassy_executor::task]
async fn control_task(stack: embassy_net::Stack<'static>, store: Store) -> ! {
    comms::tcp::control_run(stack, store).await
}

#[embassy_executor::task]
async fn blink(mut led: Output<'static>) -> ! {
    loop {
        led.toggle();
        Timer::after_millis(500).await;
    }
}

#[embassy_executor::task]
async fn status_task() -> ! {
    shared_rt::status_run().await
}
