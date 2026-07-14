//! W5500 transport backend for W5500-EVB-Pico2 experiments.

use defmt::{info, unwrap};
use embassy_executor::Spawner;
use embassy_net::Stack;
use embassy_net_wiznet::chip::W5500;
use embassy_net_wiznet::{Device, Runner as WiznetRunner, State as WiznetState};
use embassy_rp::clocks::RoscRng;
use embassy_rp::gpio::{Input, Output};
use embassy_rp::peripherals::SPI0;
use embassy_rp::spi::{Async, Spi};
use embassy_time::Delay;
use embedded_hal_bus::spi::ExclusiveDevice;
use static_cell::StaticCell;

use super::NetConfig;

type EthSpi = ExclusiveDevice<Spi<'static, SPI0, Async>, Output<'static>, Delay>;
type EthRunner = WiznetRunner<'static, W5500, EthSpi, Input<'static>, Output<'static>>;

static WIZNET_STATE: StaticCell<WiznetState<8, 8>> = StaticCell::new();

pub struct EthernetParts {
    pub spi: Spi<'static, SPI0, Async>,
    pub cs: Output<'static>,
    pub int: Input<'static>,
    pub rst: Output<'static>,
}

pub async fn init(
    spawner: Spawner,
    parts: EthernetParts,
    mac: [u8; 6],
    config: NetConfig,
) -> Stack<'static> {
    let spi_dev = unwrap!(ExclusiveDevice::new(parts.spi, parts.cs, Delay));
    info!("network: starting W5500");
    let (device, runner) = embassy_net_wiznet::new(
        mac,
        WIZNET_STATE.init(WiznetState::new()),
        spi_dev,
        parts.int,
        parts.rst,
    )
    .await
    .expect("W5500 init failed");
    spawner.spawn(unwrap!(ethernet_task(runner)));

    let (stack, runner) = super::new(device, config, RoscRng.next_u64());
    spawner.spawn(unwrap!(net_task(runner)));
    stack.wait_config_up().await;
    info!("network: IPv4 configuration acquired");
    stack
}

#[embassy_executor::task]
async fn ethernet_task(runner: EthRunner) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn net_task(mut runner: embassy_net::Runner<'static, Device<'static>>) -> ! {
    runner.run().await
}
