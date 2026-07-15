# Whirl rig

The whirl rig acquires two RMB20SC12BC96 absolute encoders and one optical
revolution pulse. It runs on a W5500-EVB-Pico2 by default, or the
pin-compatible W6100-EVB-Pico2 through the `board-w6100` feature.

## Hardware

- A shared 1 MHz SSI clock on GP22, fanned out through the two RS422 clock
  transmitters.
- Pitch SSI data on GP26 and yaw SSI data on GP27. PIO0 state machine 0
  samples both 12-bit natural-binary inputs in the same instruction.
- An active-high 3.3 V optical revolution pulse on GP28. PIO0 state machine 1
  measures rising-edge intervals with 1 µs resolution.
- W5500 or W6100 Ethernet on SPI0, plus the GP14 tick-timing output.

The mandatory real-time loop runs synchronously from the raw PWM-wrap
latch at 2 kHz, with no Embassy executor on core 1. The loop and its PIO FIFO,
SSI decoding and RPM-estimator path execute from SRAM. SSI acquisition is
pipelined with fixed one-sample latency. The raw optical period and a 250 ms
time-normalised EWMA RPM estimate are streamed with pulse and validity flags.
RPM becomes stale after 100 ms without an accepted pulse.

The Fourier generators, waveform table and static controller selection remain
available for future output hardware. The current rig has no ADC, DAC or laser
and its `actuate` hook is a no-op.

Build with `cargo build --release -p fw-whirl-rig` from `firmware/`. General
workflows are in the [user guide](../../../docs/user_guide.md), and extension
rules are in the [developer guide](../../../docs/developer_guide.md). Consult
[notes.md](../../../notes.md) before hardware use.

`src/board.rs` is only the pin and ownership map. `src/rig.rs` contains PIO
assembly and measurement semantics, while `src/telemetry.rs` declares the
latest-value and diagnostic atomics exposed to the host.
