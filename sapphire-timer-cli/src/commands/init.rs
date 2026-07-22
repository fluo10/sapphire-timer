use std::path::Path;

use anyhow::{Result, bail};
use sapphire_timer_core::timer::init_workspace;

use super::show_path;

pub fn run(path: Option<&Path>, remote: Option<&str>) -> Result<()> {
    if remote.is_some() {
        bail!("`init` creates a local workspace and cannot target --remote");
    }
    let root = match path {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir()?,
    };

    let timer = init_workspace(&root)?;
    let (presets, _) = sapphire_timer_core::ops::list_presets(&timer)?;

    println!("initialized:     {}", show_path(&timer.root));
    println!("presets:         {}", show_path(&timer.presets_dir()?));
    println!("logs:            {}", show_path(&timer.logs_dir()?));
    println!("starter presets: {}", presets.len());
    println!();
    println!("run `sapphire-timer start pomodoro` to begin");
    Ok(())
}
