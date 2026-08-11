//! Non-blocking RP2350 PIO transport for two SSI absolute encoders.

use embassy_rp::clocks::clk_sys_freq;
use embassy_rp::gpio::Level;
use embassy_rp::pio::{Common, Direction, Instance, PioPin, ShiftDirection, StateMachine};
use embassy_rp::{pac, Peri};
use fixed::traits::ToFixed;

use crate::raw_pio::RawPioInstance;

pub struct DualSsiReader<'d, PIO: Instance, const SM: usize> {
    /// Retains Embassy's ownership and one-time state-machine configuration.
    _sm: StateMachine<'d, PIO, SM>,
    /// Matching raw PIO register block for the SRAM-resident FIFO hot path.
    raw: pac::pio::Pio,
    bit_count: u32,
}

impl<'d, PIO: RawPioInstance + 'd, const SM: usize> DualSsiReader<'d, PIO, SM> {
    /// Configure a dual-input SSI state machine.
    ///
    /// The typed PIO instance selects its matching PAC register block, so a
    /// caller cannot accidentally pair (for example) a PIO0 state machine with
    /// PIO1 FIFO registers.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        common: &mut Common<'d, PIO>,
        mut sm: StateMachine<'d, PIO, SM>,
        clock: Peri<'d, impl PioPin + 'd>,
        data_0: Peri<'d, impl PioPin + 'd>,
        data_1: Peri<'d, impl PioPin + 'd>,
        bits: u8,
        bit_rate_hz: u32,
    ) -> Self {
        assert!((1..=16).contains(&bits));
        assert!((1..=4_000_000).contains(&bit_rate_hz));

        let program = pio::pio_asm!(
            r#"
                .side_set 1
                .wrap_target
                pull block       side 1
                mov x, osr        side 1
                mov isr, null     side 1
            bitloop:
                nop               side 0
                in pins, 2        side 0
                nop               side 1
                jmp x-- bitloop   side 1
                nop               side 0 [1]
                nop               side 1 [1]
                push block        side 1
                .wrap
            "#
        );
        let program = common.load_program(&program.program);
        let clock = common.make_pio_pin(clock);
        let data_0 = common.make_pio_pin(data_0);
        let data_1 = common.make_pio_pin(data_1);

        let mut config = embassy_rp::pio::Config::default();
        config.use_program(&program, &[&clock]);
        config.set_in_pins(&[&data_0, &data_1]);
        config.shift_in.direction = ShiftDirection::Left;
        let sys_hz = clk_sys_freq().to_fixed::<fixed::FixedU64<fixed::types::extra::U8>>();
        let instruction_hz =
            (bit_rate_hz * 4).to_fixed::<fixed::FixedU64<fixed::types::extra::U8>>();
        config.clock_divider = (sys_hz / instruction_hz).to_fixed();

        sm.set_config(&config);
        sm.set_pins(Level::High, &[&clock]);
        sm.set_pin_dirs(Direction::Out, &[&clock]);
        sm.set_pin_dirs(Direction::In, &[&data_0, &data_1]);
        sm.set_enable(true);

        Self {
            _sm: sm,
            raw: PIO::raw(),
            bit_count: u32::from(bits - 1),
        }
    }

    /// Begin one transaction. A successful call never waits for the encoder.
    #[unsafe(link_section = ".data.ram_func")]
    pub fn start(&mut self) -> bool {
        if self.raw.fstat().read().txfull() & (1 << SM) != 0 {
            return false;
        }
        self.raw.txf(SM).write_value(self.bit_count);
        true
    }

    /// Return the completed word, or `None` while the transaction is running.
    #[unsafe(link_section = ".data.ram_func")]
    pub fn read(&mut self) -> Option<u32> {
        if self.raw.fstat().read().rxempty() & (1 << SM) != 0 {
            return None;
        }
        Some(self.raw.rxf(SM).read())
    }
}
