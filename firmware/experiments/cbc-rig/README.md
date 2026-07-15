# CBC rig

The reference HELIC-DAQ control and acquisition experiment. It runs on a
W5500-EVB-Pico2 by default, with the pin-compatible W6100-EVB-Pico2 available
through the `board-w6100` feature.

## Hardware

- AD7609 eight-channel simultaneous ADC on SPI1. PWM-driven CONVST supplies
  the hardware sample clock and the BUSY falling edge starts each RT tick.
- AD5064 four-channel DAC on the same core-1 SPI bus. The current configuration
  treats all channels as unipolar; this must match the fitted output stages.
- Optional optoNCDT laser input on UART0 RX. A disconnected input requires the
  external 10 kΩ pull-up on GP1 described in the project notes.
- W5500 or W6100 Ethernet on the board's SPI0 interface.
- GP14 timing output, high while the real-time tick body executes.

The default sample rate is 8 kHz and output channel 0 is selected.

## Control

`ActiveController` is currently `PassThrough`, selected statically in
[`src/config.rs`](src/config.rs). The generated target passes through the
controller; forcing and arbitrary-table signals are then added by the common
RT loop. Replace both `ActiveController` and `make_controller()` to use another
controller.

[`src/board.rs`](src/board.rs) is the complete pin and ownership map;
[`src/rig.rs`](src/rig.rs) contains CBC assembly and behaviour;
[`src/telemetry.rs`](src/telemetry.rs) declares shared scalar state; and
[`src/main.rs`](src/main.rs) assigns tasks and cores.

Build with `cargo build --release -p fw-cbc-rig` from `firmware/`. See the
[user guide](../../../docs/user_guide.md) for operation and the
[developer guide](../../../docs/developer_guide.md) for extension rules.
