//! Non-blocking RP2350 PIO transport for SSI absolute encoders.

use embassy_rp::clocks::clk_sys_freq;
use embassy_rp::gpio::Level;
use embassy_rp::pio::{Common, Direction, Instance, PioPin, ShiftDirection, StateMachine};
use embassy_rp::Peri;
use fixed::traits::ToFixed;

pub struct SsiReader<'d, PIO: Instance, const SM: usize> {
    sm: StateMachine<'d, PIO, SM>,
    bit_count: u32,
}

impl<'d, PIO: Instance + 'd, const SM: usize> SsiReader<'d, PIO, SM> {
    pub fn new(
        common: &mut Common<'d, PIO>,
        mut sm: StateMachine<'d, PIO, SM>,
        clock: Peri<'d, impl PioPin + 'd>,
        data: Peri<'d, impl PioPin + 'd>,
        bits: u8,
        bit_rate_hz: u32,
    ) -> Self {
        assert!((1..=32).contains(&bits));
        assert!((1..=500_000).contains(&bit_rate_hz));

        let program = pio::pio_asm!(
            r#"
                .side_set 1
                .wrap_target
                pull block       side 1
                mov x, osr        side 1
                mov isr, null     side 1
            bitloop:
                nop               side 0
                in pins, 1        side 0
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
        let data = common.make_pio_pin(data);

        let mut config = embassy_rp::pio::Config::default();
        config.use_program(&program, &[&clock]);
        config.set_in_pins(&[&data]);
        config.shift_in.direction = ShiftDirection::Left;
        let sys_hz = clk_sys_freq().to_fixed::<fixed::FixedU64<fixed::types::extra::U8>>();
        let instruction_hz =
            (bit_rate_hz * 4).to_fixed::<fixed::FixedU64<fixed::types::extra::U8>>();
        config.clock_divider = (sys_hz / instruction_hz).to_fixed();

        sm.set_config(&config);
        sm.set_pins(Level::High, &[&clock]);
        sm.set_pin_dirs(Direction::Out, &[&clock]);
        sm.set_pin_dirs(Direction::In, &[&data]);
        sm.set_enable(true);

        Self {
            sm,
            bit_count: u32::from(bits - 1),
        }
    }

    /// Begin one transaction. A successful call never waits for the encoder.
    pub fn start(&mut self) -> bool {
        self.sm.tx().try_push(self.bit_count)
    }

    /// Return the completed word, or `None` while the transaction is running.
    pub fn read(&mut self) -> Option<u32> {
        self.sm.rx().try_pull()
    }
}
