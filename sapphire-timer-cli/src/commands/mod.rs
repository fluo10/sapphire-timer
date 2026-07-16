pub mod cache;
pub mod init;
pub mod log;
pub mod preset;
pub mod search;
pub mod start;
pub mod sync;

use std::path::Path;

use anyhow::{Context as _, Result};
use sapphire_timer_core::{Timer, TimerState, user_config::UserConfig};

const NOT_A_TIMER: &str =
    "not a sapphire-timer workspace — run `sapphire-timer init` to create one";

/// Resolve the workspace from `--timer-dir`, else search upwards.
pub fn resolve_timer(dir: Option<&Path>) -> Result<Timer> {
    Timer::resolve(dir).context(NOT_A_TIMER)
}

/// Resolve the workspace and open its index.
pub fn open_state(dir: Option<&Path>) -> Result<(TimerState, UserConfig)> {
    let timer = resolve_timer(dir)?;
    let config = UserConfig::load()?;
    let state = TimerState::open(timer, &config)?;
    Ok((state, config))
}

/// Render a path for humans.
///
/// Workspace roots are canonicalized, which on Windows prefixes them with the
/// `\\?\` extended-length marker. That is an implementation detail of the
/// path, not something to show a user.
pub fn show_path(path: &Path) -> String {
    let s = path.display().to_string();
    match s.strip_prefix(r"\\?\") {
        Some(stripped) => stripped.to_owned(),
        None => s,
    }
}

/// Format a byte count the way the other sapphire CLIs do.
pub fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b >= KB * KB * KB {
        format!("{:.1} GB", b / (KB * KB * KB))
    } else if b >= KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

/// Format a duration as `MM:SS`, or `H:MM:SS` past an hour.
pub fn hms(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}
