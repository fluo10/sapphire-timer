use std::io::{IsTerminal as _, Write as _};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use clap::Args;
use sapphire_timer_core::{
    ops::{self, Stop},
    preset,
};

use super::{hms, open_workspace};

#[derive(Args)]
pub struct StartArgs {
    /// Preset name, as shown by `sapphire-timer preset list`.
    preset: String,

    /// Note to record with the session. This is what `sapphire-timer search`
    /// searches. Omit it on a terminal and you'll be asked once at the end.
    #[arg(long, short)]
    comment: Option<String>,
}

pub fn run(
    dir: Option<&Path>,
    args: StartArgs,
    remote: Option<&str>,
    token: Option<&str>,
) -> Result<()> {
    let ws = open_workspace(dir, remote, token)?;
    let (presets, rewritten) = ops::list_presets(ws.timer())?;
    // Presets that were just assigned an id need to reach the index and git
    // (or the remote server).
    for path in &rewritten {
        ws.index_and_stage(path)?;
    }

    let preset = preset::find_by_name(&presets, &args.preset)?;

    // Ctrl-C must not kill the process: an abandoned countdown is still data.
    // Flip a flag instead and let run_timer finish and write the session.
    let interrupted = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&interrupted);
    // A second Ctrl-C after the handler is installed still exits, because the
    // handler only runs once per press and the loop ends on the first.
    ctrlc::set_handler(move || flag.store(true, Ordering::SeqCst))?;

    eprintln!(
        "{} — {} min. Ctrl-C to stop early.",
        preset.name, preset.duration_minutes
    );

    let comment = args.comment.clone();
    let (session, log_path) = ops::run_timer(
        ws.timer(),
        preset,
        |stop| match comment {
            Some(c) => c,
            None => prompt_comment(stop),
        },
        |remaining| {
            // \r-overwrite on stderr, so stdout stays clean for piping.
            eprint!("\r  {}  remaining ", hms(remaining.as_secs()));
            let _ = std::io::stderr().flush();
        },
        || interrupted.load(Ordering::SeqCst),
    )?;
    eprintln!();

    // Index and stage (or push) the appended log line.
    ws.index_and_stage(&log_path)?;

    match session.outcome {
        sapphire_timer_core::Outcome::Completed => {
            println!(
                "completed: {} ({})",
                session.preset_name,
                hms(session.elapsed_secs)
            )
        }
        sapphire_timer_core::Outcome::Interrupted => {
            println!(
                "stopped:   {} ({})",
                session.preset_name,
                hms(session.elapsed_secs)
            )
        }
    }
    Ok(())
}

/// Ask for a note once, interactively. Returns empty when not on a terminal.
fn prompt_comment(stop: Stop) -> String {
    if !std::io::stdin().is_terminal() {
        return String::new();
    }
    let label = match stop {
        Stop::Elapsed => "comment (optional): ",
        Stop::Interrupted => "\ncomment (optional): ",
    };
    eprint!("{label}");
    let _ = std::io::stderr().flush();

    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(_) => line.trim().to_owned(),
        Err(_) => String::new(),
    }
}
