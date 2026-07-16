use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

mod commands;

#[derive(Parser)]
#[command(
    name = "sapphire-timer",
    about = "Preset-based timer that keeps your data alive as plain text",
    version
)]
struct Cli {
    /// Path to the timer root (the directory containing `.sapphire-timer/`).
    /// Overrides the automatic upward search from the current directory.
    #[arg(long, env = "SAPPHIRE_TIMER_DIR", global = true, value_name = "DIR")]
    timer_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a timer workspace with starter presets.
    Init {
        /// Where to create it. Defaults to the current directory.
        path: Option<PathBuf>,
    },
    /// Inspect timer presets.
    Preset {
        #[command(subcommand)]
        action: commands::preset::PresetCommand,
    },
    /// Run a preset's countdown in the foreground and record the session.
    Start(commands::start::StartArgs),
    /// List recorded sessions.
    Log(commands::log::LogArgs),
    /// Full-text search across presets and session logs.
    Search(commands::search::SearchArgs),
    /// Inspect and maintain the search index.
    Cache {
        #[command(subcommand)]
        action: commands::cache::CacheCommand,
    },
    /// Commit, pull and push via the sync backend, then re-index.
    Sync,
}

fn main() -> Result<()> {
    sapphire_timer_core::init_app_context();
    let cli = Cli::parse();
    let dir = cli.timer_dir.as_deref();

    match cli.command {
        Command::Init { path } => commands::init::run(path.as_deref()),
        Command::Preset { action } => commands::preset::run(dir, action),
        Command::Start(args) => commands::start::run(dir, args),
        Command::Log(args) => commands::log::run(dir, args),
        Command::Search(args) => commands::search::run(dir, args),
        Command::Cache { action } => commands::cache::run(dir, action),
        Command::Sync => commands::sync::run(dir),
    }
}
