mod access;
mod config;
mod protocol;
mod server;
mod upstream;

use anyhow::Result;
use config::Config;

#[tokio::main]
async fn main() -> Result<()> {
    let config = Config::load()?;
    server::run(config).await
}
