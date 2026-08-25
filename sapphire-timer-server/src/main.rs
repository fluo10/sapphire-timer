//! Self-hosted sapphire-timer remote workspace server.
//!
//! Runs the framework's JSON-RPC remote-sync server
//! ([`sapphire_framework::remote_server`]) so a `sapphire-timer` CLI/GUI can use
//! it as a remote workspace (`--remote http://host:port` / a `[workspace.<id>]`
//! `url` entry). One server, one workspace by default (framework issue #86);
//! the protocol carries a `ws` id so multiple workspaces still work.
//!
//! Files are the origin, the DB is a rebuildable cache — everything lives under
//! `--data-dir`.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use clap::Parser;
use sapphire_framework::remote_server::{KeyStore, ServerState, serve};

#[derive(Parser)]
#[command(
    name = "sapphire-timer-server",
    about = "Self-hosted remote workspace server for sapphire-timer",
    version
)]
struct Args {
    /// Address to bind.
    #[arg(long, env = "SAPPHIRE_TIMER_SERVER_ADDR", default_value = "127.0.0.1:8080")]
    addr: SocketAddr,

    /// Data directory (file origins + rebuildable cache + change log + blobs).
    /// Defaults to `<data_dir>/sapphire-timer/server`.
    #[arg(long, env = "SAPPHIRE_TIMER_SERVER_DATA_DIR", value_name = "DIR")]
    data_dir: Option<PathBuf>,

    /// Path to the API key file. Defaults to `<data_dir>/keys.toml`. A missing
    /// file is treated as an empty key store (it is not created); the server
    /// then refuses to start rather than listen unauthenticated. `gen-key`
    /// style management of this file is tracked in framework issue #92.
    #[arg(long, env = "SAPPHIRE_TIMER_SERVER_KEYS", value_name = "FILE")]
    keys: Option<PathBuf>,
}

fn default_data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("sapphire-timer")
        .join("server")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = tracing_subscriber::fmt().try_init();
    let args = Args::parse();

    let data_dir = args.data_dir.unwrap_or_else(default_data_dir);
    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("creating data dir {}", data_dir.display()))?;

    let keys_path = args.keys.unwrap_or_else(|| data_dir.join("keys.toml"));
    let keys = KeyStore::load(&keys_path)
        .with_context(|| format!("loading API keys from {}", keys_path.display()))?;
    let state = ServerState::new(&data_dir).with_keys(Arc::new(keys));
    tracing::info!(addr = %args.addr, data_dir = %data_dir.display(), keys = %keys_path.display(), "sapphire-timer-server starting");

    serve(args.addr, Arc::new(state))
        .await
        .context("server failed")?;
    Ok(())
}
