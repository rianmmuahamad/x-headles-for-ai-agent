//! `xhl-mcp` — MCP server (stdio) yang mengekspos operasi xhl sebagai tool agent.
//!
//! Aturan adapter:
//! - Tidak ada logika bisnis di sini: setiap tool memanggil service di `xhl-core`.
//! - Error selalu dipetakan lewat `XhlError::agent_safe_message()`; pesan `Debug`
//!   mentah tidak pernah keluar ke agent.
//! - Cookie/header tidak pernah menjadi argumen atau hasil tool.
//! - stdout milik protokol MCP, karena itu seluruh logging menuju stderr.

mod server;
mod tools;

use std::process::ExitCode;

use clap::Parser;
use rmcp::{transport::stdio, ServiceExt};
use xhl_core::config::Config;

use crate::server::XhlServer;

#[derive(Parser, Debug)]
#[command(
    name = "xhl-mcp",
    version,
    about = "MCP server untuk X Headless — kendalikan akun X dari agent AI"
)]
struct Args {
    /// Akun yang dipakai (default: $XHL_ACCOUNT atau "default").
    #[arg(long)]
    account: Option<String>,

    /// Jangan pernah menunggu jendela rate limit; kembalikan error sebagai gantinya.
    #[arg(long)]
    no_wait: bool,
}

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();
    let args = Args::parse();

    match serve(args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            // Satu-satunya keluaran yang boleh ditulis ke stderr: stdout milik protokol.
            eprintln!("xhl-mcp gagal: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Logging ke **stderr**: stdout hanya boleh berisi pesan protokol MCP.
fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};

    let filter = EnvFilter::try_from_env("XHL_LOG").unwrap_or_else(|_| EnvFilter::new("warn"));
    let _ = fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .try_init();
}

async fn serve(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let mut config = Config::from_env()?;
    if let Some(account) = args.account {
        config.account = account;
    }
    if args.no_wait {
        config.no_wait = true;
    }
    config.ensure_data_dir()?;

    tracing::info!(
        account = %config.account,
        profile = config.http_profile.name,
        "xhl-mcp dimulai"
    );

    let service = XhlServer::new(config).serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
