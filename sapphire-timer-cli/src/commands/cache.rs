use std::io::Write as _;
use std::path::Path;

use anyhow::{Result, bail};
use clap::Subcommand;
use sapphire_timer_core::{TimerState, VectorDb, user_config::UserConfig};

use super::{human_size, open_state, resolve_timer, show_path};

#[derive(Subcommand)]
pub enum CacheCommand {
    /// Show index location and document/vector counts.
    Info,
    /// Re-index files that changed since the last sync.
    Sync,
    /// Drop the index and rebuild it from the files on disk.
    Rebuild,
    /// Embed any chunks still lacking a vector.
    Embed,
    /// Remove orphaned cache directories.
    Clean,
}

pub fn run(dir: Option<&Path>, action: CacheCommand, remote: Option<&str>) -> Result<()> {
    if remote.is_some() {
        // The index is a local concern. For a remote workspace the server owns
        // its index; the mirror's index is maintained automatically on sync.
        bail!("`cache` maintains the local index and cannot target --remote");
    }
    match action {
        CacheCommand::Info => info(dir),
        CacheCommand::Sync => sync(dir),
        CacheCommand::Rebuild => rebuild(dir),
        CacheCommand::Embed => embed(dir),
        CacheCommand::Clean => clean(dir),
    }
}

fn info(dir: Option<&Path>) -> Result<()> {
    let (state, config) = open_state(dir)?;
    let db = state.cache_info()?;

    println!("path:           {}", show_path(&db.db_path));
    println!("documents:      {}", db.document_count);

    let retrieve = &config.cache.retrieve;
    match &retrieve.embedding {
        Some(e) if e.enabled => {
            println!(
                "embedding:      enabled (provider={}, model={})",
                e.provider, e.model
            );
            match retrieve.db {
                VectorDb::None => println!("vector backend: none"),
                backend => {
                    if e.dimension.is_some() {
                        state.load_retrieve_backend(&config)?;
                        let db = state.cache_info()?;
                        println!(
                            "vector backend: {} (dim={})",
                            backend.as_str(),
                            db.embedding_dim
                        );
                        println!(
                            "vectors:        {} indexed, {} pending",
                            db.vector_count, db.pending_count
                        );
                    } else {
                        println!(
                            "vector backend: {} (dimension not configured)",
                            backend.as_str()
                        );
                    }
                }
            }
        }
        _ => println!("embedding:      disabled"),
    }
    Ok(())
}

fn sync(dir: Option<&Path>) -> Result<()> {
    let (state, _) = open_state(dir)?;
    let (upserted, removed) = state.sync()?;
    println!("synced: {upserted} upserted, {removed} removed");
    Ok(())
}

fn rebuild(dir: Option<&Path>) -> Result<()> {
    let timer = resolve_timer(dir)?;
    let state = TimerState::rebuild(timer)?;
    let (upserted, _) = state.sync_full()?;
    println!("rebuilt: {upserted} documents indexed");
    Ok(())
}

fn embed(dir: Option<&Path>) -> Result<()> {
    let (state, config) = open_state(dir)?;

    let retrieve = &config.cache.retrieve;
    let Some(embedding) = retrieve.embedding.as_ref().filter(|e| e.enabled) else {
        anyhow::bail!(
            "embedding is disabled — enable it in {}",
            UserConfig::path().display()
        );
    };
    if retrieve.db == VectorDb::None {
        anyhow::bail!("retrieve.db is \"none\" — set it to \"redb\" or \"lancedb\"");
    }
    if embedding.dimension.is_none() {
        anyhow::bail!("embedding.dimension is required for vector search");
    }

    state.load_retrieve_backend(&config)?;
    let embedded = state.embed_pending(&config, |done, total| {
        eprint!("\r  embedding {done}/{total} ");
        let _ = std::io::stderr().flush();
    })?;
    eprintln!();
    println!("embedded: {embedded} chunks");
    Ok(())
}

fn clean(dir: Option<&Path>) -> Result<()> {
    let (state, _) = open_state(dir)?;
    let db = state.cache_info()?;

    // The redb store lives in a sibling directory of the reported db path; the
    // whole per-workspace cache dir is what's worth reporting here.
    let Some(cache_dir) = db.db_path.parent() else {
        println!("nothing to clean");
        return Ok(());
    };

    let mut total = 0u64;
    let mut found = false;
    if let Ok(entries) = std::fs::read_dir(cache_dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            total += dir_size(&entry.path());
            found = true;
        }
    }

    if !found {
        println!("nothing to clean");
        return Ok(());
    }

    println!("cache dir:      {}", show_path(cache_dir));
    println!("size:           {}", human_size(total));
    println!();
    println!("the cache is rebuildable — remove the directory above and run");
    println!("`sapphire-timer cache rebuild` to recreate it");
    Ok(())
}

fn dir_size(path: &Path) -> u64 {
    let Ok(meta) = std::fs::metadata(path) else {
        return 0;
    };
    if meta.is_file() {
        return meta.len();
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .filter_map(|e| e.ok())
        .map(|e| dir_size(&e.path()))
        .sum()
}
