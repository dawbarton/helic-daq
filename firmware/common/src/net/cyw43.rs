//! CYW43439 station-mode backend for Raspberry Pi Pico 2W experiments.

use cyw43::{Control, JoinOptions, PowerManagementMode};
use cyw43_pio::{PioSpi, DEFAULT_CLOCK_DIVIDER};
use defmt::{info, unwrap, warn};
use embassy_executor::Spawner;
use embassy_net::Stack;
use embassy_rp::clocks::RoscRng;
use embassy_rp::dma;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::interrupt::typelevel::{Binding, DMA_IRQ_0, PIO1_IRQ_0};
use embassy_rp::peripherals::{DMA_CH0, PIN_23, PIN_24, PIN_25, PIN_29, PIO1};
use embassy_rp::pio::{self, Pio};
use embassy_rp::Peri;
use embassy_time::Timer;
use static_cell::StaticCell;

use super::NetConfig;

type WifiSpi = PioSpi<'static, PIO1, 0>;
type WifiBus = cyw43::SpiBus<Output<'static>, WifiSpi>;
type WifiRunner = cyw43::Runner<'static, WifiBus>;
type NetRunner = embassy_net::Runner<'static, cyw43::NetDriver<'static>>;

static STATE: StaticCell<cyw43::State> = StaticCell::new();

pub struct WifiParts {
    pub pio: Peri<'static, PIO1>,
    pub pwr: Peri<'static, PIN_23>,
    pub dio: Peri<'static, PIN_24>,
    pub cs: Peri<'static, PIN_25>,
    pub clk: Peri<'static, PIN_29>,
    pub dma: Peri<'static, DMA_CH0>,
}

pub async fn init(
    spawner: Spawner,
    parts: WifiParts,
    pio_irq: impl Binding<PIO1_IRQ_0, pio::InterruptHandler<PIO1>> + 'static,
    dma_irq: impl Binding<DMA_IRQ_0, dma::InterruptHandler<DMA_CH0>> + 'static,
    ssid: &'static str,
    password: &'static str,
    config: NetConfig,
) -> (Stack<'static>, Control<'static>, [u8; 6]) {
    let pwr = Output::new(parts.pwr, Level::Low);
    let cs = Output::new(parts.cs, Level::High);
    let mut pio = Pio::new(parts.pio, pio_irq);
    let spi = PioSpi::new(
        &mut pio.common,
        pio.sm0,
        DEFAULT_CLOCK_DIVIDER,
        pio.irq0,
        cs,
        parts.dio,
        parts.clk,
        dma::Channel::new(parts.dma, dma_irq),
    );

    // These exact blob-wrapper and driver versions are pinned together in
    // Cargo.toml; changing either requires Pico 2W RF and join verification.
    let (device, mut control, runner) = cyw43::new(
        STATE.init(cyw43::State::new()),
        pwr,
        spi,
        &cyw43_setup::FW,
        &cyw43_setup::NVRAM,
    )
    .await;
    spawner.spawn(unwrap!(wifi_task(runner)));
    control.init(cyw43_setup::CLM).await;
    control
        .set_power_management(PowerManagementMode::None)
        .await;

    loop {
        info!("wifi: joining {}", ssid);
        match control
            .join(ssid, JoinOptions::new(password.as_bytes()))
            .await
        {
            Ok(()) => break,
            Err(error) => {
                warn!("wifi join failed: {:?}", error);
                Timer::after_secs(1).await;
            }
        }
    }
    let mac = control.address().await;

    let (stack, runner) = super::new(device, config, RoscRng.next_u64());
    spawner.spawn(unwrap!(net_task(runner)));
    stack.wait_config_up().await;
    info!("wifi: IPv4 configuration acquired");
    (stack, control, mac)
}

#[embassy_executor::task]
async fn wifi_task(runner: WifiRunner) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn net_task(mut runner: NetRunner) -> ! {
    runner.run().await
}
