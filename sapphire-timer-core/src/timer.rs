//! Timer workspace discovery and layout.
//!
//! A timer workspace is a directory containing a `.sapphire-timer/` marker:
//!
//! ```text
//! <root>/
//! ├── .sapphire-timer/
//! │   ├── config.toml     # TimerConfig, tracked by git, shared across machines
//! │   └── .gitignore
//! ├── presets/            # one TOML per preset
//! └── logs/               # YYYY-MM.jsonl, append-only
//! ```
//!
//! Presets and logs sit *outside* the marker directory deliberately: the
//! framework's indexer skips dot-prefixed directories at any depth, so
//! anything under `.sapphire-timer/` is invisible to search.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Per-workspace configuration (`.sapphire-timer/config.toml`).
///
/// This is committed and shared across machines. Machine-specific settings
/// (cache backend, embedding provider, sync cadence) belong in
/// [`UserConfig`](crate::user_config::UserConfig) instead.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimerConfig {
    /// Directory holding preset TOML files, relative to the workspace root.
    #[serde(default = "default_presets_dir")]
    pub presets_dir: String,
    /// Directory holding session JSONL logs, relative to the workspace root.
    #[serde(default = "default_logs_dir")]
    pub logs_dir: String,
}

fn default_presets_dir() -> String {
    "presets".to_owned()
}

fn default_logs_dir() -> String {
    "logs".to_owned()
}

impl Default for TimerConfig {
    fn default() -> Self {
        Self {
            presets_dir: default_presets_dir(),
            logs_dir: default_logs_dir(),
        }
    }
}

/// An open timer workspace.
#[derive(Debug, Clone)]
pub struct Timer {
    /// Canonicalized workspace root.
    pub root: PathBuf,
}

impl Timer {
    /// Marker directory name — also the app name, and hence the framework's
    /// per-app workspace marker.
    pub const MARKER: &'static str = ".sapphire-timer";

    /// Open the workspace rooted at `root`.
    pub fn from_root(root: &Path) -> Result<Self> {
        let root = root.canonicalize()?;
        if !root.join(Self::MARKER).is_dir() {
            return Err(Error::TimerNotFound);
        }
        Ok(Self { root })
    }

    /// Search upwards from `start` for a directory containing the marker.
    pub fn find_from(start: &Path) -> Result<Self> {
        let start = start.canonicalize()?;
        for dir in start.ancestors() {
            if dir.join(Self::MARKER).is_dir() {
                return Ok(Self {
                    root: dir.to_path_buf(),
                });
            }
        }
        Err(Error::TimerNotFound)
    }

    /// Search upwards from the current directory.
    pub fn find() -> Result<Self> {
        Self::find_from(&std::env::current_dir()?)
    }

    /// Open `root` if given, else search upwards from the current directory.
    pub fn resolve(root: Option<&Path>) -> Result<Self> {
        match root {
            Some(path) => Self::from_root(path),
            None => Self::find(),
        }
    }

    pub fn marker_dir(&self) -> PathBuf {
        self.root.join(Self::MARKER)
    }

    pub fn config_path(&self) -> PathBuf {
        self.marker_dir().join("config.toml")
    }

    /// Load the per-workspace config, falling back to defaults when absent.
    pub fn config(&self) -> Result<TimerConfig> {
        let path = self.config_path();
        if !path.exists() {
            return Ok(TimerConfig::default());
        }
        let text = std::fs::read_to_string(&path)?;
        toml::from_str(&text).map_err(|e| Error::InvalidConfig(format!("{}: {e}", path.display())))
    }

    pub fn presets_dir(&self) -> Result<PathBuf> {
        Ok(self.root.join(self.config()?.presets_dir))
    }

    pub fn logs_dir(&self) -> Result<PathBuf> {
        Ok(self.root.join(self.config()?.logs_dir))
    }
}

/// Create a timer workspace at `root`, with two starter presets.
///
/// The framework has no marker-creation helper — every app implements its own
/// (see `sapphire-framework-workspace-cli`'s `init` for the reference).
///
/// Returns the opened workspace. Idempotent enough to re-run: existing files
/// are left alone.
pub fn init_workspace(root: &Path) -> Result<Timer> {
    use crate::preset::{Preset, mint_id, preset_path};
    use std::collections::HashSet;

    std::fs::create_dir_all(root)?;
    let marker = root.join(Timer::MARKER);
    std::fs::create_dir_all(&marker)?;

    let config_path = marker.join("config.toml");
    if !config_path.exists() {
        std::fs::write(
            &config_path,
            toml::to_string_pretty(&TimerConfig::default())?,
        )?;
    }

    // The retrieve cache lives under the platform cache dir, not in here, but
    // keep the convention the other apps use.
    let gitignore = marker.join(".gitignore");
    if !gitignore.exists() {
        std::fs::write(&gitignore, "cache/\n")?;
    }

    let config = TimerConfig::default();
    let presets_dir = root.join(&config.presets_dir);
    std::fs::create_dir_all(&presets_dir)?;
    std::fs::create_dir_all(root.join(&config.logs_dir))?;

    // `GrainId::now_unix()` has decisecond precision, so minting these two
    // back-to-back would hand them the same id — and logs resolve presets by
    // id. Mint through a `taken` set so they stay distinct.
    let mut taken = HashSet::new();
    let starters = [
        (
            "pomodoro",
            25,
            "Focus block. One task, no context switching.",
        ),
        ("break", 5, "Short break between focus blocks."),
    ];
    for (name, duration_minutes, description) in starters {
        let path = preset_path(&presets_dir, name);
        if path.exists() {
            continue;
        }
        let id = mint_id(&taken);
        taken.insert(id);
        let preset = Preset {
            id,
            name: name.to_owned(),
            duration_minutes,
            description: description.to_owned(),
        };
        std::fs::write(&path, preset.to_toml()?)?;
    }

    Timer::from_root(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_creates_a_discoverable_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let timer = init_workspace(tmp.path()).unwrap();

        assert!(timer.marker_dir().is_dir());
        assert!(timer.presets_dir().unwrap().is_dir());
        assert!(timer.logs_dir().unwrap().is_dir());

        let (presets, _) = crate::preset::load_presets(&timer.presets_dir().unwrap()).unwrap();
        assert_eq!(presets.len(), 2, "starter presets should be written");
        assert_ne!(
            presets[0].id, presets[1].id,
            "starter presets are minted in the same tick and must not share an id"
        );
    }

    #[test]
    fn init_is_rerunnable_and_keeps_preset_ids() {
        let tmp = tempfile::tempdir().unwrap();
        let timer = init_workspace(tmp.path()).unwrap();
        let dir = timer.presets_dir().unwrap();
        let (before, _) = crate::preset::load_presets(&dir).unwrap();

        init_workspace(tmp.path()).unwrap();
        let (after, _) = crate::preset::load_presets(&dir).unwrap();

        assert_eq!(
            before.iter().map(|p| p.id).collect::<Vec<_>>(),
            after.iter().map(|p| p.id).collect::<Vec<_>>()
        );
    }

    #[test]
    fn find_from_walks_up_to_the_marker() {
        let tmp = tempfile::tempdir().unwrap();
        init_workspace(tmp.path()).unwrap();
        let nested = tmp.path().join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();

        let found = Timer::find_from(&nested).unwrap();
        assert_eq!(found.root, tmp.path().canonicalize().unwrap());
    }

    #[test]
    fn from_root_rejects_a_directory_without_a_marker() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(matches!(
            Timer::from_root(tmp.path()),
            Err(Error::TimerNotFound)
        ));
    }
}
