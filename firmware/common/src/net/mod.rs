//! Transport-independent IPv4 configuration and stack allocation.

#[cfg(feature = "net-cyw43")]
pub mod cyw43;
#[cfg(feature = "net-wiznet")]
pub mod wiznet;

#[cfg(any(feature = "net-cyw43", feature = "net-wiznet"))]
use embassy_net::driver::Driver;
#[cfg(any(feature = "net-cyw43", feature = "net-wiznet"))]
use embassy_net::{Ipv4Address, Ipv4Cidr, Stack, StackResources};
#[cfg(any(feature = "net-cyw43", feature = "net-wiznet"))]
use heapless::Vec;
#[cfg(any(feature = "net-cyw43", feature = "net-wiznet"))]
use static_cell::StaticCell;

#[cfg(any(feature = "net-cyw43", feature = "net-wiznet"))]
static RESOURCES: StaticCell<StackResources<8>> = StaticCell::new();

#[derive(Clone, Copy)]
pub enum NetConfig {
    Static { address: [u8; 4], prefix: u8 },
    Dhcp,
}

#[cfg(any(feature = "net-cyw43", feature = "net-wiznet"))]
pub(crate) fn new<D: Driver>(
    device: D,
    config: NetConfig,
    seed: u64,
) -> (Stack<'static>, embassy_net::Runner<'static, D>) {
    let config = match config {
        NetConfig::Static { address, prefix } => {
            embassy_net::Config::ipv4_static(embassy_net::StaticConfigV4 {
                address: Ipv4Cidr::new(
                    Ipv4Address::new(address[0], address[1], address[2], address[3]),
                    prefix,
                ),
                gateway: None,
                dns_servers: Vec::new(),
            })
        }
        NetConfig::Dhcp => embassy_net::Config::dhcpv4(Default::default()),
    };
    embassy_net::new(device, config, RESOURCES.init(StackResources::new()), seed)
}
