use std::path::Path;

use anyhow::Result;
use clap::Args;
use sapphire_timer_core::ops;

use super::{hms, resolve_timer};

#[derive(Args)]
pub struct LogArgs {
    /// How many of the most recent sessions to show.
    #[arg(long, short, default_value_t = 20)]
    limit: usize,
}

pub fn run(dir: Option<&Path>, args: LogArgs) -> Result<()> {
    let timer = resolve_timer(dir)?;
    let sessions = ops::list_sessions(&timer)?;

    if sessions.is_empty() {
        println!("no sessions yet — run `sapphire-timer start <preset>`");
        return Ok(());
    }

    // Newest last is how the log reads on disk; show newest first here.
    for s in sessions.iter().rev().take(args.limit) {
        let mark = match s.outcome {
            sapphire_timer_core::Outcome::Completed => "✓",
            sapphire_timer_core::Outcome::Interrupted => "×",
        };
        println!(
            "{} {}  {:<12} {:>8}  {}",
            mark,
            s.started_at.format("%Y-%m-%d %H:%M"),
            s.preset_name,
            hms(s.elapsed_secs),
            s.comment
        );
    }
    Ok(())
}
