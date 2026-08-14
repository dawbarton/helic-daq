"""UDP discovery for HELIC-DAQ devices on local IPv4 interfaces."""

from __future__ import annotations

import select
import socket
import time
from dataclasses import dataclass

from . import protocol


@dataclass(frozen=True)
class DiscoveredDevice:
    address: str
    version: int
    control_port: int
    mac: str
    experiment: str
    firmware: str


def _local_addresses() -> list[str]:
    addresses = {"0.0.0.0", "127.0.0.1"}
    try:
        for result in socket.getaddrinfo(
            socket.gethostname(), 0, socket.AF_INET, socket.SOCK_DGRAM
        ):
            addresses.add(result[4][0])
    except OSError:
        pass
    return sorted(addresses)


def _open_sockets(addresses: list[str]) -> list[socket.socket]:
    sockets = []
    for address in addresses:
        sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        try:
            sock.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
            sock.bind((address, 0))
        except OSError:
            sock.close()
            continue
        sockets.append(sock)
    return sockets


def find_devices(
    timeout: float = 1.0,
    port: int = protocol.DISCOVERY_PORT,
    addresses: list[str] | None = None,
) -> list[DiscoveredDevice]:
    """Broadcast a query and return unique responses received before timeout."""
    local = ["0.0.0.0"] if addresses else _local_addresses()
    sockets = _open_sockets(local)
    targets = addresses or ["255.255.255.255", "127.0.0.1"]
    try:
        for sock in sockets:
            for target in targets:
                try:
                    sock.sendto(protocol.BEACON_REQUEST, (target, port))
                except OSError:
                    pass

        found: dict[tuple[str, bytes], DiscoveredDevice] = {}
        deadline = time.monotonic() + timeout
        while sockets:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                break
            readable, _, _ = select.select(sockets, [], [], remaining)
            if not readable:
                break
            for sock in readable:
                try:
                    payload, peer = sock.recvfrom(128)
                    beacon = protocol.decode_beacon_response(payload)
                except (OSError, protocol.ProtocolError):
                    continue
                found[(peer[0], beacon.mac)] = DiscoveredDevice(
                    address=peer[0],
                    version=beacon.version,
                    control_port=beacon.control_port,
                    mac=":".join(f"{byte:02x}" for byte in beacon.mac),
                    experiment=beacon.experiment,
                    firmware=beacon.firmware,
                )
        return sorted(found.values(), key=lambda device: (device.address, device.mac))
    finally:
        for sock in sockets:
            sock.close()
