"""Host interface to the HELIC-DAQ real-time control and DAQ platform."""

from importlib.metadata import PackageNotFoundError, version

from . import protocol as protocol
from .device import Device, DeviceError
from .discovery import find_devices
from .stream import StreamReceiver

__all__ = ["Device", "DeviceError", "StreamReceiver", "find_devices"]

try:
    __version__ = version("helic-daq")
except PackageNotFoundError:
    __version__ = "0+unknown"
