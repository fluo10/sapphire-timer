//! Opening a **remote** timer workspace.
//!
//! A remote workspace is mirrored into a local cache directory (under the app
//! cache) and driven through the framework's
//! [`RemoteBackend`](sapphire_backend::RemoteBackend). Because the mirror is an
//! ordinary on-disk timer workspace, the file-based commands (`preset`, `log`,
//! `search`) read it exactly as they read a local one — the only remote-aware
//! parts are pulling on open and pushing on write.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Result, bail};
use sapphire_backend::{
    RemoteBackend, RemoteClient, WorkspaceBackend as _, WorkspaceLocator, WorkspaceState,
};
use sapphire_timer_core::{FileSearchResult, SearchMode, TIMER_CTX, Timer};
use sapphire_workspace::Workspace;

/// A remote timer workspace: a local cache mirror plus the sync backend that
/// keeps it in step with the server.
pub struct RemoteWorkspace {
    /// The cache-rooted timer, used for the file-based reads the commands do.
    pub timer: Timer,
    backend: RemoteBackend,
    /// Current-thread runtime driving the async backend from this sync CLI.
    rt: tokio::runtime::Runtime,
}

impl RemoteWorkspace {
    /// Open the remote workspace referenced by `remote` (an `http(s)://host#ws`
    /// URL), pulling its current state into the local cache mirror.
    ///
    /// Pulling is best-effort: when the server is unreachable, the command
    /// proceeds against whatever the cache already holds (offline reads).
    pub fn open(remote: &str, token: Option<&str>) -> Result<Self> {
        let (url, ws) = match WorkspaceLocator::parse(remote) {
            WorkspaceLocator::Remote { url, ws } => (url, ws),
            WorkspaceLocator::Local(_) => {
                bail!("--remote expects an http(s):// URL, got '{remote}'");
            }
        };

        let ctx = &TIMER_CTX;
        let cache_root = ctx
            .cache_dir()
            .join("remotes")
            .join(mirror_dir_name(&url, &ws));
        std::fs::create_dir_all(&cache_root)?;
        // The workspace marker (`.sapphire-timer`) is local metadata, never
        // synced; create it so `Workspace::from_root` accepts the mirror.
        std::fs::create_dir_all(cache_root.join(format!(".{}", ctx.app_name)))?;

        let workspace = Workspace::from_root(ctx, &cache_root)?;
        // The mirror syncs over JSON-RPC, not git; the framework no longer
        // attaches any git backend, so a plain open is correct.
        let cache_state = Arc::new(WorkspaceState::open(workspace)?);

        let mut client = RemoteClient::new(url);
        if let Some(t) = token {
            client = client.with_token(t);
        }
        let backend = RemoteBackend::new(client, ws, cache_state);

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        if let Err(e) = rt.block_on(backend.sync()) {
            eprintln!("warning: could not reach remote ({e}); using cached data");
        }

        let timer = Timer::resolve(Some(&cache_root))?;
        Ok(Self { timer, backend, rt })
    }

    /// Search the local cache mirror (server-delegated search is a WASM-only
    /// concern; native uses the local FTS index).
    pub fn search(
        &self,
        query: &str,
        limit: usize,
        mode: SearchMode,
    ) -> Result<Vec<FileSearchResult>> {
        Ok(self.rt.block_on(self.backend.search(query, limit, mode))?)
    }

    /// Push a just-written file to the server (and (re)index it in the mirror).
    /// `path` is absolute, under the mirror root.
    pub fn index_and_stage(&self, path: &Path) -> Result<()> {
        let rel = path.strip_prefix(&self.timer.root).unwrap_or(path);
        let content = std::fs::read_to_string(path)?;
        self.rt
            .block_on(self.backend.write_file(rel, &content))?;
        Ok(())
    }

    /// Pull newer changes from the server into the mirror. Returns
    /// `(upserted, removed)` to match the local sync command's output.
    pub fn sync(&self) -> Result<(usize, usize)> {
        let summary = self.rt.block_on(self.backend.sync())?;
        Ok((summary.upserted, summary.removed))
    }
}

/// A filesystem-safe directory name for the `(url, ws)` mirror.
fn mirror_dir_name(url: &str, ws: &str) -> String {
    format!("{}_{}", sanitize(url), sanitize(ws))
}

fn sanitize(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
        .collect();
    if cleaned.is_empty() {
        "default".to_owned()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    use sapphire_backend::RemoteClient;
    use sapphire_rpc::Change;

    /// Start a real framework server on an ephemeral port in a background
    /// thread; return its base URL. The daemon thread ends when the test
    /// process exits.
    fn spawn_server(data_dir: PathBuf) -> String {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                let state = Arc::new(sapphire_remote_server::ServerState::new(data_dir));
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let addr = listener.local_addr().unwrap();
                tx.send(addr).unwrap();
                axum::serve(listener, sapphire_remote_server::router(state))
                    .await
                    .unwrap();
            });
        });
        format!("http://{}", rx.recv().unwrap())
    }

    #[test]
    fn remote_workspace_reads_searches_and_pushes() {
        // Isolate the app cache to a temp dir before anything reads it.
        let cache = tempfile::tempdir().unwrap();
        TIMER_CTX.set_cache_dir(cache.path().to_path_buf());
        sapphire_timer_core::init_app_context(); // device defaults (cache no-op)

        let server_data = tempfile::tempdir().unwrap();
        let url = spawn_server(server_data.path().to_path_buf());

        // Seed the server's "default" workspace with a preset.
        let seed_rt = tokio::runtime::Runtime::new().unwrap();
        let client = RemoteClient::new(url.clone());
        seed_rt
            .block_on(client.push(
                "default",
                0,
                vec![Change::upsert(
                    "presets/focus.toml",
                    "duration_minutes = 25\ndescription = \"deep work fossil\"\n",
                    chrono::Utc::now(),
                )],
            ))
            .unwrap();
        drop(seed_rt); // no ambient runtime while the CLI code uses its own

        // Open the remote workspace — this pulls the seed into the mirror.
        let rw = RemoteWorkspace::open(&url, None).unwrap();

        // The preset is readable through the ordinary file-based path.
        let (presets, _) = sapphire_timer_core::ops::list_presets(&rw.timer).unwrap();
        assert!(
            presets.iter().any(|p| p.name == "focus"),
            "expected 'focus' preset, got {:?}",
            presets.iter().map(|p| &p.name).collect::<Vec<_>>()
        );

        // And searchable via the mirror's local FTS index.
        let hits = rw.search("fossil", 10, SearchMode::Fts).unwrap();
        assert!(!hits.is_empty(), "search for 'fossil' returned nothing");

        // A local write is pushed to the server.
        let logs = rw.timer.logs_dir().unwrap();
        std::fs::create_dir_all(&logs).unwrap();
        let log_file = logs.join("2026-07.jsonl");
        std::fs::write(&log_file, "{\"preset_name\":\"focus\"}\n").unwrap();
        rw.index_and_stage(&log_file).unwrap();

        // A fresh client pulls the pushed log back.
        let verify_rt = tokio::runtime::Runtime::new().unwrap();
        let pulled = verify_rt
            .block_on(RemoteClient::new(url).pull("default", 0, 100))
            .unwrap();
        assert!(
            pulled.changes.iter().any(|c| c.path == "logs/2026-07.jsonl"),
            "server did not receive the pushed log; has {:?}",
            pulled.changes.iter().map(|c| &c.path).collect::<Vec<_>>()
        );
    }
}
