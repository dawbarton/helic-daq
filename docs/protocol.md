# HELIC-DAQ wire protocol v1

Authoritative specification. Implemented by `helic-proto` (Rust, used by the
firmware) and `host/helic_daq/protocol.py` (Python); both are unit-tested
against the known-answer vectors at the end of this document.

All multi-byte fields are **little-endian**. The device listens on:

| Port | Transport | Purpose |
|---|---|---|
| 2350 | TCP | Control: parameter get/set, stream control, status |
| 2351 | UDP | Sample streaming (device → host) |

## Control channel (TCP :2350)

One client at a time. Strict request/response: the host sends a frame, the
device replies with exactly one frame. Closing the connection stops any
active stream.

### Frame layout

| offset | size | field |
|---|---|---|
| 0 | 2 | magic = `0x4C48` (little-endian ASCII `HL`) |
| 2 | 1 | type (message type) |
| 3 | 1 | seq — chosen by the host, echoed in the response |
| 4 | 2 | len — payload length (max 512) |
| 6 | len | payload |
| 6+len | 2 | CRC-16/CCITT-FALSE over bytes 2..6+len (type through payload) |

CRC-16/CCITT-FALSE: polynomial `0x1021`, initial value `0xFFFF`, no
reflection, no final XOR.

A response has the same type as the request, or type `0xFF` (Error) with
payload `[error_code u8, request_type u8]`. Malformed frames (bad magic,
oversize, CRC failure) cause the device to drop the connection, since
resynchronisation inside a TCP stream is not meaningful.

### Message types

| type | name | request payload | response payload |
|---|---|---|---|
| 1 | GetParNames | — | each parameter name, NUL-terminated, in index order |
| 2 | GetParInfo | — | per parameter: `type u8, count u16, writable u8` |
| 3 | GetPar | `index u16` × n | raw values concatenated in request order |
| 4 | SetPar | `index u16`, raw value (`count × size` bytes) | — |
| 5 | SetBlock | *reserved* (staged writes for long arrays) | |
| 6 | Commit | *reserved* (atomic activation of staged blocks) | |
| 7 | StreamSetup | `decimation u16, count u32, n u8, source u8 × n` | — |
| 8 | StreamStart | `port u16` (host UDP port; target IP = TCP peer) | — |
| 9 | StreamStop | — | — |
| 10 | Status | — | `version u8, n_params u16, sample_rate f32, uptime_ms u32` |
| 255 | Error | — | `error_code u8, request_type u8` |

Parameter **type codes** are Python `struct` format characters:
`B b H h I i f c` (u8, i8, u16, i16, u32, i32, f32, char). A parameter's
wire size is `count × sizeof(type)`; `char` parameters are NUL-padded
strings.

`StreamSetup`: `decimation ≥ 1` keeps every decimation-th sample;
`count` is the total number of records to send before the stream stops
itself (0 = continuous); `source` ids are listed below. A `StreamSetup`
while a stream is running is rejected with error 7 (busy) — send
`StreamStop` first, so the packet layout never changes mid-session.

`SetPar`: non-finite f32 values (NaN, ±∞) are rejected with error 6
(bad value) — they would otherwise propagate to the DAC output.

`Status`: `uptime_ms` is a u32 and wraps after ~49.7 days.

**Error codes:** 1 bad frame, 2 unknown type, 3 bad index, 4 bad length,
5 read-only, 6 bad value, 7 busy (command queue full or stream running —
stop the stream / retry).

### Parameters

Discovered at connect; do **not** hard-code indices, they may change
between firmware versions. Current registry (firmware 0.1):

| name | type | access | meaning |
|---|---|---|---|
| firmware | c×16 | ro | firmware identification string |
| sample_freq | f | ro | sample rate, Hz |
| ticks | I | ro | RT loop tick counter |
| loop_time_last | I | ro | last tick processing time, µs |
| loop_time_max | I | ro | max tick processing time, µs |
| clock_jitter | I | ro | worst tick-spacing excess over nominal, µs |
| overruns | I | ro | ticks that exceeded the sample period |
| tick_timeouts | I | ro | sample-clock waits that timed out |
| records_dropped | I | ro | stream records dropped at source (cumulative) |
| laser | f | ro | latest laser distance, mm |
| freq | f | rw | fundamental frequency of the periodic generators, Hz |
| target_coeffs | f×33 | rw | controller reference Fourier series (see below) |
| forcing_coeffs | f×33 | rw | feed-forward forcing Fourier series |
| ctrl_reset | I | rw | write non-zero to reset controller state |
| ctrl_* | f | rw | active controller's own parameters (e.g. `ctrl_kp`) |

**Coefficient layout** (K = 16 harmonics in the default build, so 33
values): `[mean, a_1..a_K, b_1..b_K]` representing
`mean + Σ_k a_k·cos(kθ) + b_k·sin(kθ)`. Target and forcing share one phase
accumulator, so their harmonics stay phase-locked to each other. Writes
are applied atomically at a sample boundary.

## Stream channel (UDP :2351)

The device sends packets to the TCP peer's IP at the port given in
`StreamStart`. Records are the per-tick values selected by `StreamSetup`,
batched into packets of at most 1472 bytes and flushed at least every 5 ms.

### Packet layout

| offset | size | field |
|---|---|---|
| 0 | 2 | magic = `0x4C48` (little-endian ASCII `HL`) |
| 2 | 1 | version = 1 |
| 3 | 1 | n_sources — values per record |
| 4 | 4 | seq — packet counter (gaps = packets lost in transit) |
| 8 | 4 | first_index — sample index of the first record (wraps at 2³²) |
| 12 | 4 | dropped — cumulative records dropped at source (ring overflow) |
| 16 | 2 | decimation |
| 18 | 2 | n_records |
| 20 | … | `n_records × n_sources` f32 values, record-major |

Within a packet, record i has sample index `first_index + i × decimation`.

### Stream sources

| id | name | value |
|---|---|---|
| 0–7 | adc0–adc7 | ADC channels, volts |
| 8 | laser | laser distance, mm |
| 9 | target | reference generator output, volts |
| 10 | forcing | forcing generator output, volts |
| 11 | out | value written to the output DAC channel, volts |

## Known-answer test vectors

- `crc16("123456789") = 0x29B1`, `crc16("") = 0xFFFF`,
  `crc16([0x00]) = 0xE1F0`, `crc16([0x0A,0x01,0x00,0x00]) = 0xDB5B`
- Status request, seq 1, empty payload encodes to
  `48 4C 0A 01 00 00 5B DB`.
