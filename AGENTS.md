# CBC-DAQ

A real-time control and data acquisition platform for control-based continuation. It is based on a RP2350 microcontroller using the Rust Embassy framework.

## Hardware details

- W5500-EVB-Pico2 board for control and data management including ethernet via the Wiznet W5500
- AD7608 ADC for analog input (connected via SPI)
- AD5064 DAC for analog output (connected via SPI)
- A Micro-Epsilon OptoNCDT 1420 laser triangulation sensor for displacement measurement (connected via RS422/UART)

## Design goals

- A reliable (i.e., low jitter) sample rate, variable between 1-10kHz (can be restricted preset frequencies, e.g., 1kHz, 2kHz, 4kHz, 8kHz)
- A real-time control loop using, e.g., PID control
  - The precise control mechanism may vary from user to user and the code structure should facilitate easy swapping of controllers at compile time
  - The control loop may include features such as filtering and / or real-time Fourier estimation
- A periodic signal generator used as the reference for the controller and / or output via the DAC
  - The reference is typically represented as a Fourier series with 5-20 harmonics (tuned according to computational resources available)
- An arbitrary signal generator that can be used in single-shot mode or in a periodic manner
- Asynchronous updating of parameters via ethernet and / or USB serial including:
  - Control gains
  - Filter coefficients
  - Fourier coefficients for the periodic signal generator
- Streaming of key data back to the host computer via ethernet and / or USB serial; ideally user selectable data
- One core of the RP2350 used for host communications and one core used for the control loop and data acquisition

## AD7608 ADC

- 8 analogue inputs at 18 bits
- Configurable range from +/-5V to +/-10V
- Parameters such as range and oversampling are determined by individual logic inputs
- Data capture is triggered by the MCU by a logic high signal; conversion finished is signalled by the ADC via a logic low signal on the BUSY pin
- May be swapped out for an AD7606B in the future which uses SPI commands to set parameters rather than logic inputs

## AD5064 DAC

- 4 analogue outputs at 16 bits
- Two outputs are bipolar (external op-amps are used to convert the DAC output from 0-4.096V to -4.096V to +4.096V)
- Two outputs are unipolar
- May be swapped out for an AD5764 in the future

## Micro-Epsilon OptoNCDT 1420 laser triangulation sensor

- Communication is via serial; a RS422 to TTL converter is used for this purpose

## Other peripherals

- Software design should allow for the presence of other peripherals, e.g., SSI-based encoders

## Periodic signal generator

See `docs/periodic_signal_generator.md` for details of the proposed design.

## Arbitrary signal generator

The arbitrary signal should be stored as a look-up table with linear interpolation. It should be possible to adjust the signal timescale using a similar approach to the phase accumulator of the periodic signal generator. 1000-2000 samples in the LUT should be sufficient, though more may be useful if memory constraints permit. The arbitrary signal can be used in single-shot mode or in a periodic manner, depending on the application.

## Host communication

A simple protocol is required for getting / setting parameter values (asynchronously, with minimal overhead) and streaming measurement values back to the host.
