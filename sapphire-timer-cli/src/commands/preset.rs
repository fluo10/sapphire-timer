use std::path::Path;

use anyhow::Result;
use clap::Subcommand;
use sapphire_timer_core::{ops, preset};

use super::{open_workspace, show_path};

#[derive(Subcommand)]
pub enum PresetCommand {
    /// List every preset.
    List,
    /// Show one preset in full.
    Show {
        /// Preset name.
        name: String,
    },
}

pub fn run(
    dir: Option<&Path>,
    action: PresetCommand,
    remote: Option<&str>,
    token: Option<&str>,
) -> Result<()> {
    let ws = open_workspace(dir, remote, token)?;
    let timer = ws.timer();
    let (presets, _) = ops::list_presets(timer)?;

    match action {
        PresetCommand::List => {
            if presets.is_empty() {
                println!(
                    "no presets — add a TOML file under {}",
                    show_path(&timer.presets_dir()?)
                );
                return Ok(());
            }
            for p in &presets {
                println!(
                    "{:<16} {:>4} min  {}",
                    p.name, p.duration_minutes, p.description
                );
            }
        }
        PresetCommand::Show { name } => {
            let p = preset::find_by_name(&presets, &name)?;
            println!("id:             {}", p.id);
            println!("name:           {}", p.name);
            println!("duration:       {} min", p.duration_minutes);
            println!("description:    {}", p.description);
        }
    }
    Ok(())
}
