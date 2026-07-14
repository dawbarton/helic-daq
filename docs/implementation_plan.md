# HELIC-DAQ implementation plan

Informed by a review of the previous BeagleBone Black implementation
([dawbarton/rtc](https://github.com/dawbarton/rtc)); see §10 for what was
carried over and what was deliberately changed.

Decisions agreed so far (2026-07-09):

- **Feedback path:** configurable — the controller receives a full measurement frame (all 8 ADC channels + latest laser value) and each controller implementation picks what it uses. The OptoNCDT runs at up to 8 kHz.
- **Loop latency:** per-sample. ADC read → control computation → DAC write completes within one sample period.
- **Comms:** Ethernet first (USB serial later behind the same protocol layer). Static IP. TCP for parameter get/set and commands; UDP for high-rate streaming with sequence numbers.
- **Host tooling:** Python library + CLI is part of this project.
- **Pin map:** proposed below; hardware to be wired to match.
- **Debug setup:** none yet — recommendation below.

## 1. System architecture

### Core split

- **Core 0** — Embassy async executor: W5500 Ethernet (embassy-net + `embassy-net-wiznet`, MACRAW mode), TCP command server, UDP streamer, laser UART parsing, and later USB serial. Nothing on core 0 is timing-critical.
- **Core 1** — real-time loop only. Spawned via `embassy_rp::multicore::spawn_core1`. The sample path runs at the highest interrupt priority; a minimal executor handles core-1 housekeeping (applying parameter updates is done at sample boundaries, see §5).

### Sample-tick timing (the low-jitter trick)

The AD7609 CONVST pulse is generated **directly by an RP2350 PWM slice** at the sample rate. Conversion start is therefore hardware-timed from the crystal — zero software jitter on the sampling instant, which is what dominates measurement quality. The software pipeline then runs per tick:

```
PWM (hardware) ─ CONVST ↑ ──► AD7609 converts (~4 µs, no oversampling)
BUSY ↓ (GPIO IRQ, core 1, highest priority)
  ├─ SPI read 8×18-bit frame            (~10 µs @ ~15 MHz)
  ├─ scale to f32 volts; snapshot laser value (atomic read)
  ├─ periodic + arbitrary signal generators (phase accumulators)
  ├─ controller.update(frame, reference) → output(s)
  ├─ AD5064 SPI write (up to 4 ch × 24-bit @ 20 MHz, ~5 µs)
  ├─ optional Fourier estimation update
  ├─ update diagnostics (loop execution time last/max, jitter, overrun count)
  └─ push selected record into stream ring buffer (lock-free SPSC)
```

Diagnostics are ordinary registered parameters (see §5a) readable by the host
at any time — carried over from rtc's `clock_jitter` / `overrun` /
`user_max_time`, which proved invaluable in practice.

Budget at 8 kHz / 150 MHz: 125 µs ≈ 18,750 cycles per tick; the above is comfortably under 30% including a 20-harmonic generator (~1,000 cycles per the signal-generator doc). DAC update time has small software-dependent jitter (a few hundred ns); acceptable since the sampling instant is hardware-locked. A spare GPIO is toggled at tick start/end for scope-based jitter/load verification.

Sample-rate presets: **1 / 2 / 4 / 8 kHz**, selected by PWM divider constants. (10 kHz can be added later; 8 kHz is the initial ceiling to match the laser.)

### Cross-core data flow (no locks on the RT path)

- **Core 1 → Core 0 (stream data):** lock-free SPSC ring buffer (`heapless::spsc` or `bbqueue`) in shared SRAM. Core 0 drains it into UDP packets. Overrun policy: drop-oldest with a dropped-count reported in stream headers.
- **Core 0 → Core 1 (parameter updates):** command mailbox (small SPSC queue). Core 1 applies pending updates at a sample boundary, so a tick never sees half-updated state. Array parameters (Fourier coefficient sets, filter coefficient sets, arbitrary-signal LUT) are **double-buffered**: core 0 fills the inactive buffer, then a single atomic index swap makes it live.
- **Laser → Core 1:** core 0 parses the UART stream and publishes latest value + sample-index timestamp via an atomic cell; core 1 snapshots it each tick. At matched rates (8 kHz) this gives at most one sample of staleness.

## 2. Workspace layout

```
helic-daq/
├── firmware/        # RP2350 Embassy binary (thumbv8m.main-none-eabihf)
│   └── src/
│       ├── main.rs            # core 0: net, protocol, laser
│       ├── rt_loop.rs         # core 1: sample tick pipeline
│       ├── board.rs           # pin map, SPI/UART/PWM setup (single place to rewire)
│       ├── config.rs          # ActiveController type alias, presets, static IP
│       ├── params.rs          # name-based parameter registry (§5a)
│       ├── drivers/           # ad7609.rs, ad5064.rs, optoncdt.rs
│       └── comms/             # tcp_server.rs, udp_stream.rs, (later usb.rs)
├── helic-core/        # no_std, no-alloc DSP library — host-testable with cargo test
│   └── src/         # controllers, filters, generators, fourier, frame types
├── helic-proto/       # no_std wire protocol: message layouts, param IDs, framing
├── host/            # Python package `helic_daq` + CLI
└── docs/
```

`helic-core` and `helic-proto` compile on std too, so all DSP and protocol logic gets ordinary unit tests on the host — the firmware crate stays thin.

## 3. Hardware abstraction & drivers

Traits keep future part swaps localized:

- `trait AnalogIn` — `start_conversion` handled by hardware (PWM); driver exposes `read_frame()`. Implemented by `Ad7609` (range/oversampling via GPIO); a future `Ad7606b` implements the same trait with SPI register configuration.
- `trait AnalogOut` — `write(channel_outputs)`. Implemented by `Ad5064` (per-channel unipolar/bipolar scaling, since two channels pass through inverting op-amp stages); future `Ad5764` swaps in.
- `trait Displacement` (or a generic "aux sensor" slot) — implemented by `OptoNcdt1420`; leaves room for SSI encoders via PIO later.

## 4. Proposed pin map (W5500-EVB-Pico2)

The W5500 occupies SPI0 (GP16 MISO, GP17 CSn, GP18 SCK, GP19 MOSI, GP20 RSTn, GP21 INTn). Proposal for the rest — all in `board.rs`, trivially re-wireable:

| Function | Pin | Notes |
|---|---|---|
| Laser UART0 TX / RX | GP0 / GP1 | 921,600 baud for 8 kHz output rate |
| AD7609 OS0 / OS1 / OS2 | GP2 / GP3 / GP4 | oversampling select |
| AD7609 RANGE | GP5 | ±10 V / ±20 V |
| AD7609 RESET | GP6 | pulsed at init |
| AD7609 BUSY | GP7 | falling-edge IRQ, core 1 |
| AD7609 CONVST A+B | GP8 | PWM slice 4A output = sample clock |
| AD5064 ~SYNC | GP9 | |
| SPI1 SCK / MOSI / MISO | GP10 / GP11 / GP12 | shared bus, ADC + DAC |
| AD7609 ~CS | GP13 | |
| Tick-timing debug pin | GP14 | scope verification of jitter/load |
| AD5064 ~LDAC | GP15 | or tie low for immediate update |
| Spare / future SSI (PIO) | GP22, GP26–28 | |

ADC and DAC share SPI1 with separate chip selects; transactions are sequential within a tick. If mode/baud reconfiguration between the two proves costly, the DAC moves to a PIO-based SPI (RP2350 has PIO to spare).

## 5. Control & DSP (`helic-core`)

- **`trait Controller`**: `fn tick(&mut self, m: &Frame, r: &Reference) -> Outputs` plus parameter set/get hooks. The active implementation is chosen **at compile time** via a type alias in `config.rs` (optionally behind Cargo features). Built-ins: `PassThrough` (open loop), `Pid` (with derivative filtering and anti-windup); the structure documents how users add their own.
- **Filters**: cascaded biquads (f32, Direct Form II transposed), coefficients host-settable.
- **Periodic signal generator**: exactly per `docs/periodic_signal_generator.md` — 32-bit phase accumulator (64-bit optional), wrapping-multiply harmonic phases, 1024-entry interpolated sine LUT, 5–20 harmonics (const-generic cap), atomic increment update for glitch-free frequency changes. The generator exposes a `period_start` flag (phase wrap) so per-period processing can hook onto it.
- **Arbitrary signal generator**: 1000–2000-sample f32 LUT with linear interpolation, its own phase accumulator for timescale adjustment, single-shot (arm/trigger, hold last value at end) or periodic mode.
- **Fourier estimation**: coherent demodulation against the generator's own phase accumulator (multiply by LUT sin/cos of `k·phase`, low-pass/average) — harmonics stay phase-locked to the forcing by construction, which is exactly what CBC needs.
- **Designed-for but not built initially** (rtc's duffing rig has these; the APIs above must leave room): per-period Fourier coefficient updates with running mean/variance over the last N periods (steady-state detection), and a filtered random perturbation generator for system ID. The `period_start` hook and the estimator's trait-style interface are the accommodation points.

All of this is pure `no_std` f32 code with host unit tests (generator spectral purity, PID step responses, estimator convergence).

## 5a. Parameter registry (adopted from rtc)

A name-based, self-describing registry rather than a hard-coded ID table.
Controllers and platform code register parameters at init:

```rust
registry.add("x_Kp", ParamRef::F32(&PID_KP), Access::ReadWrite)?;   // conceptually
```

- Each entry: null-terminated **name**, **type code** (mirroring Python `struct`
  codes, as rtc did: `f`, `i`, `I`, …), **element count** (arrays allowed), and
  **access class** (read-only, live-write, or block-write).
- The host queries `GetParNames` / `GetParTypes` / `GetParSizes` once at
  connect and addresses parameters by **registry index** thereafter — adding a
  parameter to a controller is one registration line, with zero protocol or
  host-code changes.
- Fixed-capacity static table (`heapless`), no allocation.
- **Consistency (fixing rtc's weakness):** rtc wrote through raw pointers into
  live RT variables, guarded by a spin-wait that narrows but doesn't close the
  race. Here, scalar live-writes go through the core-0→core-1 mailbox and are
  applied at a sample boundary; array/block parameters (Fourier coefficient
  sets, filter coefficients, arbitrary LUT) are double-buffered with an atomic
  swap on `Commit`. Reads are served from a per-tick snapshot, so a multi-value
  read is always sample-coherent.
- Built-in platform parameters: firmware version, sample rate, sample counter,
  input voltage range, raw + volts per channel, DAC outputs, laser value, and
  the diagnostics set (`clock_jitter`, `overrun`, `loop_time_last`,
  `loop_time_max`).

## 6. Host communication

### Protocol (`helic-proto`, documented in `docs/protocol.md`)

Hand-written fixed-layout little-endian binary (trivially parseable with Python `struct`; no serde dependency on the wire).

- **TCP :2350 — control.** Length-prefixed frames: `magic u16 | type u8 | seq u8 | len u16 | payload | crc16`. Messages: `GetParNames` / `GetParTypes` / `GetParSizes` (registry discovery, as in rtc), `GetPar(indices[])`, `SetPar(index, value)`, `SetBlock(index, offset, values[])` (Fourier/filter/LUT arrays), `Commit(index)` (atomic swap), `SetStreamParams(indices[], decimation)`, `StartStream` / `StopStream`, `TriggerArb(mode)`, `Status`, `SetSampleRate(preset)`. Every message gets an ACK/NAK with the seq echoed. Parameters are addressed by registry index on the wire; names exist only at discovery time.
- **UDP :2351 — streaming.** A stream is a list of **registered parameter indices** plus a decimation factor — any parameter can be streamed (raw channels, volts, controller output, Fourier coefficients, diagnostics…), exactly as in rtc. Packet: `magic | stream_seq u32 | first_sample_index u64 | dropped u32 | n_records u16 | records[]`, records batched to fill ~1,400-byte packets, values snapshotted per tick so each record is sample-coherent. Worst case (8 kHz × 12 f32 values ≈ 400 kB/s) is well within W5500 throughput. Finite "capture N samples" workflows are done host-side on the continuous stream.

### Python package (`host/`)

`helic_daq` package: `Device` class that performs registry discovery at connect and exposes attribute-style access with tab completion (`dev.par.x_Kp = 0.5`, as rtc's Python interface did), typed get/set from the discovered type codes, coefficient/array upload with commit; `StreamReceiver` (UDP → numpy arrays, drop accounting, capture-N-samples helper, capture-to-file); and a CLI (`helic-daq list/get/set/stream/plot`) with basic live plotting via matplotlib. Framing constants mirrored from `helic-proto` with a round-trip test; everything else is discovered, not hard-coded.

## 7. Development setup recommendation

Buy a **Raspberry Pi Debug Probe** (~£12) and connect it to the Pico2's 3-pin SWD header. Toolchain: Rust stable + `thumbv8m.main-none-eabihf` target, **probe-rs** (`cargo run` flashes and streams logs), **defmt + defmt-rtt** for near-zero-overhead logging (safe even on core 1), `flip-link` for stack-overflow protection. Fallback: UF2 via BOOTSEL + `picotool`. A cheap logic analyzer (or scope) is strongly recommended for milestone 4's jitter verification and SPI bring-up.

## 8. Milestones

1. **Scaffolding** — workspace, Embassy skeleton booting both cores, defmt logging, blink + tick GPIO, CI (fmt, clippy, host tests, firmware build).
2. **`helic-core` DSP** — generators, PID, biquads, Fourier estimator, all with host unit tests. *(No hardware needed; can run in parallel with 3.)*
3. **Drivers** — AD7609 (PWM CONVST + BUSY IRQ + SPI read), AD5064, OptoNCDT parser; verified with scope/loopback.
4. **Real-time loop** — full tick pipeline on core 1, parameter mailbox, stream ring buffer; **measure jitter and CPU headroom** at 8 kHz via the debug pin; DAC-out → ADC-in loopback test.
5. **Ethernet + protocol v1** — embassy-net-wiznet, static IP, TCP command server with registry discovery, UDP streamer.
6. **Python host + end-to-end demo** — package + CLI; closed-loop PID demo against an RC-filter "plant" (DAC → RC → ADC); documented walkthrough.
7. **Extensions** — USB serial transport behind the same protocol layer, laser-in-the-loop control, flash-persisted config, 10 kHz preset if headroom allows, SSI encoder slot.

## 9. Open assumptions (to confirm)

1. **Laser preconfigured**: MVP assumes the OptoNCDT is already set (baud, 8 kHz output, ASCII/binary mode) via Micro-Epsilon's tool; firmware only parses. Firmware-driven sensor setup can be a milestone-7 item.
2. **ADC config pins wired to GPIO**: range and OS0–2 are MCU-controlled (per the pin map), not hard-strapped.
3. **Units**: samples converted to f32 volts on the MCU (cheap, one multiply) rather than streaming raw counts.
4. **Presets**: 1/2/4/8 kHz initially; 10 kHz deferred pending measured headroom.
5. **Static IP default** e.g. `192.168.1.235/24`, compile-time constant until flash-config lands.

## 10. Lessons from the previous implementation (dawbarton/rtc)

Reviewed 2026-07-09. **Carried over:**

- Name-based self-describing parameter registry with host-side discovery
  (§5a) and attribute-style Python access.
- Streams defined as lists of registered parameters + decimation, not fixed
  channels (§6).
- Runtime diagnostics (`clock_jitter`, `overrun`, loop last/max time) exposed
  as ordinary parameters (§1).
- On-write trigger semantics for parameters that need side effects (e.g.
  changing the input voltage range reconfigures the ADC and the scale factor).
- Practical hardware notes: AD5064 needs ~3 µs between sequential words
  (space out writes); oversampling ratio chosen from the sample rate as in
  `rtc_main.c`.

**Deliberately changed:**

- rtc's host access wrote through raw pointers into live RT variables with a
  spin-wait to dodge (not prevent) tearing → replaced by mailbox +
  double-buffer + sample-boundary application (§1, §5a).
- rtc accumulated phase as `time_mod_2pi += Δt·f` in f32 (rounding drift,
  per-tick sincos) → replaced by the integer phase accumulator + LUT (§5).
- rtc ran comms as a blocking polling loop on the same core, with interrupts
  disabled during the whole RT handler → replaced by the two-core split with
  lock-free queues (§1).
- rtc buffered finite captures in up to 8 MB of RAM and downloaded them
  afterwards over USB — impossible in 520 KB of SRAM and a comms bottleneck
  anyway → replaced by continuous UDP streaming with host-side capture (§6).
- NEON-vectorised sincos and alignment constraints don't apply to the M33;
  the LUT approach removes the need.

**Not adopted (for now):** per-period Fourier mean/variance and the filtered
random perturbation source from the duffing rig — accommodated by design
(§5, `period_start` hook) but not built in the first version; rig-style
per-experiment binaries — a single compile-time controller selection in
`config.rs` suffices initially.
