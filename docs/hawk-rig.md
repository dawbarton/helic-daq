# Hawk rig firmware plan

Hardware findings, architectural decisions, open questions, and the staged
implementation plan for the suspended hawk model used in the wind tunnel.

The intended hardware is one Raspberry Pi Pico 2W, two independently calibrated
elevator servos, one fuselage-mounted six-axis IMU, and one absolute encoder on
the pitch axis of the supporting gimbal. More encoders may be added later. The
Pico 2W will run the HELIC-DAQ control and acquisition loop and return discovered
parameters and sample streams over Wi-Fi.

This document is a plan, not hardware evidence. None of the hawk hardware paths
described below has yet been implemented or verified in HELIC-DAQ.

## Source material

The component material reviewed for this plan lives in the top-level `tmp/`
directory, which is gitignored and therefore exists only on the machine where
the material was gathered. The findings below summarise everything this plan
relies on from those files. The reviewed material is:

- servo specification: `../tmp/hawk-rig/servos/Specification - Servo.pdf`;
- Feetech HLS Arduino SDK: `../tmp/hawk-rig/servos/sdk/`;
- encoder specification: `../tmp/hawk-rig/encoder/Specification - Encoder.pdf`;
- AS5048A Arduino API: `../tmp/hawk-rig/encoder/AS5048A-master/`;
- IMU guide: `../tmp/hawk-rig/imu/lsm6dsox-and-ism330dhc-6-dof-imu.pdf`;
- Adafruit LSM6DS Arduino API: `../tmp/hawk-rig/imu/Adafruit_LSM6DS-master/`.

The repository architecture, wire protocol, supported host workflow, and current
hardware-verification boundary are defined in `developer_guide.md`,
`protocol.md`, `user_guide.md`, and `../notes.md` respectively.

## Component findings

### Elevator servos

The supplied specification identifies the servo as the Feetech HLS3606M-C001,
a metal-cased, steel-geared, continuous-rotation-capable serial servo with an
internal 12-bit magnetic position sensor.

Relevant electrical and mechanical properties are:

| Property | Value |
|---|---|
| Rated input range | 4.5–7.4 V |
| Typical test points | 4.8 V and 6 V |
| No-load speed at 6 V | 0.09 s/60 degrees, approximately 111 RPM |
| No-load current at 6 V | 150 mA |
| Rated current at 6 V | 350 mA |
| Stall current at 6 V | 1.3 A per servo |
| Stall torque at 6 V | 6 kg.cm |
| Idle current | 20 mA |
| Operating temperature | -20 to 60 degrees C |
| Internal position sensor | 12-bit magnetic encoder |
| Position resolution | 4096 counts/revolution, approximately 0.088 degrees/count |
| Neutral | count 2048, nominally 180 degrees |
| Gear backlash | at most 0.5 degrees |
| Waterproofing | none |

The two servos must not be powered from the Pico. A separate supply should be
rated for at least their combined 2.6 A stall current, with suitable margin,
local bulk decoupling, and a common logic ground. Servo-generated supply noise
must be kept away from the Pico, IMU, and encoder rails.

The control interface is half-duplex asynchronous TTL serial with 8 data bits,
one stop bit, and no parity. The configurable baud-rate range is 38,400 baud to
1 Mbaud, with 1 Mbaud as the stated factory default. IDs range from 0 to 253.
The signal input recognises a high level between 2 and 5 V and a low level
between 0 and 0.45 V. This does not establish that the servo's transmitted high
level is safe for the non-5-V-tolerant RP2350 input; the receive voltage must be
measured, and the interface must level-limit it before direct connection.

The maximum position-update rate is one update per millisecond. The supported
feedback comprises position, speed, load, input voltage, temperature, moving
state, and current. The servo supports position, constant-speed, and
constant-current modes, as well as multi-turn operation. Hawk should use normal
single-turn position mode unless a later mechanical requirement establishes
otherwise.

The HLS SDK defines the important registers as follows:

| Register | Address |
|---|---:|
| ID | 5 |
| Baud rate | 6 |
| Minimum angle limit | 9–10 |
| Maximum angle limit | 11–12 |
| Mode | 33 |
| Torque enable | 40 |
| Acceleration | 41 |
| Goal position | 42–43 |
| Goal torque | 44–45 |
| Goal speed | 46–47 |
| Torque limit | 48–49 |
| Present position | 56–57 |
| Present speed | 58–59 |
| Present load | 60–61 |
| Present voltage | 62 |
| Present temperature | 63 |
| Moving state | 66 |
| Present current | 69–70 |

Packets start with `FF FF`, followed by ID, length, instruction or status,
payload, and the one's-complement checksum of the intervening bytes. The SDK
provides a broadcast synchronised write and a synchronised read. For two
elevators, one synchronised write should update both goals together; two
independent sequential writes would introduce an avoidable skew.

Both servos may arrive as ID 1. Each must therefore be provisioned separately
before both are attached to the shared bus. One should remain ID 1 and the other
should become ID 2. Normal firmware must not rewrite EEPROM at boot.

The example SDK comments give the following command-unit conversions, but they
must be confirmed against the fitted devices before becoming user-facing
units:

- speed unit: approximately 0.732 RPM;
- acceleration unit: approximately 8.7 degrees/s2;
- torque/current limit unit: approximately 6.5 mA.

### Pitch encoder

The encoder is the ams OSRAM AS5048A, a 14-bit absolute magnetic rotary encoder
with an SPI interface. It measures a complete revolution without an index or
homing operation.

Relevant properties are:

| Property | Value |
|---|---|
| Resolution | 14 bits, 16,384 counts/revolution |
| Nominal angular resolution | 0.0219 degrees/count |
| Output sampling rate | nominally 11.25 kHz, 10.2–12.4 kHz range |
| System propagation delay | nominally 100 microseconds |
| Output noise | up to approximately 0.06 degrees RMS |
| Supply | 3.0–3.6 V directly, or 4.5–5.5 V using the internal LDO |
| Maximum supply current | 15 mA |
| SPI clock | at most 10 MHz |
| SPI word | 16 bits, MSB first |
| SPI mode | mode 1, matching the supplied Arduino API |

At 3.3 V, VDD3V and VDD5V are connected together. In 5 V operation, VDD3V
requires a 10 microfarad capacitor and must not be loaded externally. A 3.3 V
supply is preferable for direct Pico logic compatibility.

An SPI command contains an even-parity bit, a read/write bit, and a 14-bit
address. The returned word contains even parity, an error flag, and 14 bits of
data. Responses are pipelined: the result of one command is returned during the
next 16-bit transfer. A continuous acquisition loop can therefore prime the
pipeline once, then send `Read Angle` each tick while receiving the preceding
angle. Every received word must have its parity and error flag checked.

Important registers are:

| Register | Address |
|---|---:|
| Clear error flag | `0x0001` |
| Diagnostics and AGC | `0x3FFD` |
| CORDIC magnitude | `0x3FFE` |
| Angle | `0x3FFF` |

The diagnostic register reports:

- OCF: offset compensation is complete;
- COF: CORDIC overflow, making angle and magnitude invalid;
- COMP low: magnetic field is too strong;
- COMP high: magnetic field is too weak;
- AGC value: an additional indication of magnetic-field strength.

Recommended magnet guidance is a diametrically magnetised magnet approximately
6–8 mm in diameter and at least 2.5 mm high, producing 30–70 mT peak field at
the die. The magnet should normally be 0.5–2.5 mm above the package, and its
axis should be centred within approximately 0.25 mm. Actual magnitude and
diagnostic readings should be recorded during bring-up.

The zero should initially be a compile-time or host-configured software offset.
The AS5048A supports OTP zero programming, but burning it is irreversible and
is not needed for the first implementation.

The supplied Arduino API's degree conversion must not be ported verbatim. It
uses `8191` as a convenience scaling constant even though the raw result is a
14-bit value. HELIC-DAQ should convert using `count / 16384 * 2*pi`, then apply
the software zero and a well-tested signed wrapping operation. The gimbal is
expected to remain within a limited pitch range, so the first interface can
report the signed absolute angle without multi-turn unwrapping.

Future encoders should share the SPI clock and data pins and receive separate
chip selects. The device also supports daisy chaining, but separate chip
selects are easier to diagnose and isolate during bring-up. The driver should
therefore be allocation-free and usable with a fixed compile-time array of
devices rather than being hard-coded to exactly one encoder.

### Fuselage IMU

The supplied guide covers three related Adafruit breakouts: LSM6DSOX,
ISM330DHCX, and LSM6DSO32. The presence of the ISM330DHCX-specific API and
example suggests that it may be the intended device, but the fitted part is
not identified unambiguously by the local material. It must be confirmed
before finalising the driver configuration.

For the ISM330DHCX and LSM6DSOX:

- acceleration ranges are plus or minus 2, 4, 8, and 16 g;
- gyro ranges are plus or minus 125, 250, 500, 1000, and 2000 degrees/s;
- the ISM330DHCX additionally supports plus or minus 4000 degrees/s;
- supported output data rates run from 12.5 Hz to 6.66 kHz;
- WHO_AM_I is `0x6B` for ISM330DHCX and `0x6C` for LSM6DSOX;
- the breakout supports I2C or SPI, with two data-ready interrupt outputs;
- the breakout accepts 3–5 V power and logic through its regulator and level
  shifting, although powering VIN from 3.3 V keeps the system consistently at
  Pico logic levels.

SPI is preferable to I2C for the bounded core-1 path. The supplied Arduino API
uses SPI mode 0 at 1 MHz. That conservative rate should be used during initial
bring-up and raised only after signal-integrity and timing measurements.

The relevant register sequence demonstrated by the API is:

1. read WHO_AM_I and reject the wrong part;
2. issue software reset and wait for it to clear during cold-path setup;
3. enable block-data update so a low/high byte pair cannot tear;
4. configure acceleration and gyro output data rates and ranges;
5. perform a 14-byte burst from `OUT_TEMP_L` through all gyro and acceleration
   axes.

The Arduino library defaults to 104 Hz, plus or minus 4 g, and plus or minus
2000 degrees/s. Those are API defaults, not a suitable final choice for hawk.
The initial hawk configuration should use a 1.66 kHz output data rate so a
1 kHz master tick sees fresh data on every tick, subject to hardware
confirmation. Acceleration and gyro ranges should be compile-time choices
selected from measured tunnel loads; plus or minus 4 g and plus or minus
500 degrees/s are reasonable starting points, not acceptance limits.

The IMU burst should be converted to `f32` SI units in the SRAM-resident path:

- acceleration in m/s2;
- angular rate in rad/s;
- temperature in degrees C.

The IMU data-ready signal should be routed to a Pico input. The 1 kHz PWM
remains the experiment's master sample clock, while the data-ready state or
status register establishes whether each read was fresh. Missing-fresh-data
events must be counted and should eventually participate in the safety fault
policy.

## Recommended firmware architecture

`hawk-rig` should become a new production firmware crate under
`firmware/experiments/hawk-rig`. It should start from `pico2w-rig` because both
use the Pico 2W CYW43439 station-mode transport, DHCP, and the mandatory
synchronous core-1 loop.

The proposed core split is:

```text
core 1, synchronous SRAM loop             core 0, Embassy tasks
------------------------------------      -------------------------------
1 kHz PWM-wrap tick                       CYW43439 station-mode Wi-Fi
apply at most two commands                DHCP and discovery
read two servo positions                  TCP parameter/control service
read IMU burst                            UDP stream packetisation
read AS5048A pitch                        record-ring drain
controller and generators                status logging
clamp and safety gate                     TIMER0 alarm watchdog
sync-write both servo goals
enqueue one coherent record
```

All peripherals used per tick belong exclusively to core 1. Core 0 must never
touch their state. The hot path must not allocate, log, call Embassy, use
`embassy-time`, take a critical section, or execute from flash.

### Sample clock and expected budget

Use `SampleRate::Hz1000` and `PwmWrapSpinTick`. A hardware PWM wrap therefore
owns every sample instant, regardless of Wi-Fi or host activity. The one
millisecond period also matches the servo's stated maximum position-update
rate.

At 1 Mbaud, with ten serial bits per byte, an approximate worst-case bus budget
is:

- two-servo seven-byte SyncWrite: approximately 24 wire bytes, or 240
  microseconds;
- synchronised request and two position replies: approximately 260
  microseconds, depending on the exact requested block and turnaround time;
- 14-byte IMU burst at 1 MHz plus command byte: approximately 120
  microseconds;
- AS5048A command/response at 3 MHz plus mandated CS timing: approximately
  12 microseconds;
- controller, scaling, diagnostics, and record construction: still to be
  measured.

The estimate is roughly 650 microseconds before software overhead and is not
proof that the loop meets its deadline. The first hardware acceptance limit
should be set only after logic-analyser and GP14 measurements. A substantial
margin below 1000 microseconds is required.

If this budget proves too tight, reduce low-rate health-register traffic first.
Do not weaken the hardware clock, omit error checking, or move servo activity
to an unbounded core-0 task. A higher master rate is not initially appropriate:
complete servo command and feedback traffic cannot fit comfortably inside the
500 microseconds available at 2 kHz.

### Provisional Pico 2W resource map

The following map avoids the Pico 2W's fixed radio resources and gives the IMU
and encoder separate SPI peripherals, so their modes never need to be changed
on a shared bus:

| Function | Proposed peripheral and pins |
|---|---|
| CYW43439 | PIO1, DMA0, and fixed GP23/24/25/29 |
| Servo bus | UART1 TX GP4, UART1 RX GP5, transceiver direction/OE GP6 |
| IMU | SPI0: MISO GP16, CS GP17, SCK GP18, MOSI GP19; DRDY GP20 |
| AS5048A | SPI1: SCK GP10, MOSI GP11, MISO GP12, CS GP13 |
| Tick timing output | GP14 |
| Master sample clock | PWM slice 4, with no external PWM pin required |

This is a firmware proposal rather than a wiring instruction. `board.rs` must
reflect the final interface schematic, including the servo transceiver and any
additional encoder chip selects.

### Servo electrical interface

The half-duplex bus should use separate UART TX and RX pins on the Pico and an
external interface that joins them to the one-wire servo signal:

- the TX path must be tri-stated while a servo replies;
- the direction pin must change only after the UART has completely transmitted
  its stop bit;
- the RX path must limit any servo-driven high level to a safe RP2350 voltage;
- the direction turnaround and first response byte must be measured at 1
  Mbaud;
- the bus should fail to the receive/high-impedance state at reset;
- two servo power and ground returns must not send motor current through the
  Pico ground path.

A raw, ownership-preserving UART transport should perform bounded FIFO polling
from SRAM. Its construction should consume the typed UART and GPIO owners once,
beside the audited pin map, then expose only safe bound operations to `rig.rs`.

### SPI timing

The existing `firmware/common/src/analog_spi.rs` raw SPI mechanism is a useful
base for both sensors because it reconfigures and transfers entirely from SRAM.
It needs a timing-aware extension for the AS5048A. The encoder requires at
least 350 ns from CS falling to the first clock, at least 50 ns after the last
falling clock before CS rises, and at least 350 ns high between 16-bit frames.
Those requirements should be represented explicitly rather than relying on
incidental instruction overhead.

The IMU and encoder should use separate SPI peripherals in the initial board
map. Future AS5048A encoders can share SPI1 with separate chip selects.

## Control model

The existing HELIC-DAQ real-time pipeline has one scalar controller output. The
clean mapping is to define that scalar as collective elevator deflection in
degrees. The rig applies independent calibration to obtain each servo goal:

```text
collective command in degrees
        |
        +-- left:  zero + direction * linkage_scale * command -> clamp
        |
        +-- right: zero + direction * linkage_scale * command -> clamp
                           |
                    one SyncWrite packet
```

The two elevator mechanisms may therefore have different zero counts,
directions, linkage ratios, and hard count limits while representing the same
physical deflection. These calibration values and absolute safety limits
belong in `config.rs` initially and must be established with the linkage
disconnected or unloaded.

This mapping preserves the existing facilities:

- the mean target coefficient commands a constant elevator angle;
- the Fourier generator can command periodic elevator motion;
- a waveform table can replay a measured or designed deflection history;
- a controller can use pitch, IMU, or elevator feedback without runtime
  dispatch;
- `cmd_epoch` identifies the first record affected by a command.

The common generated sources currently use volts. Add an experiment-provided
output-unit hook to `Rig`, defaulting to `"V"`, so hawk can advertise `target`,
`forcing`, `table`, and `out` in degrees without changing the wire protocol or
existing experiments.

If future experiments require independently varying left and right elevator
commands at every sample, add an explicit multi-output controller and record
contract to the platform. Do not encode a second dynamic output in hard-coded
host indices, free-form atomics, or a core-0 side channel.

## Proposed discovered interface

The initial coherent stream inputs should be:

| Source | Unit | Meaning |
|---|---|---|
| `elev_l` | `deg` | Calibrated left elevator position reported by servo 1 |
| `elev_r` | `deg` | Calibrated right elevator position reported by servo 2 |
| `pitch` | `rad` | Signed AS5048A gimbal pitch around the configured zero |
| `acc_x` | `m/s2` | Fuselage acceleration X |
| `acc_y` | `m/s2` | Fuselage acceleration Y |
| `acc_z` | `m/s2` | Fuselage acceleration Z |
| `gyro_x` | `rad/s` | Fuselage angular rate X |
| `gyro_y` | `rad/s` | Fuselage angular rate Y |
| `gyro_z` | `rad/s` | Fuselage angular rate Z |
| `imu_temp` | `degC` | IMU die temperature |
| `servo_ok` | `bool` | Both position replies on this tick were valid |
| `imu_fresh` | `bool` | IMU data was fresh on this tick |
| `encoder_ok` | `bool` | Encoder parity, protocol, and field state were valid |

The common loop then appends `target`, `forcing`, `table`, `out`, and
`cmd_epoch`, producing 18 sources. This remains below the fixed limit of 24 and
leaves room for additional encoder channels or carefully selected servo
telemetry.

The streamed `out` is the applied collective deflection after the hard clamp
and safety gate. `elev_l` and `elev_r` are the positions measured before that
tick's new synchronised goal is sent, which is the normal one-tick command and
measurement ordering.

Latest-value experiment parameters and event counters should include, within
the fixed experiment-parameter capacity:

- left and right servo communication errors;
- left and right servo status faults;
- servo feedback-stale events;
- IMU communication and stale-data events;
- encoder parity/protocol errors;
- encoder field/CORDIC faults;
- optionally the most recently polled servo voltage, temperature, current,
  and load values.

Position must be read every tick. Slower health values should be read in a
fixed round-robin schedule, for example one value every 100 ticks, so their
cost is bounded and their worst-case tick is visible. Event counters should
use `ExtraParam::u32_event` when `diag_reset` should clear them.

No host package should hard-code the resulting indices. The existing protocol
v3 name discovery is sufficient, so hawk does not require a wire-protocol
revision or compatibility shim.

## Output safety

`HawkRig` drives physical actuators and must set `Rig::SAFETY_GATED = true`.
The system must start disarmed after every reset, and a dropped TCP connection
must disarm it.

The hard output clamp should enforce the intersection of the mechanically safe
left and right calibrated ranges. Each per-servo conversion must clamp again
to its absolute count window so that a calibration or arithmetic error cannot
command outside the fitted linkage range.

Candidate latching trip conditions are:

- either servo position reply becomes stale for a fixed number of ticks;
- repeated servo checksum, framing, ID, or timeout failures;
- a servo reports an overtemperature, voltage, overload, or other status
  fault;
- measured servo position diverges persistently from its goal beyond a safe
  following-error window;
- the AS5048A reports CORDIC overflow or persistent high/low field;
- encoder replies fail parity or retain the protocol error flag;
- IMU reads fail or fresh data remains absent for a fixed number of ticks;
- measured pitch leaves the mechanically safe gimbal window, if the encoder
  remains sufficiently trustworthy to enforce that condition.

Transient communication errors should remain observable, but the precise
number of consecutive failures needed to trip must be selected from hardware
evidence. The reusable `helic_core::safety::StaleCounter` pattern is suitable
for bounded consecutive-failure tracking.

The safe physical action is not yet settled. During initial bench bring-up,
disarm and trip should disable servo torque. In a running wind tunnel, a loose
elevator under aerodynamic load may be less safe than holding calibrated
neutral. The final policy must be agreed before tunnel operation and tested by
cutting control communications under representative load. It must not be
inferred solely from firmware convenience.

## Implementation plan

### 1. Add portable device logic to `helic-drivers`

Add three `no_std`, allocation-free modules:

1. `as5048a.rs`
   - construct commands with even parity;
   - validate response parity and error flag;
   - support primed/pipelined angle reads;
   - decode diagnostics, AGC, and magnitude;
   - scale to revolutions or radians with software zero and signed wrapping;
   - avoid irreversible OTP operations in the normal driver;
   - support fixed compile-time collections of encoders.
2. `ism330dhcx.rs`
   - cold-path WHO_AM_I, reset, BDU, automatic increment, range, and ODR setup;
   - one bounded raw burst decode for temperature, gyro, and acceleration;
   - explicit range-dependent `f32` SI scaling;
   - status/freshness decoding;
   - keep a small variant boundary if the fitted device is LSM6DSOX instead.
3. `feetech_hls.rs`
   - packet encode/decode and one's-complement checksum;
   - ping, torque enable/disable, and read primitives for setup;
   - two-servo SyncWrite position command;
   - two-servo SyncRead position response decoding;
   - status and signed-value decoding;
   - no `String`, heap allocation, or unbounded parser loops.

Host tests should cover known request and response bytes, wrong headers,
lengths, IDs, parity, checksums, truncated replies, signed fields, angle wraps,
range scaling, and all diagnostic bits. The Arduino code is behavioural and
packet-layout evidence, not Rust code to translate line for line.

### 2. Add RP2350 hot-path transports to `firmware/common`

Add an ownership-preserving raw half-duplex UART type. Embassy may perform
one-time pin and UART setup, after which the raw type owns the matching PAC
UART, TX, RX, and transceiver-enable mapping. It must provide:

- SRAM-resident bounded write and read operations;
- a TX-complete wait before releasing the bus;
- receive FIFO drain before a transaction;
- explicit microsecond timeout from raw TIMER0;
- deterministic receive-to-transmit turnaround;
- counters returned to the rig rather than logging from the hot path.

Extend the raw SPI support with explicit per-device CS setup, hold, and high
times, or add a dedicated AS5048A raw wrapper that enforces them. Bind each raw
device once beside the audited board ownership and expose only safe methods to
the tick.

Everything reachable per tick must be in `.data.ram_func`. Update the static
layout checker to require the hawk `run_hot_loop`, EABI memory helpers, UART
transfer, IMU burst, encoder transfer, and servo transfer symbols in SRAM.

### 3. Create `firmware/experiments/hawk-rig`

Use the predictable experiment structure:

- `board.rs`: only the final pin map and unassembled core-1, Wi-Fi, and core
  ownership bundles;
- `config.rs`: experiment name, 1 kHz sample rate, Wi-Fi configuration, servo
  IDs, bus rates, sensor ranges, encoder zero, elevator calibration, and hard
  safety limits;
- `telemetry.rs`: atomic latest values and bounded event counters;
- `rig.rs`: assemble all core-1 transports, initialise hardware before the
  clock begins, implement measurement, collective-to-dual-servo actuation,
  parameters, and safety hooks;
- `main.rs`: bind only the owned interrupts, move the complete hardware bundle
  to core 1, start the TIMER0 watchdog on core 0, and compose Wi-Fi, TCP, UDP,
  beacon, heartbeat, and status tasks;
- `Cargo.toml`, `build.rs`, `memory.x`, and `README.md`: follow `pico2w-rig`
  with a concise hawk-specific description.

`ActiveController` should initially be `PassThrough`. This exercises the whole
measurement, generator, clamp, and actuator path without introducing an
untested flight-dynamics controller. Any later controller belongs in
`helic-core` with a simulated host test.

### 4. Integrate hawk as a production experiment

Adding a genuine experiment changes the repository's declared production set.
Update all places that currently enumerate exactly CBC, whirl, and Pico 2W:

- the firmware workspace and CI expectations, where explicit;
- `firmware/tools/check_rt_layout.py`;
- `firmware/tools/rt_regression.py` with a `hawk` profile;
- `developer_guide.md` architecture, extension, build, and verification text;
- `user_guide.md` flashing, Wi-Fi, wiring, source, safety, and example workflow;
- `../notes.md`, marking every hawk path software-only until physical evidence
  is recorded.

Protocol v3, the broker, and the host-language codecs should not require a
hawk-specific change. Their discovery tests should nevertheless exercise a
non-voltage output unit and a larger source table where practical.

## Hardware bring-up and acceptance sequence

All DAQ and actuator interactions must be sequential. Do not run overlapping
clients or parallel hardware processes.

### Stage 1: power and servo provisioning

1. Verify the servo supply voltage, current limit, ground topology, and idle
   bus level without the Pico connected.
2. Measure the servo-driven TTL high level and validate the level-limiting and
   tri-state interface.
3. Connect one unloaded servo only, confirm ID 1 and 1 Mbaud, read all feedback,
   and test torque disable.
4. Provision the second servo as ID 2 while it is the only device attached.
5. Reconnect both servos, ping them individually, and verify that a SyncRead
   returns two correctly identified, checksum-valid position frames.
6. With linkages disconnected, exercise a very narrow command window and
   confirm that SyncWrite begins both motions together.

### Stage 2: encoder

1. Power the AS5048A at 3.3 V and verify SPI mode 1 at a conservative clock.
2. Confirm the pipelined response and even parity on a logic analyser.
3. Compare raw counts at known mechanical angles.
4. Record OCF, COF, COMP high/low, AGC, and magnitude across the intended pitch
   range.
5. Adjust magnet centring or spacing before accepting software compensation
   for a physical field fault.

### Stage 3: IMU

1. Read WHO_AM_I and record the exact fitted variant.
2. Verify the reset, BDU, selected ranges, selected ODR, and 14-byte burst.
3. Establish the fuselage coordinate convention and record the sensor-to-body
   axis transform.
4. Check approximately 1 g at rest in multiple orientations and known angular
   rotations around every axis.
5. Compare DRDY/status behaviour with the 1 kHz PWM tick and establish the
   expected freshness rate.

### Stage 4: integrated real-time loop

1. Run at 1 kHz with servos torque-disabled and no host capture.
2. Check identity, source discovery, parameter discovery, tick rate, phase
   spread, loop-time phases, timeouts, and all device counters.
3. Repeat under unthrottled TCP polling.
4. Enable conservative servo limits and run a low-amplitude, low-frequency
   collective command with the model unloaded.
5. Capture a small source set over Wi-Fi, then all sources, adding decimation
   if required by measured radio throughput.
6. Verify record indices, `cmd_epoch`, source-ring drops, UDP gaps, sensor
   freshness, and the relationship between applied command and reported
   elevator positions.
7. Measure GP14, UART direction turnaround, servo packet timing, both SPI
   transfers, and total tick duration on a scope or logic analyser.

Initial acceptance requires:

- approximately 1000 ticks/s in fixed-duration phases;
- zero overruns and tick timeouts;
- zero source-ring drops in the stated capture configuration;
- no unexplained command backlog growth;
- fixed or tightly bounded PWM wake phase;
- no servo checksum, ID, timeout, or status faults;
- no IMU communication failures or unexplained stale samples;
- no AS5048A parity, protocol, CORDIC, or magnetic-field faults;
- a measured tick maximum with deliberate margin below 1000 microseconds;
- deterministic disarm after reset and control disconnection.

### Stage 5: fault injection and tunnel qualification

Before tunnel use, test the response to:

- control TCP disconnection;
- explicit disarm;
- one disconnected servo;
- corrupted or absent servo replies;
- a blocked or following-error servo;
- servo undervoltage, overtemperature indication, or supply brownout;
- disconnected IMU or missing fresh data;
- disconnected encoder, invalid parity, and bad magnetic field;
- pitch beyond the configured safe window;
- Pico reset while the servo supply remains energised.

Only after those tests should the model be installed in the tunnel. Repeat the
low-amplitude sequence with airflow increased in controlled steps, while an
operator retains an independent emergency power-off. Record exact firmware
identity, wiring revision, calibration, safety policy, timing, stream results,
and every physical observation in `../notes.md`.

## Software regression requirements

Before calling the implementation complete, run the full repository checks
listed in `../AGENTS.md`, including root formatting, clippy, and tests; the
complete release firmware workspace; the real-time layout checker; the wired
variants; and all available Python, Julia, and MATLAB tests.

The hawk-specific release build must be built immediately before its static
layout check. Inspect the ELF for new compiler-generated calls reachable from
the tick; named-symbol checking is only a minimum guard. Run the hawk hardware
regression sequentially under idle, TCP-poll, and Wi-Fi capture load, then
disconnect for at least five minutes and confirm that the core-0 time watchdog
kept the record drain and timers healthy.

## Decisions required before implementation is final

The following facts cannot be inferred safely from the supplied files:

1. the exact fitted IMU variant and breakout revision;
2. the servo bus's measured transmit voltage and the final half-duplex interface
   schematic;
3. the left and right servo IDs after provisioning;
4. each elevator's zero count, direction, linkage ratio, and safe mechanical
   count window;
5. the gimbal encoder's mechanical zero, sign convention, magnet geometry, and
   safe pitch window;
6. the required sensor and control bandwidth beyond the proposed 1 kHz master
   rate;
7. whether a trip under tunnel load must torque-disable the elevators or hold
   calibrated neutral;
8. which servo status bits and consecutive-error thresholds constitute an
   immediate safety trip.

These are bring-up inputs and safety decisions, not details that firmware
should guess.
