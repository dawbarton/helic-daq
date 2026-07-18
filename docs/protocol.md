# HELIC-DAQ wire protocol v3

Authoritative specification. `helic-proto` implements the Rust codec used by
the firmware; `host-python/helic_daq/protocol.py`,
`host-julia/src/protocol.jl`, and `host-matlab/+helicdaq/Protocol.m` implement
the host codecs. All four are tested against the vectors below.

All multi-byte fields are little-endian.

| Port | Transport | Purpose |
|---|---|---|
| 2350 | TCP | Parameter access, stream control and status |
| 2351 | UDP | Device-to-host sample streaming |
| 2352 | UDP | Device discovery beacon |

## Control channel (TCP :2350)

The direct MCU endpoint serves one client at a time. The host sends one frame
and receives exactly one response. Closing that direct connection stops any
active stream. The optional broker retains this request/response framing but
accepts multiple downstream clients and has the shared-state rules described
below.

### Frame layout

| offset | size | field |
|---|---|---|
| 0 | 2 | magic = `0x4C48`, little-endian ASCII `HL` |
| 2 | 1 | message type |
| 3 | 1 | sequence number, chosen by the host and echoed |
| 4 | 2 | payload length, at most 1024 bytes |
| 6 | len | payload |
| 6+len | 2 | CRC-16/CCITT-FALSE over message type through payload |

The CRC uses polynomial `0x1021`, initial value `0xFFFF`, no reflection and
no final XOR. A response has the request type, or type 255 with payload
`error_code u8, request_type u8`. Bad framing drops the TCP connection.

### Message types

| type | name | request payload | response payload |
|---|---|---|---|
| 1 | GetParams | `start u16` | `start u16, next u16`, then repeated definitions |
| 2 | GetSources | empty | repeated `name NUL, unit NUL` |
| 3 | GetPar | repeated `index u16` | raw values concatenated in request order |
| 4 | SetPar | `index u16, raw value` | empty |
| 5 | SetBlock | `index u16, offset u32, data...` | empty |
| 6 | Commit | `index u16, len u32` | empty |
| 7 | StreamSetup | `decimation u16, count u32, n u8, source u8 × n` | empty |
| 8 | StreamStart | host UDP `port u16` | empty |
| 9 | StreamStop | empty | empty |
| 10 | Status | empty | `version u8, n_params u16, n_sources u8, sample_rate f32, uptime_ms u32` |
| 255 | Error | not a request | `error_code u8, request_type u8` |

Parameter type codes are Python `struct` characters: `B b H h I i f c` for
u8, i8, u16, i16, u32, i32, f32 and char. A value occupies
`count × sizeof(type)` bytes; char arrays are NUL-padded strings. Parameter
names are ASCII and at most 23 bytes.

`SetPar` rejects non-finite f32 values. `SetBlock` stages a slice of a long
array starting at an element offset; `Commit` atomically activates the first
`len` staged elements at an RT sample boundary. For `table`, offsets count
f32 elements, block data must be a multiple of four bytes, and commits accept
2–4096 finite values. A pending swap returns busy so core 0 never writes a
buffer still visible to core 1.

`StreamSetup` requires `decimation ≥ 1`, at least one source, and every source
id to be less than `n_sources`. `count = 0` streams continuously. Reconfiguring
an active stream returns busy, preventing a packet-layout change mid-stream.

`uptime_ms` wraps after approximately 49.7 days.

Error codes are: 1 bad frame, 2 unknown type, 3 bad index, 4 bad length,
5 read-only, 6 bad value and 7 busy.

### Broker extension

`helic-broker` presents the same protocol-v3 control and stream services on
the loopback interface and forwards the message types above to one MCU. It
accepts multiple TCP clients. Stream configuration and running state are
global: any client can configure, start, attach to, or stop the stream. Each
client's UDP endpoint and quiet flag are connection-local. `StreamStart` on an
active stream attaches that client without restarting the MCU stream.

The broker consumes these additional request types instead of forwarding
them:

| type | name | request payload | response payload |
|---|---|---|---|
| 128 | BrokerInfo | empty | broker information below |
| 129 | QuietStreamStart | client UDP `port u16` | empty |
| 130 | GetRecent | `records u32` | echoed `records u32`, followed by UDP replay |
| 131 | SetClientQuiet | `quiet u8` (0 or 1) | empty |

`QuietStreamStart` has the same global start/attach behaviour as
`StreamStart`, but sets the requesting client quiet. `SetClientQuiet` changes
only the requesting client. A quiet client receives no live datagrams.
`GetRecent` is valid only for an attached, quiet client and sends exactly the
requested number of records, using the active stream's ordinary packet
format. The client remains quiet. History is shared, may predate the client's
connection, and is discarded when the stream stops or completes.

BrokerInfo extension version 1 is:

| offset | size | field |
|---|---|---|
| 0 | 1 | extension version = 1 |
| 1 | 1 | state flags |
| 2 | 2 | capability flags |
| 4 | 4 | configured history capacity, ms |
| 8 | 4 | currently available history records |
| 12 | 2 | active/configured decimation (1 if unconfigured) |
| 14 | 4 | active/configured count |
| 18 | 2 | connected TCP clients |
| 20 | 1 | selected source count `n` |
| 21 | n | selected source ids |

State bits are: bit 0 upstream connected, bit 1 configured, bit 2 running,
bit 3 this client attached, bit 4 this client quiet, and bit 5 a staged table
transaction is owned. Capability bits 0–3 respectively advertise quiet
start, recent replay, quietness changes, and shared configuration.

Broker-only error codes are: 8 client not attached, 9 client not quiet,
10 no active stream, and 11 insufficient history. A direct MCU does not
implement types 128–131 and returns the ordinary unknown-type error.

### Parameter discovery

Hosts read `n_params` from `Status`, then request `GetParams` starting at index
zero. Each response echoes the requested `start`, supplies the exclusive
`next` index and appends the complete definitions in `[start,next)`. The host
continues from `next` until it equals `n_params`. Firmware fills each page with
as many complete definitions as fit; a definition is never split across
pages. When `start < n_params`, `next` is strictly greater than `start`.
`start == n_params` is a valid empty terminal page, while a larger start is a
bad index. Any request not exactly two bytes long is a bad length.

Hosts must verify the echoed start, forward progress, the definition count
against `next - start`, and the final count against `Status`. Definitions stay
in registry order, so names and metadata cannot become misaligned. Hosts must
discover by name and must not cache indices across connections. Parameter
names are ASCII and at most 23 bytes. Source names are ASCII and at most
15 bytes; source units are ASCII and at most 7 bytes.

The v3 base registry is:

| name | type | access | meaning |
|---|---|---|---|
| firmware | c×16 | ro | `<version> <7-char git hash>` build identity |
| experiment | c×16 | ro | compiled experiment name |
| sample_freq | f | ro | sample rate, Hz |
| ticks | I | ro | RT-loop tick count |
| loop_time_last | I | ro | latest tick execution time, µs |
| loop_time_max | I | ro | maximum tick execution time, µs |
| clock_jitter | I | ro | worst excess tick spacing, µs |
| overruns | I | ro | ticks exceeding the sample period |
| tick_timeouts | I | ro | timed-out sample-clock waits |
| records_dropped | I | ro | source-ring overflow count |
| freq | f | rw | master Fourier frequency, Hz |
| target_coeffs | f×33 | rw | target Fourier coefficients |
| forcing_coeffs | f×33 | rw | forcing Fourier coefficients |
| ctrl_reset | I | rw | write non-zero to reset the controller |
| table | f×4096 | block | staged arbitrary-waveform storage; GetPar is too large |
| table_len | H | ro | active table length |
| table_freq | f | rw | free-running table playback frequency, Hz |
| table_gain | f | rw | table contribution gain |
| table_interp | I | rw | interpolation order: 0 zero-order hold, 1 linear |
| table_mode | I | rw | 0 off, 1 loop, 2 one-shot, 3 locked loop, 4 locked one-shot |
| table_mult | I | rw | locked integer frequency multiplier, at least 1 |
| table_phase | f | rw | locked phase offset in turns, in [0,1) |
| table_trigger | I | rw | write non-zero to arm/start a one-shot |
| wake_phase_min | I | ro | min µs from conversion trigger to tick body start |
| wake_phase_max | I | ro | max µs from conversion trigger to tick body start |
| t_measure_max | I | ro | maximum measure (ADC read) phase time, µs |
| t_actuate_max | I | ro | maximum actuate (DAC write) phase time, µs |
| t_rest_max | I | ro | maximum remaining tick body time, µs |
| diag_reset | I | rw | write non-zero to reset timing diagnostics and event counters |
| cmd_backlog_max | I | ro | maximum queued host commands observed at a tick boundary |
| arm | I | rw | output safety arm: write 1 to arm (clears a stale trip), 0 to disarm |
| safety | I | ro | safety-gate bitfield: bit0 armed, bit1 latched trip, bit2 clamped, bit3 quieted |

`wake_phase_*` read 4294967295/0 until the rig reports a sample-clock
phase. `diag_reset` clears the `*_max`/`*_min` diagnostics along with
`loop_time_max`, `clock_jitter`, `overruns`, `tick_timeouts`,
`records_dropped`, `cmd_backlog_max`, the safety clamp/quiet tick counts, and
experiment event counters such as the laser error diagnostics; total counters
such as `ticks` and `laser_frames_received` keep running. `arm`/`safety` act
only on an experiment whose rig opts into the safety gate (`cbc-rig`);
elsewhere `arm` is inert and `safety` reads 0. The output is disarmed after
every reset and on control-connection loss.

Experiment read-only values, rig parameters and controller parameters follow
the base registry. For `cbc-rig`, these include `laser`,
`laser_frames_received`, `laser_uart_errors`, `laser_parse_errors`,
`laser_invalid_frames`, `laser_unexpected_values`, `laser_sync_errors`,
`rig_laser_range`, and `rig_out_channel`. `laser_frames_received` is a
monotonic total since binary-stream synchronisation. The UART, parser,
invalid-frame, unexpected-value, and synchronisation error counters are totals
since boot or the last `diag_reset`. `laser_sync_errors` counts UART and
parser faults while acquiring eight consecutive well-formed distance frames
after `OUTPUT RS422`. Controller names depend on the compile-time selected
controller.

Fourier coefficients use `[mean, a_1..a_K, b_1..b_K]`, representing
`mean + Σ_k a_k cos(kθ) + b_k sin(kθ)`. The default build uses K = 16.

### Source discovery

GetSources returns the source table. A source id is its zero-based position
in this table. The table is assembled as:

1. experiment inputs;
2. controller telemetry;
3. `target`, `forcing`, `table`, and `out`, all in volts;
4. `cmd_epoch`, in counts.

Names are unique ASCII strings of at most 15 bytes; units are at most 7
bytes. `cbc-rig` currently begins with `adc0` through `adc7` in volts and
`laser` in millimetres. Hosts resolve requested source names from this table;
there are no protocol-wide fixed source ids.

`cmd_epoch` starts at zero and advances once for every `RtCommand` that core 1
applies at a sample boundary. It wraps modulo 2²⁴, so every emitted value is
an exactly representable `f32`; hosts calculate changes modulo 2²⁴. The record
containing the advanced value is the first record affected by those commands.
A jump of two means that two commands were applied at the same boundary.
Operations that do not enter the command queue, including `SetBlock` and
`diag_reset`, do not advance it; a table `Commit` does. Decimation or record
loss can bound an update between observed records but cannot recover an
omitted effective sample index.

## Stream channel (UDP :2351)

The device sends packets to the TCP peer address and the port supplied in
StreamStart. Selected per-tick values are batched into datagrams no larger
than 1472 bytes and flushed at least every 5 ms.

Before sending StreamStart, hosts should bind their UDP receive socket and
send a small UDP datagram from that socket to the device stream port. The
datagram payload is ignored and has no wire-level meaning; it exists to let
stateful host firewalls and NATs classify the subsequent device-to-host
stream packets as return traffic. The UDP primer must use the same local port
that will be supplied in StreamStart.

| offset | size | field |
|---|---|---|
| 0 | 2 | magic = `0x4C48` |
| 2 | 1 | version = 3 |
| 3 | 1 | values per record |
| 4 | 4 | packet sequence number |
| 8 | 4 | first sample index |
| 12 | 4 | cumulative source-ring drops |
| 16 | 2 | decimation |
| 18 | 2 | record count |
| 20 | ... | record-major f32 values |

Within a packet, record i has sample index
`first_index + i × decimation`. Packet sequence gaps indicate UDP loss;
`dropped` reports loss before packetisation.

## Discovery beacon (UDP :2352)

The request is `magic u16, 0x01`.
The response is `magic u16, 0x02, version u8, control_port u16, mac[6],
experiment c×16, firmware c×16`. Both hardware firmware and the host simulator
respond. Requests and responses are fixed-size and carry no control-frame CRC.
Hardware uses the same compact firmware identity as the parameter registry;
the defmt boot banner retains the full `helic-daq <version> <git describe>`.

## Known-answer vectors

- `crc16("123456789") = 0x29B1`, `crc16("") = 0xFFFF`,
  `crc16([00]) = 0xE1F0`.
- GetParams request for start index zero, sequence 1:
  `48 4C 01 01 02 00 00 00 89 0C`.
- GetSources request, sequence 1:
  `48 4C 02 01 00 00 98 5E`.
- A GetParams entry for writable scalar f32 `freq`:
  `66 72 65 71 00 66 01 00 01`.
- GetSources entries `adc0 [V]`, `laser [mm]`:
  `61 64 63 30 00 56 00 6C 61 73 65 72 00 6D 6D 00`.
- SetBlock request, sequence 2, index 12, offset `0x01020304`, data `AA BB`:
  `48 4C 05 02 08 00 0C 00 04 03 02 01 AA BB 39 A7`.
- Commit request, sequence 3, index 12, length `0x01020304`:
  `48 4C 06 03 06 00 0C 00 04 03 02 01 08 D1`.
- Status response, sequence 1, version 3, 17 params, 13 sources, 8000 Hz,
  uptime 42000 ms:
  `48 4C 0A 01 0C 00 03 11 00 0D 00 00 FA 45 10 A4 00 00 76 0A`.
- BrokerInfo payload, extension version 1, state `0x1F`, capabilities
  `0x000F`, 10 s capacity, 42 available records, decimation 4, continuous
  count, two clients, and sources 0, 8, and 12:
  `01 1F 0F 00 10 27 00 00 2A 00 00 00 04 00 00 00 00 00 02 00 03 00 08 0C`.
- Beacon request: `48 4C 01`.
- Beacon response for protocol 3, port 2350, MAC `02:48:4c:00:00:01`,
  experiment `cbc-rig`, firmware `helic-daq sim`:
  `48 4C 02 03 2E 09 02 48 4C 00 00 01 63 62 63 2D 72 69 67 00 00 00 00 00 00 00 00 00 68 65 6C 69 63 2D 64 61 71 20 73 69 6D 00 00 00`.
