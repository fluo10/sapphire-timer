use std::path::Path;

use anyhow::Result;
use clap::{Args, ValueEnum};
use sapphire_timer_core::SearchMode;

use super::open_workspace;

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
pub enum Mode {
    /// Full-text only.
    Fts,
    /// Vector similarity only. Falls back to full-text with no embedder.
    Semantic,
    /// Both, merged by reciprocal rank fusion.
    Hybrid,
}

impl From<Mode> for SearchMode {
    fn from(m: Mode) -> Self {
        match m {
            Mode::Fts => SearchMode::Fts,
            Mode::Semantic => SearchMode::Semantic,
            Mode::Hybrid => SearchMode::Hybrid,
        }
    }
}

#[derive(Args)]
pub struct SearchArgs {
    /// What to look for. Substring and CJK queries work (trigram index);
    /// queries shorter than 3 characters match nothing.
    query: String,

    #[arg(long, short, default_value_t = 10)]
    limit: usize,

    #[arg(long, value_enum, default_value_t = Mode::Fts)]
    mode: Mode,
}

pub fn run(
    dir: Option<&Path>,
    args: SearchArgs,
    remote: Option<&str>,
    token: Option<&str>,
) -> Result<()> {
    let ws = open_workspace(dir, remote, token)?;
    ws.ensure_search_ready()?;

    let results = ws.search(&args.query, args.limit, args.mode.into())?;
    if results.is_empty() {
        println!("no matches");
        return Ok(());
    }

    let root = &ws.timer().root;
    for file in &results {
        // Show paths relative to the workspace: absolute ones are noise here.
        let path = Path::new(&file.path);
        let shown = path.strip_prefix(root).unwrap_or(path);
        println!("{}", shown.display());
        for chunk in &file.chunks {
            for line in chunk.text.lines() {
                println!("    {line}");
            }
        }
    }
    Ok(())
}
