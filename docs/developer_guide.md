# HELIC-DAQ developer guide

How the code is organised, how the real-time architecture works, and how to
extend it. Design rationale and the milestone roadmap live in
[implementation_plan.md](implementation_plan.md); the wire protocol in
[protocol.md](protocol.md).

## Repository layout

Two Cargo workspaces plus a Python package:

| Path | What | Builds for |
|---|---|---|
| `helic-core/` | DSP: phase accumulator, sine LUT, generators, filters, PID, controller trait, Fourier estimator | host + firmware (`no_std`, no alloc) |
| `helic-drivers/` | AD7609, AD5064, optoNCDT drivers over `embedded-hal` 1.0 traits | host + firmware |
| `helic-proto/` | Wire protocol: framing, CRC, stream header, type codes | host + firmware |
| `firmware/common/` | Experiment-independent firmware support | `thumbv8m.main-none-eabihf` only |
| `firmware/experiments/cbc-rig/` | Current CBC rig binary, wiring and compile-time configuration | `thumbv8m.main-none-eabihf` only |
| `host/` | Python package `helic_daq` + `helic-daq` CLI | host |

The split exists so that **everything with logic in it is unit-tested on
the host** (`cargo test` at the root runs ~60 tests; `python -m unittest`
in `host/` another 24). The firmware crate is deliberately thin: pin
wiring, task plumbing, and glue.

## Firmware architecture

```
core 1 (real-time)                       core 0 (everything else)
┌─────────────────────────────┐          ┌───────────────────────────────┐
│ rt_loop task                │          │ TCP control server (:2350)    │
│  PWM slice 4 → CONVST       │ commands │   ParamStore (registry+shadow)│
│  BUSY↓ → SPI read (AD7609)  │◄─────────│ UDP streamer (:2351)          │
│  apply queued commands      │  SPSC    │ laser UART task → atomic      │
│  generators (target+forcing │          │ status task (1 Hz defmt)      │
│  + waveform table)          │          │                               │
│  controller → DAC (AD5064)  │ records  │ embassy-net + W5500 (SPI0)    │
│  diagnostics atomics        │─────────►│ heartbeat LED                 │
└─────────────────────────────┘  SPSC    └───────────────────────────────┘
```

### Timing (the part that matters most)

The AD7609's CONVST pin is driven by **PWM slice 4** as a free-running
output. The sampling instant is therefore crystal-timed — software load
cannot move it. Sample-rate presets map to exact divider/wrap pairs from
the 150 MHz system clock (`config.rs::SampleRate::pwm_params`).

The software pipeline is edge-triggered: `rt_loop` awaits the BUSY falling
edge (conversion complete), then runs

1. SPI read of the 144-bit frame (~12 µs at 12 MHz) and scaling to volts;
2. drain of the command mailbox (parameter updates land here, at a sample
   boundary, never mid-tick);
3. one `PhaseAccumulator::step()`, then evaluation of the **target** and
   **forcing** Fourier series against the same phase (all harmonics of
   both stay locked forever — wrapping-multiply phases, see
   `docs/periodic_signal_generator.md`);
4. `controller.tick(inputs, target, dt) + forcing + table` → rig actuation;
5. a `Record` pushed into the stream ring; diagnostics updated.

A 2-period timeout on the BUSY wait keeps the loop alive (at reduced rate)
with no ADC attached, so bench bring-up works; such ticks increment
`tick_timeouts`.

GP14 is high for the duration of the tick body — put a scope on it to see
processing time and jitter directly.

### Cross-core rules

Core 0 never touches loop state. Three mechanisms, all lock-free:

- **Commands** (core 0 → 1): `heapless::spsc` queue of `RtCommand`.
  Array-valued parameters (coefficient sets) travel **by value** — the
  enqueue/dequeue is the double-buffer swap, so a tick can never observe a
  half-written array.
- **Waveform tables** (core 0 → 1): two fixed 4096-sample buffers. Core 0
  writes only the inactive buffer; `Commit` queues its id and core 1 switches
  at a sample boundary. Further writes remain busy until core 1 publishes the
  new active id, so neither core can access one buffer mutably and immutably
  at the same time.
- **Records** (core 1 → 0): 256-deep `heapless::spsc` ring. The RT loop
  never blocks on it; overflow drops the record and increments
  `records_dropped`.
- **Scalars**: `AtomicU32` statics in `rt_loop.rs` (diagnostics written by
  core 1, laser value written by core 0's UART task and read by the loop).

The analog SPI bus (SPI1: ADC + DAC chip selects) belongs to core 1
exclusively. `board.rs` hands the unassembled `AnalogParts` to core 1,
which builds the shared-bus devices there — that is what lets the bus
mutex be the zero-cost `NoopRawMutex` (it is `!Sync`, so this is also
compiler-enforced).

### Networking (core 0)

`embassy-net-wiznet` drives the W5500 in MACRAW mode over async SPI0;
`embassy-net` (smoltcp) provides the IP stack with the static address from
`config.rs`. Two server tasks:

- `helic_fw_common::comms::tcp::control_run` — accepts one client, reads
  CRC-checked frames, dispatches to `ParamStore`, replies. Framing errors drop the
  connection (no meaningful resync inside TCP). Disconnect stops streaming.
- `helic_fw_common::comms::udp::stream_task` — every 5 ms drains the record
  ring; when a session is active it packs the selected sources into
  ≤1472-byte packets. Session config lives in
  `helic_fw_common::comms::STREAM`, a critical-section mutex shared by the two
  tasks. `StreamStart` bumps a generation counter, which
  re-arms the streamer (sequence reset, finite-capture countdown).

### The parameter registry (`params.rs`)

rtc-style discoverable registry: the host reads names/types/sizes at
connect and uses indices thereafter, so **adding a parameter is a firmware-
only change**. `helic_fw_common::params::ParamStore` serves reads from RT-loop
atomics or from the shadow copies of writable values, and turns writes into
`RtCommand`s.

To add a platform parameter: append a `ParamDef` to `BASE_PARAMS`, add its
index constant, and handle it in `get` (and `set` if writable). Controller
parameters need no registry work at all — see below.

## Extending

### Writing a controller

Implement `helic_core::controller::Controller`:

```rust
pub struct MyController { /* gains, filters, state */ }

impl Controller for MyController {
    fn tick(&mut self, inputs: &[f32], reference: f32, dt: f32) -> f32 {
        // Input slot names and units come from the active rig.
        reference - inputs[0]
    }
    fn reset(&mut self) { /* clear integrators/filters */ }
    fn param_names() -> &'static [&'static str] { &["ctrl_gain"] }
    fn set_param(&mut self, id: u16, value: f32) { /* id indexes param_names */ }
    const TELEMETRY: &'static [(&'static str, &'static str)] = &[("error", "V")];
    fn telemetry(&self, out: &mut [f32]) { /* fill after tick */ }
}
```

Then point `firmware/experiments/cbc-rig/src/config.rs` at it:

```rust
pub type ActiveController = MyController;
pub fn make_controller() -> ActiveController { ... }
```

`param_names` entries appear automatically in the registry (and therefore
in `helic-daq list`) as writable f32 parameters; writes arrive via
`set_param` at a sample boundary. The firmware currently supports up to
eight controller parameters and fails at boot if the active controller exposes
more, so an over-large controller configuration is caught before an
experiment. Everything in `helic-core` is available:
`SosFilter` biquad cascades, `Pid`, `FourierEstimator` (feed it the shared
phase for phase-locked harmonic estimates), and the generators.

Controllers are plain `no_std` structs — write host unit tests next to
them (see `controller.rs` for the pattern of closing the loop around a
simulated plant).

### Budget

At 8 kHz / 150 MHz there are 18,750 cycles per tick; the fixed costs (SPI
read ~12 µs, DAC write ~2 µs, two 16-harmonic series ~2k cycles) leave
roughly half the period for the controller. Check `loop_time_max` and
`overruns` after changes; the GP14 pin shows the same thing on a scope.
Avoid `f64` in the tick path (the M33 FPU is single-precision; doubles are
software-emulated).

### Swapping peripherals

Drivers are generic over `embedded-hal` traits and the `AnalogIn` /
`AnalogOut` traits in `helic-drivers`. An AD7606B (SPI-configured) or AD5764
replacement implements the same trait and slots into `board.rs`; the RT
loop does not change. Pin assignments live **only** in `board.rs`.

### Adding a stream source

Experiment inputs are declared by `Rig::INPUTS`; write their values in the
same order from `Rig::measure`. Controller-internal signals are declared by
`Controller::TELEMETRY` and filled by `telemetry`. The common loop appends
`target`, `forcing`, `table` and `out`, so neither rigs nor controllers manage
numeric slots. Protocol-v2 source discovery exposes this assembled table to
the host at every connection.

## Hardware bring-up notes

The acquisition + generation + closed-loop-control chain is **verified on
hardware** (interim rtc analog cape, 2026-07; details and gotchas in
`notes.md` §4.3). Reference points when re-checking on a new assembly or scope:

- AD7609 SPI mode 2 at 12 MHz (readout after BUSY↓) — verified; raise the clock
  only after clean captures.
- AD5064 SPI mode 1 at 16 MHz; the part wants ~3 µs between consecutive
  words — currently only one channel is written per tick, so this only
  matters for multi-channel output work.
- CONVST duty is 50%; only the rising edge is meaningful.
- The `sine` CLI command plus a scope on output 0 exercises the whole
  chain: TCP → registry → mailbox → generator → DAC.

## Testing and CI

```sh
cargo test                                  # root: helic-core/drivers/proto (~60 tests)
cd firmware && cargo build --release --workspace # all experiment binaries
cd host && PYTHONPATH=.:tests python -m unittest discover -s tests
```

CI (GitHub Actions) runs fmt + clippy `-D warnings` + tests for the host
crates, the firmware cross-build, and the Python suite. The Rust and
Python protocol implementations share known-answer vectors
(`docs/protocol.md`) so codec drift fails tests on both sides.

Flashing/debugging: `cargo run --release -p fw-cbc-rig` in `firmware/` uses probe-rs
(`--chip RP235x`) and streams defmt logs over RTT; `DEFMT_LOG` is set in
`firmware/.cargo/config.toml`. Without a probe, build a UF2 with picotool
(see the user guide).

## Known gaps / next steps

- End-to-end behaviour is **verified on hardware** (2026-07): networking, RT
  loop, ADC read, DAC write, DAC→ADC loopback (DC + AC), signal generator, all
  four sample-rate presets, parameter round-trip, and closed-loop PID. The only
  path not yet exercised is the **laser UART with a real optoNCDT sensor**
  (needs the physical sensor). See `notes.md` §4.3/§5.
- Arbitrary table upload and playback are implemented and host-tested, but
  still require scope verification on hardware, including glitch-free
  re-commit and long phase-locked runs.
- USB serial as a second transport, flash-persisted configuration, laser
  TX (sensor configuration from firmware), and per-period Fourier
  statistics are planned extensions — see implementation_plan.md §8/§10.
