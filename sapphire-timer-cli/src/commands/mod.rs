pub mod cache;
pub mod init;
pub mod log;
pub mod preset;
pub mod remote;
pub mod search;
pub mod start;
pub mod sync;

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use sapphire_backend::{WorkspaceLocator, WorkspaceSelection};
use sapphire_timer_core::{
    FileSearchResult, SearchMode, TIMER_CTX, Timer, TimerState, timer::init_workspace,
    user_config::UserConfig,
};

const NOT_A_TIMER: &str =
    "not a sapphire-timer workspace — run `sapphire-timer init` to create one";

/// A workspace selection from the CLI's global flags.
#[derive(Clone, Copy, Default)]
pub struct Locator<'a> {
    /// `--timer-dir <path>`: an ad-hoc local root.
    pub dir: Option<&'a Path>,
    /// `--remote <url>`: an ad-hoc remote endpoint.
    pub remote: Option<&'a str>,
    /// `--workspace <id>`: a named entry from the `[workspace.*]` registry.
    pub workspace: Option<&'a str>,
    /// `--token`: bearer for a remote workspace.
    pub token: Option<&'a str>,
}

impl<'a> Locator<'a> {
    fn selection(&self) -> WorkspaceSelection<'a> {
        WorkspaceSelection {
            id: self.workspace,
            ad_hoc_path: self.dir,
            ad_hoc_url: self.remote,
            token: self.token,
        }
    }
}

/// Resolve the workspace from `--timer-dir`, else search upwards.
pub fn resolve_timer(dir: Option<&Path>) -> Result<Timer> {
    Timer::resolve(dir).context(NOT_A_TIMER)
}

/// Resolve a **local** workspace root from the selection (errors on a remote
/// selection). Used by local-only commands (`cache`).
pub fn resolve_local_path(loc: &Locator, config: &UserConfig) -> Result<PathBuf> {
    match config
        .workspace
        .resolve(&loc.selection(), TIMER_CTX.data_dir())?
    {
        WorkspaceLocator::Local(p) => Ok(p),
        WorkspaceLocator::Remote { .. } => {
            bail!("this command is local-only and cannot target a remote workspace")
        }
    }
}

/// Open a local workspace at `dir` (requiring it to already exist) plus its config.
pub fn open_state(dir: Option<&Path>) -> Result<(TimerState, UserConfig)> {
    let timer = resolve_timer(dir)?;
    let config = UserConfig::load()?;
    let state = TimerState::open(timer)?;
    Ok((state, config))
}

/// A timer workspace, local or remote, behind one interface.
///
/// A remote workspace is a local cache mirror kept in sync over JSON-RPC (see
/// [`remote::RemoteWorkspace`]); a local one is the workspace on disk. Commands
/// that read files use [`timer`](Self::timer); those that search, sync, or
/// stage writes go through the methods here so they work either way.
pub enum TimerWorkspace {
    /// A workspace rooted on the local filesystem.
    Local {
        /// The opened index/sync state.
        state: TimerState,
        /// Loaded user config.
        config: UserConfig,
    },
    /// A remote workspace mirrored into a local cache.
    Remote(remote::RemoteWorkspace),
}

impl TimerWorkspace {
    /// The (possibly cache-rooted) timer for file-based reads.
    pub fn timer(&self) -> &Timer {
        match self {
            Self::Local { state, .. } => &state.timer,
            Self::Remote(r) => &r.timer,
        }
    }

    /// Load the vector backend/embedder for search. No-op for remote (the
    /// mirror's index is already open; embeddings are a local-only concern in
    /// this MVP).
    pub fn ensure_search_ready(&self) -> Result<()> {
        if let Self::Local { state, config } = self {
            state.load_retrieve_backend(config)?;
        }
        Ok(())
    }

    /// Full-text / semantic search over presets and session logs.
    pub fn search(
        &self,
        query: &str,
        limit: usize,
        mode: SearchMode,
    ) -> Result<Vec<FileSearchResult>> {
        match self {
            Self::Local { state, .. } => Ok(state.search(query, limit, mode)?),
            Self::Remote(r) => r.search(query, limit, mode),
        }
    }

    /// Index a just-written file and stage/push it (git-stage locally, or push
    /// to the server for a remote workspace). `path` is absolute.
    pub fn index_and_stage(&self, path: &Path) -> Result<()> {
        match self {
            Self::Local { state, .. } => {
                state.on_file_updated(path)?;
                Ok(())
            }
            Self::Remote(r) => r.index_and_stage(path),
        }
    }

    /// Sync: re-index changed files (local), or pull from the server into the
    /// mirror (remote). Returns `(upserted, removed)`.
    ///
    /// Local workspaces no longer auto-sync over git — run `git` yourself to
    /// share a local workspace.
    pub fn sync(&self) -> Result<(usize, usize)> {
        match self {
            Self::Local { state, .. } => Ok(state.sync()?),
            Self::Remote(r) => r.sync(),
        }
    }
}

/// Open the workspace selected by `loc`, local or remote.
///
/// The selection precedence (`--remote` > `--timer-dir` > `--workspace <id>` >
/// the `default` entry > the built-in default path) lives in the framework's
/// [`WorkspaceRegistry::resolve`]. A local workspace that doesn't exist yet is
/// created on first open (marker + starter presets).
pub fn open_workspace(loc: &Locator) -> Result<TimerWorkspace> {
    let config = UserConfig::load()?;
    let locator = config
        .workspace
        .resolve(&loc.selection(), TIMER_CTX.data_dir())?;
    match locator {
        WorkspaceLocator::Local(path) => {
            let timer = match Timer::resolve(Some(&path)) {
                Ok(t) => t,
                Err(_) => init_workspace(&path)?,
            };
            let state = TimerState::open(timer)?;
            Ok(TimerWorkspace::Local { state, config })
        }
        WorkspaceLocator::Remote { url, ws, token } => Ok(TimerWorkspace::Remote(
            remote::RemoteWorkspace::open(&url, &ws, token.as_deref())?,
        )),
    }
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
