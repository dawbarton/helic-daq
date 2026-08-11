//! Minimal RP2350 binary composed entirely from public HELIC-DAQ contracts.

#![no_std]
#![no_main]

use cortex_m_rt::entry;
use defmt_rtt as _;
use fixture_rig_program::FixtureProgram;
use helic_core::{DoubleBuffer, FourierCoeffs};
use helic_fw_rt::rt_loop;
use helic_rt::{Rig, RtShared, SampleRate, TickSource};
use panic_probe as _;
use static_cell::ConstStaticCell;

static SHARED: RtShared = RtShared::new();
static TARGET: ConstStaticCell<DoubleBuffer<FourierCoeffs<1>>> = ConstStaticCell::new(
    DoubleBuffer::from_banks(FourierCoeffs::zero(), FourierCoeffs::zero()),
);
static FORCING: ConstStaticCell<DoubleBuffer<FourierCoeffs<1>>> = ConstStaticCell::new(
    DoubleBuffer::from_banks(FourierCoeffs::zero(), FourierCoeffs::zero()),
);

struct FixtureRig;

impl Rig for FixtureRig {
    const INPUTS: &'static [(&'static str, &'static str)] = &[("sense", "V")];
    const ACTUATORS: &'static [(&'static str, &'static str)] = &[("drive", "V")];

    #[unsafe(link_section = ".data.ram_func")]
    fn init(&mut self) {}

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

struct FixtureTick;

impl TickSource for FixtureTick {
    #[unsafe(link_section = ".data.ram_func")]
    fn wait(&mut self) -> bool {
        true
    }
}

#[entry]
fn main() -> ! {
    let channels = rt_loop::init_channels(TARGET.take(), FORCING.take());
    rt_loop::run_rt_loop(
        FixtureRig,
        FixtureTick,
        FixtureProgram::new(),
        SampleRate::Hz1000,
        &SHARED,
        channels.command_rx,
        channels.record_tx,
    )
}
