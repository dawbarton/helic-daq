//! Raw, SRAM-resident blocking SPI transfers for the core-1 hot path.
//!
//! `embassy_rp::spi` and `SpiDeviceWithConfig` are functionally correct, but
//! their per-transaction code executes from XIP flash. Core-0 network bursts
//! evict the shared XIP cache, and phase-resolved diagnostics measured a
//! nominally ~13 µs ADC read stretching beyond a whole 125 µs sample period
//! while core 0 handled TCP traffic. These helpers keep every instruction of
//! the tick's SPI path in `.data.ram_func` and program the SSP registers
//! directly from a precomputed configuration, so a transfer's duration no
//! longer depends on the XIP cache.
//!
//! Init-time configuration (pin funcsel, pad setup, device reset sequences)
//! stays with the embassy drivers; only the per-tick data path bypasses them.

use embassy_rp::pac;

/// Precomputed SSP clock/format configuration (divider maths mirrors
/// `embassy_rp::spi::calc_prescs`). Computed once at init so the hot path
/// performs register writes only.
#[derive(Clone, Copy)]
pub struct RawSpiConfig {
    cpsr: u8,
    scr: u8,
    /// SPO: clock polarity (idle high when true).
    cpol: bool,
    /// SPH: clock phase (capture on second transition when true).
    cpha: bool,
}

impl RawSpiConfig {
    pub fn new(freq: u32, cpol: bool, cpha: bool) -> Self {
        let clk_peri = embassy_rp::clocks::clk_peri_freq();
        // spi_freq = clk_peri / presc / postdiv, presc even and 2..=254,
        // postdiv 1..=256. Dividing the ratio by 2 removes the evenness
        // requirement, exactly as embassy-rp does.
        let ratio = clk_peri.div_ceil(freq * 2);
        assert!(
            (1..=127 * 256).contains(&ratio),
            "unreachable SPI frequency"
        );
        let presc = ratio.div_ceil(256);
        let postdiv = if presc == 1 {
            ratio
        } else {
            ratio.div_ceil(presc)
        };
        Self {
            cpsr: (presc * 2) as u8,
            scr: (postdiv - 1) as u8,
            cpol,
            cpha,
        }
    }
}

/// Level control for a pin already configured as a SIO output elsewhere
/// (an `embassy_rp::gpio::Output` must exist and stay alive; this type only
/// writes the atomic set/clear registers, which is safe alongside it).
#[derive(Clone, Copy)]
pub struct SioOutPin {
    mask: u32,
    bank: usize,
}

impl SioOutPin {
    pub const fn new(pin: u8) -> Self {
        Self {
            mask: 1 << (pin % 32),
            bank: (pin / 32) as usize,
        }
    }

    #[inline(always)]
    fn set_low(self) {
        pac::SIO.gpio_out(self.bank).value_clr().write_value(self.mask);
    }

    #[inline(always)]
    fn set_high(self) {
        pac::SIO.gpio_out(self.bank).value_set().write_value(self.mask);
    }
}

/// Blocking full-duplex 8-bit transfer with chip select, entirely from SRAM.
///
/// Reprogrammes clock and format registers first, so devices with different
/// configurations can share the bus exactly as with `SpiDeviceWithConfig`.
/// `buf` is transmitted and overwritten with the received bytes.
#[unsafe(link_section = ".data.ram_func")]
#[inline(never)]
pub fn transfer_in_place(spi: pac::spi::Spi, cfg: RawSpiConfig, cs: SioOutPin, buf: &mut [u8]) {
    spi.cr1().write(|w| w.set_sse(false));
    spi.cpsr().write(|w| w.set_cpsdvsr(cfg.cpsr));
    spi.cr0().write(|w| {
        w.set_dss(7); // 8-bit frames
        w.set_spo(cfg.cpol);
        w.set_sph(cfg.cpha);
        w.set_scr(cfg.scr);
    });
    spi.cr1().write(|w| w.set_sse(true));
    // Drain any stale RX left by a previous user of the bus.
    while spi.sr().read().rne() {
        let _ = spi.dr().read();
    }

    cs.set_low();
    let n = buf.len();
    let mut tx = 0;
    let mut rx = 0;
    while rx < n {
        if tx < n && spi.sr().read().tnf() {
            spi.dr().write(|w| w.set_data(buf[tx] as u16));
            tx += 1;
        }
        if spi.sr().read().rne() {
            buf[rx] = spi.dr().read().data() as u8;
            rx += 1;
        }
    }
    while spi.sr().read().bsy() {}
    cs.set_high();
}
