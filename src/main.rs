use anyhow::Result;
use rmcp::{ServiceExt, transport::io::stdio};

mod cargo;
mod server;
mod tools;

// npx @modelcontextprotocol/inspector cargo run
#[tokio::main]
async fn main() -> Result<()> {
    eprintln!("Starting cargo-copilot");
    let service = server::Copilot::new().serve(stdio()).await?;

    service.waiting().await?;
    Ok(())
}
