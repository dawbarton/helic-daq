//! Host communications (core 0): W5500 Ethernet bring-up, the TCP control
//! server, and the UDP sample streamer.
//!
//! The W5500 is driven in MACRAW mode by `embassy-net-wiznet`; `embassy-net`
//! (smoltcp) provides the IP stack using configuration supplied by the
//! experiment crate.

pub mod tcp;
pub mod udp;

use core::cell::RefCell;

use defmt::{info, unwrap};
use embassy_executor::Spawner;
use embassy_net::{Ipv4Address, Ipv4Cidr, Stack, StackResources};
use embassy_net_wiznet::chip::W5500;
use embassy_net_wiznet::{Device, Runner as WiznetRunner, State as WiznetState};
use embassy_rp::clocks::RoscRng;
use embassy_rp::gpio::{Input, Output};
use embassy_rp::peripherals::SPI0;
use embassy_rp::spi::{Async, Spi};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex;
use embassy_time::Delay;
use embedded_hal_bus::spi::ExclusiveDevice;
use heapless::Vec;
use static_cell::StaticCell;

type EthSpi = ExclusiveDevice<Spi<'static, SPI0, Async>, Output<'static>, Delay>;
type EthRunner = WiznetRunner<'static, W5500, EthSpi, Input<'static>, Output<'static>>;

/// Maximum number of stream sources in one record.
pub const MAX_STREAM_SOURCES: usize = 16;

/// Stream session state shared between the TCP server (writer) and the UDP
/// streamer (reader). Both tasks live on core 0.
pub struct StreamState {
    /// Stream target; `None` until a `StreamStart` arrives.
    pub target: Option<(Ipv4Address, u16)>,
    pub enabled: bool,
    /// Source ids (helic_proto::source) in record order.
    pub sources: Vec<u8, MAX_STREAM_SOURCES>,
    /// Keep every n-th sample (>= 1).
    pub decimation: u16,
    /// Records to send before auto-stop; 0 = continuous.
    pub count: u32,
    /// Incremented by every `StreamStart`; the streamer re-arms on change.
    pub generation: u32,
}

pub static STREAM: Mutex<CriticalSectionRawMutex, RefCell<StreamState>> =
    Mutex::new(RefCell::new(StreamState {
        target: None,
        enabled: false,
        sources: Vec::new(),
        decimation: 1,
        count: 0,
        generation: 0,
    }));

static WIZNET_STATE: StaticCell<WiznetState<8, 8>> = StaticCell::new();
static RESOURCES: StaticCell<StackResources<8>> = StaticCell::new();

/// An assembled W5500 SPI interface. Pin selection remains in the experiment
/// board module; common code owns only network bring-up.
pub struct EthernetParts {
    pub spi: Spi<'static, SPI0, Async>,
    pub cs: Output<'static>,
    pub int: Input<'static>,
    pub rst: Output<'static>,
}

#[derive(Clone, Copy)]
pub struct StaticNetConfig {
    pub mac: [u8; 6],
    pub addr: [u8; 4],
    pub prefix: u8,
}

/// Bring up Ethernet and the IP stack; returns the stack handle for the
/// server tasks. Must be called from the core-0 executor.
pub async fn init(
    spawner: Spawner,
    parts: EthernetParts,
    config: StaticNetConfig,
) -> Stack<'static> {
    let spi_dev = unwrap!(ExclusiveDevice::new(parts.spi, parts.cs, Delay));
    info!("comms::init: SPI0 device ready, starting W5500 init (reset + register access)");

    let (device, runner) = embassy_net_wiznet::new(
        config.mac,
        WIZNET_STATE.init(WiznetState::new()),
        spi_dev,
        parts.int,
        parts.rst,
    )
    .await
    .expect("W5500 init failed");
    info!("comms::init: W5500 responded, spawning ethernet_task");
    spawner.spawn(unwrap!(ethernet_task(runner)));

    let ip = config.addr;
    let net_config = embassy_net::Config::ipv4_static(embassy_net::StaticConfigV4 {
        address: Ipv4Cidr::new(Ipv4Address::new(ip[0], ip[1], ip[2], ip[3]), config.prefix),
        gateway: None,
        dns_servers: Vec::new(),
    });

    let seed = RoscRng.next_u64();
    let (stack, net_runner) = embassy_net::new(
        device,
        net_config,
        RESOURCES.init(StackResources::new()),
        seed,
    );
    spawner.spawn(unwrap!(net_task(net_runner)));

    info!(
        "network up: {}.{}.{}.{}/{} (control tcp:{}, stream udp)",
        ip[0],
        ip[1],
        ip[2],
        ip[3],
        config.prefix,
        helic_proto::CONTROL_PORT,
    );
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
