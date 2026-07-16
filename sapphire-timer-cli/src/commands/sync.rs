use std::path::Path;

use anyhow::Result;

use super::open_state;

pub fn run(dir: Option<&Path>) -> Result<()> {
    let (state, _) = open_state(dir)?;
    let (upserted, removed) = state.sync_git()?;
    println!("synced: {upserted} upserted, {removed} removed");
    Ok(())
}
