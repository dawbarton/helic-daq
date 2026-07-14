# Wiring the `rtc` / BBB-DAQ analog cape to the W5500-EVB-Pico2

Interim wiring reference for driving the existing **BBB-DAQ** analog board
(from [github.com/dawbarton/rtc](https://github.com/dawbarton/rtc),
`hardware/`) — an **AD7609** ADC + **AD5064** DAC — from the helic-daq firmware
on the RP2350, until a purpose-built board is designed.

Derived from the rtc schematics (`hardware/schematics/rtc-sheet{1,2}.sch`) and
the pinout planner (`hardware/pins.xlsx`, sheet titled "BBB-DAQ").

## Logic level: 3.3 V — connect directly, no shifters

The cape is a **BeagleBone Black cape**: every digital line runs from the
converters straight to the BBB P8/P9 expansion headers with **no level
shifters** anywhere in the schematic, and `pins.xlsx` states plainly *"All pins
are 3.3 V logic."* The BBB's GPIO is 3.3 V and not 5 V-tolerant, so the
converter digital interface (AD7609 V_DRIVE etc.) is already 3.3 V. The RP2350
is also 3.3 V CMOS → **wire the data/control lines directly.** (The analog
section still needs its own +5 V / references — see Power below.)

## Pin map

RP2350 physical pins are the standard Pico2 40-pin header (all signals below
are on the **left edge**, pins 4–20). Cape pins are BBB header P8/P9.

### Shared SPI (single RP2350 SPI1 bus → both converters)

The rtc board put the ADC and DAC on **separate** BBB SPI ports (SCLK0/SCLK1),
but the helic-daq firmware drives one shared bus. So **jumper GP10 (SCK) to both**
converter clocks; MOSI goes only to the DAC, MISO only from the ADC.

| RP2350 | Pico2 pin | Cape signal | Cape pin |
|--------|-----------|-------------|----------|
| GP10 SCK  | 14 | ADC SCLK1  | P9&nbsp;31 |
| GP10 SCK  | 14 | DAC SCLK0  | P9&nbsp;22 |
| GP11 MOSI | 15 | DAC MOSI0 (DIN, `spi0_d0`) | P9&nbsp;21 |
| GP12 MISO | 16 | ADC MISO1 (DOUT, `spi1_d1`) | P9&nbsp;30 |

### AD7609 (ADC)

| RP2350 | Pico2 pin | Cape signal | Cape pin |
|--------|-----------|-------------|----------|
| GP13 | 17 | ADC ~CS (CS1) | P9&nbsp;28 |
| GP8  | 11 | CONVST (A+B)  | P8&nbsp;10 |
| GP7  | 10 | BUSY (input)  | P8&nbsp;11 |
| GP6  | 9  | RESET         | P8&nbsp;12 |
| GP5  | 7  | RANGE         | P8&nbsp;14 |
| GP4  | 6  | OS2           | P8&nbsp;16 |
| GP3  | 5  | OS1           | P8&nbsp;17 |
| GP2  | 4  | OS0           | P8&nbsp;18 |
| —    | —  | **STBY → tie 3V3** | P8&nbsp;15 |

### AD5064 (DAC)

| RP2350 | Pico2 pin | Cape signal | Cape pin |
|--------|-----------|-------------|----------|
| GP9  | 12 | DAC ~SYNC (CS0) | P9&nbsp;17 |
| GP15 | 20 | ~LDAC           | P8&nbsp;7  |
| —    | —  | **~CLR → tie 3V3** | P8&nbsp;8 |

### Straps (BBB drove these; helic-daq firmware does not)

- **STBY (P8 15) → 3V3.** AD7609 STBY is active-low; must be high for normal
  operation (low = standby).
- **~CLR (P8 8) → 3V3.** AD5064 async clear, idle-high (matches the driver's
  "~CLR tied high" assumption).

### Power and ground

The cape has no on-board regulators — it took its rails from the BBB. Supply:

| Cape rail | Cape pins | Feed from |
|-----------|-----------|-----------|
| 3.3 V logic (DC_3.3V) | P9 3, P9 4 | Pico2 **3V3(OUT), pin 36** |
| +5 V analog (VDD_5V / SYS_5V) | P9 5, P9 6, P9 7, P9 8 | clean +5 V (see note) |
| GND | P8 1–2, P9 1–2, P9 43–46 | Pico2 GND (pins 3/8/13/18/…) — **common all grounds** |

> **Analog supply note.** The AD7609 + references + output op-amps run off +5 V.
> Pico2 **VBUS (pin 40)** is USB 5 V and will work for bring-up, but it is noisy
> and will hurt ADC performance — prefer a clean bench/LDO +5 V for real
> measurements. Always tie its ground to the Pico2 ground.

### Analog output connector J4 (for reference)

| J4 pin | Signal | | J4 pin | Signal |
|--------|--------|-|--------|--------|
| 1 | SYS_5V | | 2 | GND |
| 3 | 4.096 V reference | | 4 | GND |
| 5 | VOUT_A bipolar [−4.096, 4.096] | | 6 | GND |
| 7 | VOUT_C bipolar [−4.096, 4.096] | | 8 | GND |
| 9 | VOUT_D unipolar [0, 4.096] | | 10 | GND |
| 11 | VOUT_B unipolar [0, 4.096] | | 12 | GND |

DAC channel polarity **[A, B, C, D] = [Bipolar, Unipolar, Bipolar, Unipolar]**,
matching `board.rs` `DAC_POLARITY`. DAC reference is 4.096 V (matches
`DAC_VREF`).

## Firmware notes

- **Chip is AD7609, not AD7608.** The digital readout is identical (same AD760x
  18-bit serial frame), so the driver clocks out correct raw codes. The input
  ranges differ: **AD7609 is ±10 V (RANGE low) / ±20 V (RANGE high)**, so the
  driver's `InputRange` is `Bipolar10V` / `Bipolar20V` with 20 V / 40 V spans.
  Analog inputs are **differential** on this part.
- ADC SPI is mode 2 @ 12 MHz; DAC SPI is mode 1 @ 16 MHz (`board.rs`). Both are
  on the same shared bus with separate chip selects.

## Post-wiring bring-up

The RT loop already drives CONVST continuously (that's the climbing
`busy timeouts` when nothing is connected). Once wired:

1. Flash and watch `status_task` — `busy timeouts` should **stop climbing** and
   `overruns` stay low once the ADC asserts/deasserts BUSY. First "alive" sign.
2. Scope: CONVST on GP8/pin 11 = clean square wave at the sample rate; GP14/pin
   19 pulses high per RT tick (timing/load check).
3. Stream over UDP (`helic-daq` CLI) and check a known input voltage reads back
   correctly through the range/scale, and a DAC write appears at J4.
