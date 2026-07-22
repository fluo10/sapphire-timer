//! Core library for **sapphire-timer** — a preset-based timer that keeps its
//! data alive as plain text.
//!
//! sapphire-timer is deliberately thin. It exists as much to exercise
//! [`sapphire-framework`] from a second, non-Markdown angle as it does to run
//! timers:
//!
//! - **presets are TOML**, **session logs are JSONL** — the framework's two
//!   non-Markdown chunkers, which no other app uses;
//! - it keeps **no database of its own**. Presets and logs are read from disk,
//!   and search is delegated to the framework's retrieve index. If that is
//!   enough for a real app, the framework's index is enough;
//! - consequently it links **no SQLite at all** — `cargo tree -i
//!   libsqlite3-sys` is empty.
//!
//! [`sapphire-framework`]: https://github.com/fluo10/sapphire-framework

pub mod error;
pub mod ops;
pub mod preset;
pub mod session;
pub mod timer;
pub mod timer_state;
pub mod user_config;

pub use error::{Error, Result};
pub use preset::Preset;
pub use session::{Outcome, Session};
pub use timer::Timer;
pub use timer_state::TimerState;

// Re-export the framework surface so callers need one dependency.
pub use sapphire_workspace::{
    EmbeddingConfig, FileSearchResult, RetrieveConfig, SearchMode, VectorDb,
};

/// Shared application context: app name plus the cache and data directories
/// every [`Timer`] resolves paths against.
///
/// The app name doubles as the workspace marker: a timer workspace is any
/// directory containing `.sapphire-timer/`.
pub static TIMER_CTX: sapphire_workspace::AppContext =
    sapphire_workspace::AppContext::new("sapphire-timer");

/// Initialise [`TIMER_CTX`] with platform-default directories.
///
/// Call this once at the top of `main`, before anything opens a timer
/// workspace. It is idempotent — `AppContext` is first-writer-wins, so a host
/// that injected explicit paths beforehand wins and this becomes a no-op.
pub fn init_app_context() {
    let cache = dirs::cache_dir()
        .unwrap_or_else(|| std::env::temp_dir().join(".cache"))
        .join("sapphire-timer");
    let data = dirs::data_dir()
        .unwrap_or_else(|| std::env::temp_dir().join(".local").join("share"))
        .join("sapphire-timer");
    let _ = std::fs::create_dir_all(&cache);
    let _ = std::fs::create_dir_all(&data);
    TIMER_CTX.set_cache_dir(cache);
    TIMER_CTX.set_data_dir(data);
}
