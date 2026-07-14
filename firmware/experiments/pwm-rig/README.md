# Filtered-PWM rig

An ADC-free signal generator which replaces the external DAC with an RP2350
PWM output. It runs on a W5500-EVB-Pico2 by default, or a W6100-EVB-Pico2 via
the `board-w6100` feature.

## Hardware

- PWM slice 5 channel A on GP10, configured for 10-bit duty resolution and a
  carrier of approximately 146 kHz. The mapped output range is 0 to 3.3 V.
- An external RC or active low-pass filter is required to reconstruct an
  analogue voltage; firmware does not filter the carrier.
- PWM slice 4 wrap interrupt provides the independent 8 kHz RT sample clock.
- Optional optoNCDT laser input on UART0 RX, with an external 10 kΩ GP1 pull-up
  required for safe disconnection.
- W5500 or W6100 Ethernet on SPI0 and a GP14 tick-timing output.

## Control

The statically selected controller is `PassThrough`. The generated target,
forcing and arbitrary-table contributions are combined by the common loop and
mapped to PWM duty by the rig.

[`src/config.rs`](src/config.rs) defines the voltage range, controller and
sample rate. [`src/board.rs`](src/board.rs) contains the wiring and `Rig`
implementation.

Build with `cargo build --release -p fw-pwm-rig` from `firmware/`. Electrical
limitations are covered in the [user guide](../../../docs/user_guide.md), and
the architecture is described in the
[developer guide](../../../docs/developer_guide.md).
