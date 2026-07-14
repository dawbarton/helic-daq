# Wi-Fi signal generator

The Pico 2W variant of the ADC-free signal generator. It runs on a Raspberry
Pi Pico 2W and uses its on-board CYW43439 radio instead of a WIZnet Ethernet
controller.

## Hardware

- CYW43439 Wi-Fi interface using the Pico 2W's fixed PIO1, DMA0 and
  GP23/24/25/29 resources. The on-board LED is controlled through the radio.
- AD5064 four-channel DAC on SPI1, with channel 0 selected by default and all
  channels currently configured as unipolar.
- PWM slice 4 wrap interrupt as the hardware-paced 8 kHz real-time tick. No
  ADC board is required.
- Optional optoNCDT laser input on UART0 RX, including the external 10 kΩ GP1
  pull-up needed when disconnected.
- GP14 timing output for measuring real-time execution.

## Control and network

`ActiveController` is currently `PassThrough`; the common RT loop adds forcing
and arbitrary-table signals before DAC actuation. Wi-Fi uses station mode and
DHCP by default. Set `WIFI_SSID` and `WIFI_PASSWORD` in
[`src/config.rs`](src/config.rs) before building, without committing real
credentials.

[`src/board.rs`](src/board.rs) contains the Pico 2W resource assignment and
`Rig` implementation.

Build with `cargo build --release -p fw-sig-gen-w` from `firmware/`. Use wired
experiments for sustained full-rate multi-source streaming; see the
[user guide](../../../docs/user_guide.md) and
[developer guide](../../../docs/developer_guide.md) for details.
