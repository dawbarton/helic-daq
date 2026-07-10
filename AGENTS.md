# CBC-DAQ

A real-time control and data acquisition platform for control-based
continuation (CBC), built on an RP2350 (W5500-EVB-Pico2) with Rust +
Embassy. Successor to the BeagleBone Black-based
[dawbarton/rtc](https://github.com/dawbarton/rtc); see
`docs/implementation_plan.md` §10 for what was carried over and what
changed.

**Aim**: sample up to 8 analogue channels at a jitter-free 1–8 kHz,
run a real-time controller between measurement and output within one
sample period, generate phase-coherent periodic/arbitrary reference
signals, and let a host computer get/set parameters and stream data
live over Ethernet — all while keeping the controller and DSP
swappable at compile time for different CBC experiments.

## Where things are documented

Don't re-derive this from the code — read the docs first:

- `docs/user_guide.md` — what the device does, flashing, connecting,
  CLI/Python usage.
- `docs/developer_guide.md` — architecture, cross-core design, how to
  add a controller/parameter/stream source, timing budget.
- `docs/protocol.md` — the wire protocol, authoritative, with
  known-answer test vectors shared by the Rust and Python codecs.
- `docs/implementation_plan.md` — design rationale and milestone
  history.
- `docs/periodic_signal_generator.md` — the phase-accumulator design
  the generators implement.
- `notes.md` — status of the most recent hardware bring-up session:
  what's been tested on real hardware, what's confirmed working, and
  the current open bug. **Read this before starting a new hardware
  session**; update it when you end one.

## Hardware target

- W5500-EVB-Pico2: RP2350, on-board Wiznet W5500 (Ethernet).
- AD7608: 8-channel, 18-bit simultaneous-sampling ADC, ±5 V/±10 V,
  SPI readout, MCU-timed CONVST, BUSY handshake. Range/oversampling
  set via logic-level GPIO (a future AD7606B would set these over SPI
  instead — kept behind the `AnalogIn` trait for that reason).
- AD5064: 4-channel, 16-bit DAC, SPI. Two channels are bipolar via
  external op-amp stages (±4.096 V), two unipolar (0–4.096 V). A
  future AD5764 swap is kept behind the `AnalogOut` trait.
- Micro-Epsilon optoNCDT 1420 laser displacement sensor via RS422→TTL,
  UART, binary framing. Optional peripheral — firmware must degrade
  gracefully if it's absent (see `notes.md` for a bug in this area
  found during bring-up).
- Software should accommodate other peripherals (e.g. SSI encoders)
  without restructuring — this is why drivers sit behind
  `embedded-hal` traits rather than being wired directly into the RT
  loop.

## Conventions

- **Everything with logic in it must be host-testable.** `cbc-core`
  (DSP), `cbc-drivers` (peripheral drivers), and `cbc-proto` (wire
  protocol) are `no_std` but compile and `cargo test` on the host —
  drivers are generic over `embedded-hal` traits with mock
  implementations, not tied to `embassy-rp` types. `firmware/` is a
  separate Cargo workspace (own `.cargo/config.toml`, always targets
  `thumbv8m.main-none-eabihf`) and stays thin: pin wiring in
  `board.rs`, task plumbing, glue. If you're adding logic and can't
  write a `#[test]` for it on the host, it's probably in the wrong
  crate.
- **No allocation, no `f64` in the real-time path.** The RT loop
  (`firmware/src/rt_loop.rs`, core 1) runs at up to 8 kHz with a
  125 µs budget; the Cortex-M33 FPU is single-precision only, so `f64`
  silently gets software-emulated and will blow the budget. Use
  `heapless` containers, not `alloc`.
- **Compile-time swappable controller.** The active `Controller` is a
  type alias in `firmware/src/config.rs`, not a runtime dispatch —
  this is a deliberate design goal (different CBC experiments need
  different control laws with zero runtime overhead). New controllers
  implement `cbc_core::controller::Controller`; `param_names`/
  `set_param` make their gains host-visible automatically, no
  protocol changes needed.
- **Cross-core communication is lock-free, always.** Core 0 (host
  comms) and core 1 (RT loop) talk only via `heapless::spsc` queues
  and `AtomicU32`/similar statics — never a blocking mutex across
  cores. Parameter writes apply at a sample boundary (array-valued
  parameters travel by value through the queue, so a tick never sees
  a torn write); stream records drop-and-count on overflow rather
  than blocking the RT loop.
- **The wire protocol is discoverable, not hard-coded.** Parameters
  are a name-indexed registry (`firmware/src/params.rs`) the host
  reads at connect (`docs/protocol.md`) — adding a parameter is a
  firmware-only change. Don't hard-code parameter indices on the host
  side.
- **Doc comments explain why, not what.** Default to no comments;
  when you add one, it's because of a non-obvious constraint (a
  datasheet timing requirement, a hardware quirk, a reason a simpler
  approach doesn't work) — not a restatement of the code.
- **Commits are one logical unit each**, referencing the milestone/
  area in the subject line (see `git log` for the established style:
  `<Area>: <what and why>` or `<Area> (milestone N)`), with detail on
  *why* in the body, not a diff summary.
- **Verify before calling something done**: `cargo test` at the repo
  root (host crates), `cd firmware && cargo build --release` (and
  `cargo clippy --release -- -D warnings`), `cd host && PYTHONPATH=.:tests
  python -m unittest discover -s tests`. CI runs all three as separate
  jobs (`.github/workflows/ci.yml`) with `cargo fmt --check` +
  `clippy -D warnings` gating the Rust ones. None of this substitutes
  for actual hardware verification — see `notes.md` for how much of
  this system has and hasn't been run on real hardware yet.
