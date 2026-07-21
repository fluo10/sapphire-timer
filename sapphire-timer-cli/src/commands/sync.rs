use std::path::Path;

use anyhow::Result;

use super::open_workspace;

pub fn run(dir: Option<&Path>, remote: Option<&str>, token: Option<&str>) -> Result<()> {
    let ws = open_workspace(dir, remote, token)?;
    let (upserted, removed) = ws.sync()?;
    println!("synced: {upserted} upserted, {removed} removed");
    Ok(())
}
