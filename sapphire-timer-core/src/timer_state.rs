//! A timer workspace paired with the framework's index and sync backend.
//!
//! This is deliberately a thin wrapper over
//! [`WorkspaceState`](sapphire_workspace::WorkspaceState). sapphire-timer keeps
//! no database of its own: presets and logs are read from disk, and search is
//! the framework's. Whether that stays sufficient is the question this app
//! exists to answer.

use std::path::Path;

use sapphire_workspace::{
    DbInfo, FileSearchResult, RetrieveParams, SearchMode, Workspace, WorkspaceState,
};

use crate::{error::Result, timer::Timer, user_config::UserConfig};

pub struct TimerState {
    pub timer: Timer,
    inner: WorkspaceState,
}

impl TimerState {
    /// Open the workspace's index and sync backend.
    ///
    /// Uses the framework's `open_configured` rather than `open`, because
    /// `open` ignores [`SyncConfig`](sapphire_workspace::SyncConfig) entirely
    /// and would silently attach git even when the user asked for no sync.
    /// Note that this reaches the device registry, so
    /// [`init_app_context`](crate::init_app_context) — which injects the device
    /// defaults — must have run first, or `AppContext::device()` panics.
    pub fn open(timer: Timer, config: &UserConfig) -> Result<Self> {
        let workspace = Workspace::from_root(&crate::TIMER_CTX, &timer.root)?;
        let inner = WorkspaceState::open_configured(workspace, &config.sync)?;
        Ok(Self { timer, inner })
    }

    /// Drop and rebuild the index from the files on disk.
    pub fn rebuild(timer: Timer) -> Result<Self> {
        let workspace = Workspace::from_root(&crate::TIMER_CTX, &timer.root)?;
        let inner = WorkspaceState::rebuild(workspace)?;
        Ok(Self { timer, inner })
    }

    /// Borrow the underlying framework state, for anything not wrapped here.
    pub fn workspace_state(&self) -> &WorkspaceState {
        &self.inner
    }

    /// Load the vector backend and embedder, when configured.
    ///
    /// Both are no-ops unless embedding is enabled and a dimension is set.
    pub fn load_retrieve_backend(&self, config: &UserConfig) -> Result<()> {
        self.inner.load_retrieve_backend(&config.cache.retrieve)?;
        self.inner.load_embedder(&config.cache.retrieve)?;
        Ok(())
    }

    /// Re-index changed files. Returns `(upserted, removed)`.
    pub fn sync(&self) -> Result<(usize, usize)> {
        Ok(self.inner.sync_retrieve()?)
    }

    /// Re-index every file regardless of mtime. Returns `(upserted, removed)`.
    pub fn sync_full(&self) -> Result<(usize, usize)> {
        Ok(self.inner.sync()?)
    }

    /// Commit, pull and push via the sync backend, then re-index.
    pub fn sync_git(&self) -> Result<(usize, usize)> {
        Ok(self.inner.periodic_sync()?)
    }

    /// Index a file that just changed and stage it for sync.
    pub fn on_file_updated(&self, path: &Path) -> Result<()> {
        self.inner.on_file_updated(path)?;
        Ok(())
    }

    /// Embed any chunks still lacking a vector. Returns how many were embedded.
    pub fn embed_pending(
        &self,
        config: &UserConfig,
        on_progress: impl Fn(usize, usize),
    ) -> Result<usize> {
        Ok(self
            .inner
            .embed_pending(&config.cache.retrieve, on_progress)?)
    }

    /// Search presets and session logs.
    pub fn search(
        &self,
        query: &str,
        limit: usize,
        mode: SearchMode,
    ) -> Result<Vec<FileSearchResult>> {
        let config = UserConfig::load()?;
        let params = RetrieveParams {
            query,
            limit,
            mode,
            folder: None,
        };
        Ok(self
            .inner
            .retrieve_files(&params, &config.cache.retrieve.hybrid)?)
    }

    /// Index location and document/vector counts.
    pub fn cache_info(&self) -> Result<DbInfo> {
        Ok(self.inner.db_info()?)
    }
}
