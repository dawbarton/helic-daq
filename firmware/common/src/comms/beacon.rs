//! UDP responder for zero-configuration device discovery.

use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::Stack;
use helic_proto::beacon::{BeaconResponse, REQUEST, RESPONSE_LEN};

#[embassy_executor::task]
pub async fn beacon_task(stack: Stack<'static>, mac: [u8; 6], experiment: &'static str) -> ! {
    let mut rx_meta = [PacketMetadata::EMPTY; 2];
    let mut rx_buffer = [0; 64];
    let mut tx_meta = [PacketMetadata::EMPTY; 2];
    let mut tx_buffer = [0; 2 * RESPONSE_LEN];
    let mut socket = UdpSocket::new(
        stack,
        &mut rx_meta,
        &mut rx_buffer,
        &mut tx_meta,
        &mut tx_buffer,
    );
    socket
        .bind(helic_proto::DISCOVERY_PORT)
        .expect("discovery UDP bind failed");

    let response = BeaconResponse::new(mac, experiment, crate::params::FIRMWARE_VERSION);
    let mut encoded = [0; RESPONSE_LEN];
    response.encode(&mut encoded);
    let mut request = [0; 64];

    loop {
        let Ok((length, peer)) = socket.recv_from(&mut request).await else {
            continue;
        };
        if request[..length] == REQUEST {
            let _ = socket.send_to(&encoded, peer).await;
        }
    }
}
