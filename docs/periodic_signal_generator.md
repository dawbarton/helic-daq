# Periodic signal generator

## Implemented design

`helic-core` represents one turn with the full range of a `u32`. At each
sample the phase accumulator wraps modulo 2³²:

```text
phase = phase + increment
increment = round(f0 / fs * 2^32)
```

The increment is calculated with `f64` when a parameter changes, outside the
real-time path. Per-sample work uses integer phase and `f32` values. Frequency
resolution is `fs / 2^32`, approximately 1.9 µHz at 8 kHz. Changing the
increment does not reset phase, so frequency updates are phase-continuous.

The phase of harmonic `k` is `k * phase` with wrapping multiplication. Every
harmonic is therefore an exact modular multiple of the fundamental and cannot
drift relative to it. The same master phase drives target and forcing series;
locked waveform-table modes derive their phase from it in the same way.

Sine and cosine use a 1024-entry `f32` sine table with linear interpolation.
Cosine is an exact quarter-turn offset. The interpolation error bound is about
4.7×10⁻⁶, below one 16-bit DAC least-significant bit. A Fourier series is

```text
mean + sum(a[k] cos(k phase) + b[k] sin(k phase))
```

with 16 harmonics in the firmware. Coefficients are copied through the
cross-core command queue and replaced together at a sample boundary. The
accumulator also reports phase wrap for period-locked triggers and future
per-period processing.

## Real-time use

The common real-time loop advances the master phase once per hardware-timed
sample, evaluates target and forcing, evaluates any arbitrary-waveform table,
runs the controller, and actuates the selected rig. It intentionally does not
generate DMA-sized blocks: acquisition, feedback and output must complete for
each individual sample.

At 8 kHz and 150 MHz there are 18,750 cycles per tick. Two 16-harmonic series
occupy only a small part of that budget, but timing claims must be checked on
hardware through `loop_time_max`, `overruns` and the experiment's timing pin.

Keep output headroom for the sum of controller, forcing and table
contributions. `FourierCoeffs::amplitude_bound` bounds an individual series,
but firmware does not currently impose a combined per-channel clipping policy.
