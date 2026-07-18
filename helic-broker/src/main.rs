//! Command-line entry point for the loopback-only HELIC-DAQ broker.

use anyhow::Result;
use clap::Parser;
use helic_broker::config::Config;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    let config = Config::parse();
    config.validate()?;
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(&config.log_level))
        .init();
    helic_broker::server::run(config).await
}
