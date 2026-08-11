//! WIZnet transport backend for W5500-EVB-Pico2 and W6100-EVB-Pico2 experiments.

#[cfg(all(feature = "net-wiznet-w5500", feature = "net-wiznet-w6100"))]
compile_error!("select exactly one WIZnet chip feature");
#[cfg(not(any(feature = "net-wiznet-w5500", feature = "net-wiznet-w6100")))]
compile_error!("select a WIZnet chip feature");

use defmt::{info, unwrap};
use embassy_executor::Spawner;
use embassy_net::Stack;
#[cfg(feature = "net-wiznet-w5500")]
use embassy_net_wiznet::chip::W5500 as WiznetChip;
#[cfg(feature = "net-wiznet-w6100")]
use embassy_net_wiznet::chip::W6100 as WiznetChip;
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
type EthRunner = WiznetRunner<'static, WiznetChip, EthSpi, Input<'static>, Output<'static>>;

#[cfg(feature = "net-wiznet-w5500")]
const CHIP_NAME: &str = "W5500";
#[cfg(feature = "net-wiznet-w6100")]
const CHIP_NAME: &str = "W6100";

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
    info!("network: starting {}", CHIP_NAME);
    let (device, runner) = embassy_net_wiznet::new(
        mac,
        WIZNET_STATE.init(WiznetState::new()),
        spi_dev,
        parts.int,
        parts.rst,
    )
    .await
    .expect("WIZnet init failed");
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
