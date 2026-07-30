use anyhow::Result;

use super::{Locator, open_workspace};

pub fn run(loc: &Locator) -> Result<()> {
    let ws = open_workspace(loc)?;
    let (upserted, removed) = ws.sync()?;
    println!("synced: {upserted} upserted, {removed} removed");
    Ok(())
}
