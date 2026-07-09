# CBC-DAQ

A real-time control and data acquisition platform for control-based
continuation, built on the RP2350 (W5500-EVB-Pico2) and the Rust Embassy
framework. Successor to the BeagleBone Black-based
[rtc](https://github.com/dawbarton/rtc).

- **User guide** (flashing, connecting, CLI/Python usage): [docs/user_guide.md](docs/user_guide.md)
- **Developer guide** (architecture, extending, testing): [docs/developer_guide.md](docs/developer_guide.md)
- Wire protocol: [docs/protocol.md](docs/protocol.md)
- Requirements: [AGENTS.md](AGENTS.md)
- Design & roadmap: [docs/implementation_plan.md](docs/implementation_plan.md)
- Periodic signal generator design: [docs/periodic_signal_generator.md](docs/periodic_signal_generator.md)

## Layout

| Directory | Contents |
|---|---|
| `cbc-core/` | Hardware-independent DSP (generators, controllers, filters, Fourier estimation) — `no_std`, host-testable |
| `cbc-proto/` | Wire protocol shared between firmware and host |
| `firmware/` | RP2350 firmware (own Cargo workspace, always builds for `thumbv8m.main-none-eabihf`) |
| `host/` | Python host package + CLI (from milestone 6) |

## Building

Host crates (with tests):

```sh
cargo test
```

Firmware (from `firmware/`; the target is configured automatically):

```sh
cd firmware
cargo build --release
```

## Flashing & logs

With a debug probe (recommended — Raspberry Pi Debug Probe on the SWD header)
and [probe-rs](https://probe.rs) (`cargo install probe-rs-tools`):

```sh
cd firmware
cargo run --release   # flashes and streams defmt logs
```

Without a probe, via the BOOTSEL USB bootloader:

```sh
cd firmware
cargo build --release
picotool uf2 convert target/thumbv8m.main-none-eabihf/release/cbc-daq-firmware -t elf cbc-daq.uf2
picotool load cbc-daq.uf2  # board in BOOTSEL mode
```
