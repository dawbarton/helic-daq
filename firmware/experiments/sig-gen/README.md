# Wired signal generator

An ADC-free signal generator and laser logger. It runs on a W5500-EVB-Pico2 by
default, with W6100-EVB-Pico2 support selected by the `board-w6100` feature.

## Hardware

- AD5064 four-channel DAC on SPI1, with output channel 0 selected by default.
  All channels are currently configured as unipolar.
- PWM slice 4 wrap interrupt as the hardware-paced 8 kHz real-time tick. No
  CONVST signal or ADC board is required.
- Optional optoNCDT laser input on UART0 RX. GP1 requires an external 10 kΩ
  pull-up when the sensor can be disconnected.
- W5500 or W6100 Ethernet on the board's SPI0 interface.
- GP14 timing output for measuring the real-time tick body.

## Control

`ActiveController` is currently `PassThrough`. The common loop combines its
target output with the Fourier forcing signal and arbitrary waveform table,
then writes the result to the selected DAC channel.

Compile-time settings and controller selection are in
[`src/config.rs`](src/config.rs); wiring and the `Rig` implementation are in
[`src/board.rs`](src/board.rs).

Build with `cargo build --release -p fw-sig-gen` from `firmware/`. See the
[user guide](../../../docs/user_guide.md) and
[developer guide](../../../docs/developer_guide.md) for host use and extension.
