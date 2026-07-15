//! HELIC-DAQ firmware entry point for the dual-encoder whirl rig.
//!
//! Core 1 owns sampling and estimation; core 0 owns Ethernet control,
//! streaming, discovery, diagnostics and the heartbeat LED.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, Ordering};

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
use heapless::spsc::Queue;
use helic_fw_common::comms;
use helic_fw_common::net;
use helic_fw_common::net::wiznet::EthernetParts;
use helic_fw_common::params::{self, ExtraParam, ParamDef, ParamStore};
use helic_fw_common::rt_loop as shared_rt;
use helic_proto::ParamType;
use panic_probe as _;
use static_cell::StaticCell;

mod board;
mod config;
mod rig;
mod rt_loop;

use rig::WhirlRig;
use rt_loop::{Record, RtCommand, COMMAND_QUEUE_LEN, RECORD_QUEUE_LEN};

type Store = ParamStore<config::ActiveController, WhirlRig>;
const _: () =
    assert!(helic_fw_common::rig::source_count::<WhirlRig>() <= helic_fw_common::rig::MAX_SOURCES);

pub(crate) static PITCH_VALUE: AtomicU32 = AtomicU32::new(0);
pub(crate) static YAW_VALUE: AtomicU32 = AtomicU32::new(0);
pub(crate) static REV_PERIOD_VALUE: AtomicU32 = AtomicU32::new(0);
pub(crate) static RPM_VALUE: AtomicU32 = AtomicU32::new(0);
pub(crate) static SSI_ERRORS: AtomicU32 = AtomicU32::new(0);
pub(crate) static PULSE_COUNT: AtomicU32 = AtomicU32::new(0);
pub(crate) static PULSE_GLITCHES: AtomicU32 = AtomicU32::new(0);
pub(crate) static PULSE_ERRORS: AtomicU32 = AtomicU32::new(0);

fn write_atomic(value: &AtomicU32, out: &mut [u8]) {
    out.copy_from_slice(&value.load(Ordering::Relaxed).to_le_bytes());
}

fn get_pitch(out: &mut [u8]) {
    write_atomic(&PITCH_VALUE, out);
}

fn get_yaw(out: &mut [u8]) {
    write_atomic(&YAW_VALUE, out);
}

fn get_rev_period(out: &mut [u8]) {
    write_atomic(&REV_PERIOD_VALUE, out);
}

fn get_rpm(out: &mut [u8]) {
    write_atomic(&RPM_VALUE, out);
}

fn get_ssi_errors(out: &mut [u8]) {
    write_atomic(&SSI_ERRORS, out);
}

fn get_pulse_count(out: &mut [u8]) {
    write_atomic(&PULSE_COUNT, out);
}

fn get_pulse_glitches(out: &mut [u8]) {
    write_atomic(&PULSE_GLITCHES, out);
}

fn get_pulse_errors(out: &mut [u8]) {
    write_atomic(&PULSE_ERRORS, out);
}

const EXTRA_PARAMS: &[ExtraParam] = &[
    ExtraParam {
        def: ParamDef {
            name: "pitch",
            ty: ParamType::F32,
            count: 1,
            writable: false,
        },
        get: get_pitch,
    },
    ExtraParam {
        def: ParamDef {
            name: "yaw",
            ty: ParamType::F32,
            count: 1,
            writable: false,
        },
        get: get_yaw,
    },
    ExtraParam {
        def: ParamDef {
            name: "rev_period",
            ty: ParamType::F32,
            count: 1,
            writable: false,
        },
        get: get_rev_period,
    },
    ExtraParam {
        def: ParamDef {
            name: "rpm",
            ty: ParamType::F32,
            count: 1,
            writable: false,
        },
        get: get_rpm,
    },
    ExtraParam {
        def: ParamDef {
            name: "ssi_errors",
            ty: ParamType::U32,
            count: 1,
            writable: false,
        },
        get: get_ssi_errors,
    },
    ExtraParam {
        def: ParamDef {
            name: "pulse_count",
            ty: ParamType::U32,
            count: 1,
            writable: false,
        },
        get: get_pulse_count,
    },
    ExtraParam {
        def: ParamDef {
            name: "pulse_glitches",
            ty: ParamType::U32,
            count: 1,
            writable: false,
        },
        get: get_pulse_glitches,
    },
    ExtraParam {
        def: ParamDef {
            name: "pulse_errors",
            ty: ParamType::U32,
            count: 1,
            writable: false,
        },
        get: get_pulse_errors,
    },
];

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
static COMMAND_QUEUE: StaticCell<Queue<RtCommand, COMMAND_QUEUE_LEN>> = StaticCell::new();
static RECORD_QUEUE: StaticCell<Queue<Record, RECORD_QUEUE_LEN>> = StaticCell::new();

#[cortex_m_rt::entry]
fn main() -> ! {
    let p = embassy_rp::init(Default::default());
    info!("helic-daq firmware boot: {}", params::FIRMWARE_BANNER);
    let b = board::Board::new(p);

    let (cmd_tx, cmd_rx) = COMMAND_QUEUE.init(Queue::new()).split();
    let (rec_tx, rec_rx) = RECORD_QUEUE.init(Queue::new()).split();
    let controller = config::make_controller();
    let store = Store::new(
        cmd_tx,
        config::SAMPLE_RATE,
        config::EXPERIMENT,
        EXTRA_PARAMS,
        &controller,
    );

    spawn_core1(b.core1, CORE1_STACK.init(CoreStack::new()), move || {
        rt_loop::run(b.rt, controller, cmd_rx, rec_tx)
    });

    helic_fw_common::time_watchdog::start();

    let executor0 = EXECUTOR0.init(Executor::new());
    executor0.run(|spawner| {
        spawner.spawn(unwrap!(core0_main(spawner, b.eth, store, rec_rx)));
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
