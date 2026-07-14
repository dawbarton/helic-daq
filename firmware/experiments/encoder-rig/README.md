# Encoder rig

The CBC acquisition rig with an additional SSI absolute encoder. It runs on a
W5500-EVB-Pico2 by default, or the pin-compatible W6100-EVB-Pico2 through the
`board-w6100` feature.

## Hardware

- AD7609 eight-channel simultaneous ADC with PWM-driven CONVST and BUSY-edge
  real-time ticks.
- AD5064 four-channel DAC on the core-1 SPI1 bus. All channels are currently
  configured as unipolar to match the interim analogue board.
- RMB20 SSI encoder using PIO0 state machine 0, with clock on GP22 and data on
  GP26. The 13-bit Gray format and 500 kHz clock are provisional.
- Optional optoNCDT laser input on UART0 RX, including the required external
  10 kΩ GP1 pull-up for a disconnected sensor.
- W5500 or W6100 Ethernet on SPI0, plus the GP14 tick-timing output.

The default sample rate is 8 kHz. SSI transfer is pipelined so the encoder
reading is one sample old and the real-time loop never waits for it.

## Control

The statically selected controller is currently `PassThrough`; output channel
0 is used by default. Change `ActiveController` and `make_controller()` in
[`src/config.rs`](src/config.rs) together when selecting another controller.

[`src/board.rs`](src/board.rs) documents the pin map, encoder pipeline and
`Rig` hooks. Consult [notes.md](../../../notes.md) before encoder hardware use.

Build with `cargo build --release -p fw-encoder-rig` from `firmware/`. General
workflows are in the [user guide](../../../docs/user_guide.md), and extension
rules are in the [developer guide](../../../docs/developer_guide.md).
