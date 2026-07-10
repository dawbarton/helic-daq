# Handoff notes — hardware bring-up session (2026-07-10)

Context for picking up where this session left off: implementation status,
what was tried on real hardware, what was confirmed working, and one
open/unresolved bug. Read this before re-deriving any of it from scratch.

## 1. Implementation status (software, all committed on `main`)

Six commits on `main`, one per milestone, all with passing tests
(`8756433`, `3697b39`, `91ef234`, `955214b`, `deaaba6`, `151c3d3`):

1. **Scaffolding** — Cargo workspace (`cbc-core`, `cbc-proto` build/test on
   host; `firmware/` is its own workspace for `thumbv8m.main-none-eabihf`).
   Dual-core Embassy skeleton.
2. **DSP library** (`cbc-core`) — phase accumulator, sine LUT, periodic
   Fourier generator, arbitrary LUT generator, biquad filters, PID,
   `Controller` trait, Fourier estimator. 33 host tests.
3. **Drivers** (`cbc-drivers`) — AD7608, AD5064, optoNCDT parser, generic
   over `embedded-hal` 1.0. 17 host tests.
4. **Real-time loop** (`firmware/src/rt_loop.rs`) — PWM-timed CONVST
   (hardware-clocked sampling), BUSY-edge pipeline, generators + controller
   + DAC write, lock-free cross-core queues (`heapless::spsc`), diagnostics
   as atomics.
5. **Ethernet + protocol** (`cbc-proto`, `firmware/src/comms/`,
   `firmware/src/params.rs`) — framing/CRC/stream packet protocol
   (`docs/protocol.md`), name-based parameter registry, W5500 bring-up via
   `embassy-net-wiznet`, TCP control server, UDP streamer.
6. **Python host package** (`host/`) — `cbc_daq` package + `cbc-daq` CLI,
   24 tests including an in-process protocol emulator.

Plus `docs/user_guide.md`, `docs/developer_guide.md`, `docs/protocol.md`.

Everything above was written and unit-tested but **never run on real
hardware** before this session — this session was the first hardware
bring-up.

## 2. Uncommitted changes right now

`git status` shows two modified files, **not yet committed** — this
session's bring-up diagnostics and one real bug fix. Do not lose these:

- `firmware/src/main.rs`:
  - `laser_task` is **disabled** (not spawned) — see §4.1, this is a real
    unresolved robustness bug, not just a diagnostic toggle.
  - `laser_task`'s error-retry loop gained a 10 ms backoff
    (`Timer::after_millis(10).await` before retrying `rx.read()`) — a
    correctness fix, keep this regardless of what happens with §4.1.
  - `core0_main` and `comms::init` gained one-shot `info!` checkpoint logs
    (harmless, useful, keep them).
  - A new `net_beacon_task` was added: broadcasts a 4-byte UDP packet to
    `255.255.255.255:9999` once/second. This was a diagnostic for §4.2 and
    should be removed once that's resolved (or kept behind a feature flag
    if it's judged generally useful for future bring-up).
- `firmware/src/comms/mod.rs`: the two checkpoint logs referenced above.

Run `git diff` to see the exact patch before deciding what to keep/commit/
revert.

## 3. Hardware bring-up environment

- **Debug probe**: a second Raspberry Pi Pico running Raspberry Pi's
  official `debugprobe` firmware, wired via SWD (SWCLK/GND/SWDIO) to the
  target's 3-pin debug header. Enumerates as `Debugprobe on Pico (CMSIS-DAP)`.
- **`probe-rs` 0.31.0** installed via `brew install probe-rs-tools` (was not
  present at session start).
- Flash/run: `cd firmware && cargo run --release` (uses `probe-rs run
  --chip RP235x` per `firmware/.cargo/config.toml`).
- **Target board**: W5500-EVB-Pico2. Confirmed genuinely alive via a
  BOOTSEL-mode mass-storage-device test (independent of SWD) early in the
  session, after an initial SWD "target did not respond" failure that
  turned out to be a wiring/connection issue on the SWD link — reseating
  fixed it. **If SWD ever stops responding again, suspect the physical SWD
  connection first** (it was disturbed at least once by handling the board
  while attaching other cables); a BOOTSEL-mode mass-storage check is a
  good way to confirm the board itself is alive independent of SWD.
- **Network test rig**: a USB-Ethernet adapter on the Mac (`en7`, currently
  has a pre-existing static `192.168.178.20/16`, which happens to cover
  `192.168.1.0/24` too) connected via a **direct cable, no switch/hub** to
  the target board's on-board W5500 RJ45 jack. Device's static IP is
  `192.168.1.235/24` (`firmware/src/config.rs::IP_ADDR`).
- No ADC, DAC, or laser sensor physically connected to the target during
  this entire session — all hardware bring-up so far is MCU + on-board
  W5500 only.

## 4. What was found, in order

### 4.1 `laser_task` livelocks core 0 when the sensor isn't connected — FIXED (partially)

**Symptom**: with the laser UART, network, and status tasks all spawned,
core 0 appeared completely wedged — no LED blink, no log output at all
past the very first boot line, indefinitely.

**Diagnosis method**: used `probe-rs gdb` (starts a GDB server) + `lldb
--batch -o "gdb-remote 127.0.0.1:1337" -o "thread list" -o "bt all"` to
halt and inspect both cores' program counters without a full interactive
debugger (no `gdb`/`arm-none-eabi-gdb` available on this Mac, only `lldb`).
Resolved the halted PC to a symbol via `nm -n` (no `addr2line` available)
and manual nearest-symbol-below lookup. First capture: core 0's PC was
inside `UART0_IRQ`.

**Isolation**: bisected by commenting out spawned tasks one at a time
(`blink` alone → works; `+laser_task+status_task` → hangs again;
`blink+status_task` without `laser_task` → works, confirmed via
`status_task`'s 1 Hz log line actually appearing). This conclusively
isolated `laser_task`.

**Root cause**: nothing is connected to the optoNCDT UART RX pin (GP1), so
it's floating. This generates continuous UART framing/break errors. The
original code (`if rx.read(&mut buf).await.is_err() { continue; }`) retried
immediately with no backoff. Added a 10 ms delay before retry — this did
**not** fully fix it (still hung in a later test with the same three tasks).
Root-caused further by reading `embassy-rp`'s UART driver source
(`~/.cargo/registry/.../embassy-rp-0.10.0/src/uart/mod.rs`): the async
`read()` implementation is properly non-blocking (bounded FIFO drain,
`select`-based await), so this isn't a software busy-loop — it's a genuine
**hardware interrupt storm**: enabling the UART's error interrupts (which
`read()` does on every call) against a continuously-noisy floating line can
retrigger the interrupt fast enough, *within* that enabled window, to
dominate the CPU regardless of how long we wait *between* `read()` calls.

**Current state**: `laser_task` is **not spawned** (see §2). The 10 ms
backoff fix is real and should stay, but is not sufficient by itself.

**Not yet done — proper fix options, pick one:**
- Add a hardware pull-up resistor on the physical RX line (simplest, most
  robust — UART idle/mark state is HIGH, a pull-up prevents the floating
  condition entirely).
- A firmware-level pull-up was attempted and abandoned: `embassy-rp`'s pad
  pull-config is only reachable through `gpio::Flex`, whose `Drop` impl
  resets `pad_ctrl` (wiping the pull) before the pin can be handed to
  `UartRx::new`, and `Peri::pad_ctrl()` itself is a private/sealed method
  not reachable from outside the `embassy_rp` crate. A `core::mem::forget`
  trick to skip `Flex`'s `Drop` was considered but not implemented/tested —
  worth trying if a hardware pull-up isn't an option. See
  `~/.cargo/registry/src/index.crates.io-*/embassy-rp-0.10.0/src/gpio.rs`
  around `Flex::new`/`Flex::set_pull`/`impl Drop for Flex`.
- Alternative: restructure `laser_task` to detect a sustained error
  condition (e.g. N consecutive errors) and back off much more
  aggressively (seconds, not milliseconds), or stop retrying until
  something (a parameter write) explicitly re-enables it.

### 4.2 W5500 Ethernet: TX and link work, RX does not — UNRESOLVED

**What works, confirmed with hard evidence:**
- W5500 SPI0 register access works (accurate, correct periodic PHY link
  status polling: firmware logs `link_up = true` at the right time, and
  independently `ifconfig en7` on the Mac shows `status: active,
  100baseTX <full-duplex>` — both ends agree).
- Static IP configuration works (`network up: 192.168.1.235/24` logged).
- **Device→host transmit works**: added a temporary `net_beacon_task`
  (`firmware/src/main.rs`, uncommitted, see §2) that broadcasts a 4-byte
  UDP packet to `255.255.255.255:9999` once/second. Confirmed via `sudo
  tcpdump -i en7 -n` on the Mac: packets arrive exactly on schedule,
  correctly formatted (`IP 192.168.1.235.9999 > 255.255.255.255.9999: UDP,
  length 4`).

**What doesn't work:**
- **Host→device receive appears completely broken.** The Mac sends real
  ARP requests (confirmed via `tcpdump`: `ARP, Request who-has
  192.168.1.235 tell 192.168.178.20`, once per ping, real frames on the
  wire) but the device **never replies** — `arp -a` on the Mac shows
  `(incomplete)` indefinitely, `ping` fails with `Host is down` /
  `No route to host` / plain packet loss depending on ARP cache state.
  `netstat -I en7 -b` showed `Ipkts` (inbound packet count) stuck at
  exactly 0 throughout, even while `Opkts` incremented from ARP requests
  going out — i.e. the Mac's interface has received **zero frames total**
  from the device this entire session, despite the device's own broadcasts
  (§ above) proving it can transmit fine and the link is physically good in
  both directions (100BASE-TX autonegotiation requires bidirectional
  signal, so basic electrical continuity both ways is not in serious doubt).

**Ruled out during this session (do not re-investigate these):**
- MAC filter (`MFEN` bit, enabled by `embassy-net-wiznet`'s W5500 MACRAW
  setup) blocking ARP: confirmed via datasheet search that `MFEN=1` still
  passes broadcast packets (ARP requests are broadcast). Not the cause.
- Wrong GPIO pin mapping for W5500 SPI/RST/INT: confirmed via web search
  against WIZnet's official W5500-EVB-Pico2 docs that GP16/17/18/19/20/21 =
  MISO/CSn/SCK/MOSI/RSTn/INTn exactly matches `firmware/src/board.rs`'s
  `EthParts`. Not the cause.
- Missing `bind_interrupts!` for GPIO edge interrupts: confirmed by reading
  `embassy-rp`'s `gpio.rs` that `IO_IRQ_BANK0` is registered automatically
  inside `embassy_rp::init()` (a real `#[interrupt] fn IO_IRQ_BANK0()`
  hardwired in the crate), unlike UART/DMA/SPI which need explicit
  `bind_interrupts!` entries. GPIO async waits should work out of the box.
  Not (yet) confirmed to be involved.

**Leading remaining hypothesis, not yet tested:** the RX path in
`embassy-net-wiznet`'s `Runner::run()`
(`~/.cargo/registry/src/index.crates.io-*/embassy-net-wiznet-0.3.0/src/lib.rs`,
around line 63) is driven by `self.int.wait_for_low().await` on the W5500's
INT pin (GP21, `Pull::Up` in `board.rs`) — this is the *only* code path
that depends on that pin; TX and the 500 ms link-status poll timer do not.
Everything upstream of that wait (SPI reads/writes, PHY status, TX) is
proven working, which narrows the fault to either: the chip genuinely never
asserting its INT line on frame receipt (a chip-side register config issue
— re-check `SOCKET_INTR_MASK`/`COMMON_SOCKET_INTR` writes in
`embassy-net-wiznet`'s `device.rs`, or try polling the raw RX buffer state
via direct register reads instead of trusting the INT-driven path), or a
genuine asymmetric hardware fault (RX-specific fault in the RJ45
magnetics/W5500/cable — TX-only faults are a real, if less common, failure
mode; a bad connection can pass enough energy for 100BASE-TX link-pulse
autonegotiation while still failing full data-rate RX specifically).

**Suggested next steps, roughly in order of effort:**
1. Cheap: swap the Ethernet cable and/or the USB-Ethernet adapter to rule
   out a hardware fault empirically. Not yet tried.
2. Add register-level diagnostics: read the W5500's `Sn_IR`/`Sn_RSR`
   (socket interrupt / RX received size) registers directly and log them
   periodically, to see whether the chip's own RX buffer ever shows
   nonzero received bytes even if the INT pin/software path never notices.
   This would definitively separate "chip never received anything" from
   "chip received it but our INT-driven read path missed it."
3. Try replacing the INT-driven wait with a polled fallback (e.g. a timer
   that periodically checks RX buffer state directly via SPI, bypassing
   `wait_for_low()` entirely) as a workaround/diagnostic, since
   `embassy-net-wiznet`'s `Runner::run()` would need to be forked/patched
   locally to test this (it's not configurable from outside the crate).
4. If a cable/adapter swap doesn't fix it and register-level RX-size checks
   confirm the chip really isn't receiving anything, consider it a
   candidate hardware fault on this specific board and try a different
   W5500-EVB-Pico2 if one is available.

## 5. Suggested immediate next actions for whoever picks this up

1. Decide what to do with the uncommitted diagnostic changes (§2): at
   minimum keep the laser backoff fix and the checkpoint logs; decide
   whether to commit `laser_task` disabled-with-comment as an honest
   interim state, or hold the commit until a real fix lands; probably
   remove or gate `net_beacon_task` once §4.2 is resolved.
2. Continue §4.2 per the suggested next steps above — the cable swap (#1)
   is the fastest signal.
3. Independently of networking, ADC/DAC/RT-loop hardware bring-up has not
   been attempted at all yet (no ADC/DAC physically wired this session) —
   that's a fully separate, unblocked next milestone once you're ready,
   and doesn't depend on resolving the networking issue.
