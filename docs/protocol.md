# HELIC-DAQ wire protocol v2

Authoritative specification. `helic-proto` implements the Rust codec used by
the firmware; `host/helic_daq/protocol.py` implements the Python codec. Both
are tested against the vectors below.

All multi-byte fields are little-endian.

| Port | Transport | Purpose |
|---|---|---|
| 2350 | TCP | Parameter access, stream control and status |
| 2351 | UDP | Device-to-host sample streaming |
| 2352 | UDP | Device discovery beacon, introduced in phase 9 |

## Control channel (TCP :2350)

One client is served at a time. The host sends one frame and receives exactly
one response. Closing the connection stops any active stream.

### Frame layout

| offset | size | field |
|---|---|---|
| 0 | 2 | magic = `0x4C48`, little-endian ASCII `HL` |
| 2 | 1 | message type |
| 3 | 1 | sequence number, chosen by the host and echoed |
| 4 | 2 | payload length, at most 512 bytes |
| 6 | len | payload |
| 6+len | 2 | CRC-16/CCITT-FALSE over message type through payload |

The CRC uses polynomial `0x1021`, initial value `0xFFFF`, no reflection and
no final XOR. A response has the request type, or type 255 with payload
`error_code u8, request_type u8`. Bad framing drops the TCP connection.

### Message types

| type | name | request payload | response payload |
|---|---|---|---|
| 1 | GetParams | empty | repeated `name NUL, type u8, count u16, writable u8` |
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
`count × sizeof(type)` bytes; char arrays are NUL-padded strings. Names are
ASCII and at most 15 bytes.

`SetPar` rejects non-finite f32 values. `SetBlock` stages a slice of a long
array starting at an element offset; `Commit` atomically activates the first
`len` staged elements. Their codecs are part of v2, while the table-backed
firmware implementation is introduced in phase 5.

`StreamSetup` requires `decimation ≥ 1`, at least one source, and every source
id to be less than `n_sources`. `count = 0` streams continuously. Reconfiguring
an active stream returns busy, preventing a packet-layout change mid-stream.

`uptime_ms` wraps after approximately 49.7 days.

Error codes are: 1 bad frame, 2 unknown type, 3 bad index, 4 bad length,
5 read-only, 6 bad value and 7 busy.

### Parameter discovery

GetParams returns each complete definition in registry order, so names and
metadata cannot become misaligned. Hosts must discover by name and must not
cache indices across connections.

The v2 base registry is:

| name | type | access | meaning |
|---|---|---|---|
| firmware | c×16 | ro | firmware identification |
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

Experiment read-only values, rig parameters and controller parameters follow
the base registry. For `cbc-rig`, these include `laser`,
`rig_laser_range` and `rig_out_channel`. Controller names depend on the
compile-time selected controller.

Fourier coefficients use `[mean, a_1..a_K, b_1..b_K]`, representing
`mean + Σ_k a_k cos(kθ) + b_k sin(kθ)`. The default build uses K = 16.

### Source discovery

GetSources returns the source table. A source id is its zero-based position
in this table. The table is assembled as:

1. experiment inputs;
2. controller telemetry;
3. `target`, `forcing`, `table`, `out`, all in volts.

Names are unique ASCII strings of at most 15 bytes; units are at most 7
bytes. `cbc-rig` currently begins with `adc0` through `adc7` in volts and
`laser` in millimetres. Hosts resolve requested source names from this table;
there are no protocol-wide fixed source ids.

## Stream channel (UDP :2351)

The device sends packets to the TCP peer address and the port supplied in
StreamStart. Selected per-tick values are batched into datagrams no larger
than 1472 bytes and flushed at least every 5 ms.

| offset | size | field |
|---|---|---|
| 0 | 2 | magic = `0x4C48` |
| 2 | 1 | version = 2 |
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

This v2 extension is reserved for phase 9. The request is `magic u16, 0x01`.
The response is `magic u16, 0x02, version u8, control_port u16, mac[6],
experiment c×16, firmware c×16`. Until phase 9 hardware firmware does not
bind this port; the host simulator responds now so discovery clients can be
developed independently.

## Known-answer vectors

- `crc16("123456789") = 0x29B1`, `crc16("") = 0xFFFF`,
  `crc16([00]) = 0xE1F0`.
- GetParams request, sequence 1:
  `48 4C 01 01 00 00 44 C5`.
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
- Status response, sequence 1, version 2, 17 params, 13 sources, 8000 Hz,
  uptime 42000 ms:
  `48 4C 0A 01 0C 00 02 11 00 0D 00 00 FA 45 10 A4 00 00 03 09`.
- Beacon request: `48 4C 01`.
