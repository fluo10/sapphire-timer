//! Machine-local configuration (`$XDG_CONFIG_HOME/sapphire-timer/config.toml`).
//!
//! Deliberately separate from the per-workspace
//! [`TimerConfig`](crate::timer::TimerConfig): the same timer workspace can be
//! shared across machines with different hardware, so which cache backend and
//! embedding model to use is a property of the machine, not of the workspace.

use std::path::PathBuf;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

pub use sapphire_workspace::{
    EmbeddingConfig, RetrieveConfig, SyncBackendKind, SyncConfig, VectorDb,
};

const DEFAULT_SYNC_INTERVAL_MINUTES: u32 = 10;

fn default_sync_interval_minutes() -> Option<u32> {
    Some(DEFAULT_SYNC_INTERVAL_MINUTES)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CacheConfig {
    #[serde(default)]
    pub retrieve: RetrieveConfig,
    /// Unknown keys are preserved so a newer version's config round-trips
    /// through an older binary without losing settings.
    #[serde(flatten)]
    pub extra: IndexMap<String, toml::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserConfig {
    #[serde(default)]
    pub cache: CacheConfig,
    #[serde(default)]
    pub sync: SyncConfig,
    #[serde(
        default = "default_sync_interval_minutes",
        skip_serializing_if = "Option::is_none"
    )]
    pub sync_interval_minutes: Option<u32>,
}

impl Default for UserConfig {
    fn default() -> Self {
        Self {
            cache: CacheConfig::default(),
            sync: SyncConfig::default(),
            sync_interval_minutes: default_sync_interval_minutes(),
        }
    }
}

impl UserConfig {
    /// `$XDG_CONFIG_HOME/sapphire-timer/config.toml`.
    pub fn path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("sapphire-timer")
            .join("config.toml")
    }

    /// Load from disk, then apply environment overrides.
    ///
    /// A missing file is not an error — it yields defaults.
    ///
    /// | Variable | Field |
    /// |---|---|
    /// | `SAPPHIRE_TIMER_CACHE_RETRIEVE_DB` | `cache.retrieve.db` (`none`/`redb`/`lancedb`) |
    /// | `SAPPHIRE_TIMER_CACHE_EMBEDDING_ENABLED` | `cache.retrieve.embedding.enabled` |
    /// | `SAPPHIRE_TIMER_CACHE_EMBEDDING_PROVIDER` | `…embedding.provider` |
    /// | `SAPPHIRE_TIMER_CACHE_EMBEDDING_MODEL` | `…embedding.model` |
    /// | `SAPPHIRE_TIMER_CACHE_EMBEDDING_DIMENSION` | `…embedding.dimension` |
    /// | `SAPPHIRE_TIMER_SYNC_BACKEND` | `sync.backend` (`auto`/`none`/`git`) |
    /// | `SAPPHIRE_TIMER_SYNC_INTERVAL_MINUTES` | `sync_interval_minutes` |
    pub fn load() -> Result<Self> {
        let path = Self::path();
        let mut config = if !path.exists() {
            UserConfig::default()
        } else {
            let text = std::fs::read_to_string(&path)?;
            toml::from_str(&text)
                .map_err(|e| Error::InvalidConfig(format!("{}: {e}", path.display())))?
        };
        config.apply_env_overrides();
        Ok(config)
    }

    /// Sync cadence, or `None` when periodic sync is off.
    pub fn sync_interval(&self) -> Option<std::time::Duration> {
        self.sync_interval_minutes
            .filter(|&m| m > 0)
            .map(|m| std::time::Duration::from_secs(u64::from(m) * 60))
    }

    fn apply_env_overrides(&mut self) {
        if let Some(db) = std::env::var("SAPPHIRE_TIMER_CACHE_RETRIEVE_DB")
            .ok()
            .and_then(|v| match v.as_str() {
                "none" => Some(VectorDb::None),
                "redb" => Some(VectorDb::Redb),
                "lancedb" => Some(VectorDb::LanceDb),
                _ => None,
            })
        {
            self.cache.retrieve.db = db;
        }

        let enabled = std::env::var("SAPPHIRE_TIMER_CACHE_EMBEDDING_ENABLED")
            .ok()
            .map(|v| matches!(v.as_str(), "1" | "true" | "yes"));
        let provider = std::env::var("SAPPHIRE_TIMER_CACHE_EMBEDDING_PROVIDER").ok();
        let model = std::env::var("SAPPHIRE_TIMER_CACHE_EMBEDDING_MODEL").ok();
        let dimension = std::env::var("SAPPHIRE_TIMER_CACHE_EMBEDDING_DIMENSION")
            .ok()
            .and_then(|v| v.parse::<u32>().ok());

        // Only materialise an [embedding] section if something actually set it.
        if enabled.is_some() || provider.is_some() || model.is_some() || dimension.is_some() {
            let embedding = self
                .cache
                .retrieve
                .embedding
                .get_or_insert_with(EmbeddingConfig::default);
            if let Some(v) = enabled {
                embedding.enabled = v;
            }
            if let Some(v) = provider {
                embedding.provider = v;
            }
            if let Some(v) = model {
                embedding.model = v;
            }
            if let Some(v) = dimension {
                embedding.dimension = Some(v);
            }
        }

        if let Some(backend) = std::env::var("SAPPHIRE_TIMER_SYNC_BACKEND")
            .ok()
            .and_then(|v| match v.as_str() {
                "auto" => Some(SyncBackendKind::Auto),
                "none" => Some(SyncBackendKind::None),
                "git" => Some(SyncBackendKind::Git),
                _ => None,
            })
        {
            self.sync.backend = backend;
        }

        if let Some(minutes) = std::env::var("SAPPHIRE_TIMER_SYNC_INTERVAL_MINUTES")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
        {
            self.sync_interval_minutes = Some(minutes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip_through_toml() {
        let text = toml::to_string_pretty(&UserConfig::default()).unwrap();
        let parsed: UserConfig = toml::from_str(&text).unwrap();
        assert_eq!(parsed.sync_interval_minutes, Some(10));
    }

    #[test]
    fn an_empty_config_parses() {
        let parsed: UserConfig = toml::from_str("").unwrap();
        assert_eq!(parsed.cache.retrieve.db, VectorDb::None);
    }

    #[test]
    fn zero_interval_disables_periodic_sync() {
        let config = UserConfig {
            sync_interval_minutes: Some(0),
            ..UserConfig::default()
        };
        assert!(config.sync_interval().is_none());
    }
}
