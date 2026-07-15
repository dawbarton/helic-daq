//! RP2350 PIO edge-to-edge period capture for a positive digital pulse.
//!
//! A state machine measures complete rising-edge intervals independently of
//! core load and publishes non-blocking counter values through its RX FIFO.

use embassy_rp::clocks::clk_sys_freq;
use embassy_rp::gpio::Pull;
use embassy_rp::pio::{Common, Direction, FifoJoin, Instance, PioPin, StateMachine};
use embassy_rp::{pac, Peri};
use fixed::traits::ToFixed;

pub struct PulsePeriodReader<'d, PIO: Instance, const SM: usize> {
    /// Retains Embassy's ownership and one-time state-machine configuration.
    _sm: StateMachine<'d, PIO, SM>,
    /// Matching raw PIO register block for the SRAM-resident FIFO hot path.
    raw: pac::pio::Pio,
}

impl<'d, PIO: Instance + 'd, const SM: usize> PulsePeriodReader<'d, PIO, SM> {
    /// Configure an edge-period state machine.
    ///
    /// `raw` must address the same PIO block represented by `common` and
    /// `sm`. The typed Embassy state machine retains ownership; the duplicate
    /// PAC handle is used only for bounded FIFO access from the SRAM hot path.
    pub fn new(
        raw: pac::pio::Pio,
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
        Self { _sm: sm, raw }
    }

    #[unsafe(link_section = ".data.ram_func")]
    pub fn read(&mut self) -> Option<u32> {
        if self.raw.fstat().read().rxempty() & (1 << SM) != 0 {
            return None;
        }
        Some(self.raw.rxf(SM).read())
    }

    #[unsafe(link_section = ".data.ram_func")]
    pub fn stalled(&mut self) -> bool {
        let fdebug = self.raw.fdebug();
        let stalled = fdebug.read().rxstall() & (1 << SM) != 0;
        if stalled {
            fdebug.write(|w| w.set_rxstall(1 << SM));
        }
        stalled
    }
}
