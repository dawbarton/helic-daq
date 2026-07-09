# Periodic signal generator

## Design

**Phase accumulator.** Keep phase as `uint32` (or `uint64`), where the full range represents [0, 2π). Each sample:

```
phase += increment        // wraps mod 2^32 automatically, exactly
increment = round(f0 / fs * 2^32)
```

Frequency resolution is fs/2³² = 8000/2³² ≈ **1.9 µHz**, so 0.1 Hz is hit to ~2×10⁻⁵ relative error. If you want essentially exact frequencies, use a 64-bit accumulator (fs/2⁶⁴ ≈ 4×10⁻¹³ Hz) and take the top 32 bits for evaluation; 64-bit adds are cheap on the Cortex-M33.

**Harmonic phases for free.** The k-th harmonic's phase is simply

```
phase_k = (uint32)(k * phase)   // wrapping multiply = exact mod 2^32
```

This is *exact*: all harmonics stay perfectly phase-coherent with the fundamental forever, with zero drift. No per-harmonic accumulators needed. This matters for CBC-style forcing where you need the harmonics locked.

**Sine/cosine evaluation.** Two good options:

1. **Interpolated LUT.** Table of, say, 1024 float32 sin values over one period (4 KiB, fits comfortably in SRAM). Use the top 10 bits of `phase_k` as index, next bits as fraction, linear interpolation. Max error with N = 1024 is ≈ (2π/N)²/8 ≈ 4.7×10⁻⁶; with N = 4096 it is ≈ 2.9×10⁻⁷, below float32 rounding anyway. Cosine is the same table with a quarter-period offset added to the integer phase before lookup, again exact.
2. **Chebyshev recurrence.** Evaluate sin θ, cos θ once for the base phase, then sₖ₊₁ = 2c₁sₖ − sₖ₋₁ (likewise for cₖ). Two FMAs per harmonic per function. Error grows ~O(k) ulps but resets every sample, so it is fine for K ≲ 50.

Option 1 is simpler and errors don't compound across harmonics; I would default to it.

**Phase-to-error budget.** With a 32-bit phase truncated to 24-ish effective bits at evaluation, worst-case phase quantisation is ~4×10⁻⁷ rad, giving amplitude errors well below a 16-bit DAC's LSB (1.5×10⁻⁵). Nothing here requires doubles.

## Performance

At 150 MHz you have ~18,750 cycles per sample at 8 kHz. LUT lookup plus interpolation plus multiply-accumulate is roughly 20–30 cycles per harmonic term; 20 harmonics with both sin and cos terms is ~1,000 cycles per sample, so you are using around 5% of one core. The M33's single-precision FPU handles this natively; avoid anything that promotes to f64.

## Practical notes

- Pace output with a hardware timer plus DMA double-buffering (compute blocks of 64–256 samples), rather than computing per-sample in an ISR. In Embassy/Rust this maps naturally onto a DMA-fed PWM or external DAC over SPI.
- Sum harmonics in float32, scale once, convert to the DAC integer format with rounding; leave headroom so Σ|aₖ| + Σ|bₖ| cannot clip.
- If you change f0 or coefficients at runtime, update `increment` atomically; phase continuity is automatic since the accumulator never resets.
