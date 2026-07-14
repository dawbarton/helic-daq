//! RP2350 PIO edge-to-edge period capture for a positive digital pulse.
//!
//! A state machine measures complete rising-edge intervals independently of
//! core load and publishes non-blocking counter values through its RX FIFO.

use embassy_rp::clocks::clk_sys_freq;
use embassy_rp::gpio::Pull;
use embassy_rp::pio::{Common, Direction, FifoJoin, Instance, PioPin, StateMachine};
use embassy_rp::Peri;
use fixed::traits::ToFixed;

pub struct PulsePeriodReader<'d, PIO: Instance, const SM: usize> {
    sm: StateMachine<'d, PIO, SM>,
}

impl<'d, PIO: Instance + 'd, const SM: usize> PulsePeriodReader<'d, PIO, SM> {
    pub fn new(
        common: &mut Common<'d, PIO>,
        mut sm: StateMachine<'d, PIO, SM>,
        input: Peri<'d, impl PioPin + 'd>,
        counter_hz: u32,
    ) -> Self {
        assert!((1..=10_000_000).contains(&counter_hz));
        let program = pio::pio_asm!(
            r#"
                wait 0 pin 0
                wait 1 pin 0
                mov x, !null
            high:
                jmp pin high_count
            low:
                jmp pin edge
                jmp x-- low
            high_count:
                jmp x-- high
            edge:
                mov isr, !x
                push noblock
                mov x, !null
                jmp high
            "#
        );
        let program = common.load_program(&program.program);
        let mut input = common.make_pio_pin(input);
        input.set_pull(Pull::Down);

        let mut config = embassy_rp::pio::Config::default();
        config.use_program(&program, &[]);
        config.set_in_pins(&[&input]);
        config.set_jmp_pin(&input);
        config.fifo_join = FifoJoin::RxOnly;
        let sys_hz = clk_sys_freq().to_fixed::<fixed::FixedU64<fixed::types::extra::U8>>();
        let instruction_hz =
            (counter_hz * 2).to_fixed::<fixed::FixedU64<fixed::types::extra::U8>>();
        config.clock_divider = (sys_hz / instruction_hz).to_fixed();

        sm.set_config(&config);
        sm.set_pin_dirs(Direction::In, &[&input]);
        sm.set_enable(true);
        Self { sm }
    }

    pub fn read(&mut self) -> Option<u32> {
        self.sm.rx().try_pull()
    }

    pub fn stalled(&mut self) -> bool {
        self.sm.rx().stalled()
    }
}
