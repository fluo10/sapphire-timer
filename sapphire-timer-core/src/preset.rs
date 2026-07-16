//! Timer presets: one TOML file per preset under `presets/`.
//!
//! Presets live outside the `.sapphire-timer/` marker directory on purpose —
//! the framework's indexer skips dot-prefixed directories at any depth, so a
//! preset inside the marker would never be indexed or searchable.
//!
//! Each preset carries a stable [`GrainId`]. Session logs reference presets by
//! that id rather than by name, so renaming a preset does not orphan history.
//! Presets are meant to be hand-writable, so `id` may be omitted; it is then
//! assigned on load and written back (see [`load_presets`]).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use grain_id::GrainId;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Mint an id that is not already in `taken`.
///
/// `GrainId::now_unix()` has decisecond precision, so ids minted back-to-back
/// — assigning ids to a directory of hand-written presets, or writing the
/// starter presets during `init` — collide. Session logs resolve presets by id,
/// so a collision would silently point a log at the wrong preset.
///
/// Incrementing on conflict is the same idiom sapphire-journal uses for entry
/// ids (`increment_until_free`).
pub(crate) fn mint_id(taken: &HashSet<GrainId>) -> GrainId {
    let mut id = GrainId::now_unix();
    while taken.contains(&id) {
        id = id.increment();
    }
    id
}

/// A timer preset, as stored in `presets/{name}.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preset {
    /// Stable identifier. Session logs reference this, never `name`.
    pub id: GrainId,
    /// Human-facing name. Safe to rename: logs are keyed by `id`.
    pub name: String,
    /// Countdown duration in minutes.
    pub duration_minutes: u32,
    /// Free text. Indexed by the framework's TOML chunker, so this is
    /// searchable via `sapphire-timer search`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

/// On-disk shape, where `id` may be absent because a human wrote the file.
#[derive(Debug, Deserialize)]
struct RawPreset {
    #[serde(default)]
    id: Option<GrainId>,
    #[serde(default)]
    name: Option<String>,
    duration_minutes: u32,
    #[serde(default)]
    description: String,
}

impl Preset {
    /// Duration as a [`std::time::Duration`].
    pub fn duration(&self) -> std::time::Duration {
        std::time::Duration::from_secs(u64::from(self.duration_minutes) * 60)
    }

    /// Serialize to the TOML written at `presets/{name}.toml`.
    pub fn to_toml(&self) -> Result<String> {
        Ok(toml::to_string_pretty(self)?)
    }
}

/// Path a preset is stored at within `presets_dir`.
pub fn preset_path(presets_dir: &Path, name: &str) -> PathBuf {
    presets_dir.join(format!("{name}.toml"))
}

/// Read one preset file.
///
/// `taken` holds ids already claimed by presets read so far, so a file without
/// an `id` gets a unique one. Returns the preset plus whether an `id` had to be
/// assigned — the caller decides when to persist that (see [`load_presets`]).
fn read_preset(path: &Path, taken: &HashSet<GrainId>) -> Result<(Preset, bool)> {
    let raw_text = std::fs::read_to_string(path)?;
    let raw: RawPreset = toml::from_str(&raw_text).map_err(|e| Error::InvalidPreset {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;

    if raw.duration_minutes == 0 {
        return Err(Error::InvalidPreset {
            path: path.to_path_buf(),
            message: "duration_minutes must be greater than 0".into(),
        });
    }

    // Fall back to the filename so a preset file only strictly needs a
    // duration.
    let name = match raw.name {
        Some(n) if !n.trim().is_empty() => n,
        _ => path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .ok_or_else(|| Error::InvalidPreset {
                path: path.to_path_buf(),
                message: "cannot derive a name from the filename".into(),
            })?,
    };

    let assigned = raw.id.is_none();
    Ok((
        Preset {
            id: raw.id.unwrap_or_else(|| mint_id(taken)),
            name,
            duration_minutes: raw.duration_minutes,
            description: raw.description,
        },
        assigned,
    ))
}

/// Load every preset under `presets_dir`, sorted by name.
///
/// Ids are made unique as a side effect, and any file whose id changed is
/// rewritten. That covers two cases:
///
/// - a hand-written preset with no `id` — it gets one, so logs can reference it
///   without the author having to invent an id;
/// - two presets sharing an `id`, which happens when someone copies a preset
///   file to make a new one. Logs resolve presets by id, so a duplicate would
///   quietly attribute sessions to the wrong preset.
///
/// Files are processed in path order, so the first file to claim an id keeps
/// it and the outcome does not depend on directory iteration order.
///
/// Rewritten paths are returned so the caller can feed them to
/// `WorkspaceState::on_file_updated` (index + git stage).
pub fn load_presets(presets_dir: &Path) -> Result<(Vec<Preset>, Vec<PathBuf>)> {
    let mut presets = Vec::new();
    let mut rewritten = Vec::new();

    let Ok(entries) = std::fs::read_dir(presets_dir) else {
        return Ok((presets, rewritten));
    };

    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("toml"))
        .collect();
    paths.sort();

    let mut taken: HashSet<GrainId> = HashSet::new();
    for path in paths {
        let (mut preset, assigned) = read_preset(&path, &taken)?;

        // An id read from disk may still collide with one already claimed.
        let duplicate = !assigned && taken.contains(&preset.id);
        if duplicate {
            preset.id = mint_id(&taken);
        }

        if assigned || duplicate {
            std::fs::write(&path, preset.to_toml()?)?;
            rewritten.push(path);
        }
        taken.insert(preset.id);
        presets.push(preset);
    }

    presets.sort_by(|a, b| a.name.cmp(&b.name));
    Ok((presets, rewritten))
}

/// Resolve a preset by name, for CLI arguments.
pub fn find_by_name<'a>(presets: &'a [Preset], name: &str) -> Result<&'a Preset> {
    let matches: Vec<&Preset> = presets.iter().filter(|p| p.name == name).collect();
    match matches.len() {
        0 => Err(Error::PresetNotFound(name.to_owned())),
        1 => Ok(matches[0]),
        n => Err(Error::AmbiguousPreset(name.to_owned(), n)),
    }
}

/// Resolve a preset by id, for rendering historical sessions.
///
/// Returns `None` when the preset has since been deleted — sessions carry a
/// denormalised `preset_name` precisely so they stay readable in that case.
pub fn find_by_id(presets: &[Preset], id: GrainId) -> Option<&Preset> {
    presets.iter().find(|p| p.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn roundtrips_through_toml_including_cjk() {
        let preset = Preset {
            id: GrainId::now_unix(),
            name: "pomodoro".into(),
            duration_minutes: 25,
            description: "Focus block. 25分の集中作業。".into(),
        };
        let parsed: Preset = toml::from_str(&preset.to_toml().unwrap()).unwrap();
        assert_eq!(parsed, preset);
    }

    #[test]
    fn assigns_a_stable_id_to_a_preset_written_without_one() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "pomodoro.toml",
            "name = \"pomodoro\"\nduration_minutes = 25\n",
        );

        let (first, rewritten) = load_presets(tmp.path()).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(rewritten.len(), 1, "the file should have been rewritten");

        // Second load must not re-assign: the id is now persisted.
        let (second, rewritten) = load_presets(tmp.path()).unwrap();
        assert!(rewritten.is_empty(), "id should already be on disk");
        assert_eq!(second[0].id, first[0].id);
    }

    #[test]
    fn presets_minted_together_get_distinct_ids() {
        // GrainId::now_unix() has decisecond precision, so a naive
        // `unwrap_or_else(GrainId::now_unix)` per file hands every preset read
        // in the same tick the same id. Logs resolve presets by id, so that
        // would attribute sessions to the wrong preset.
        let tmp = tempfile::tempdir().unwrap();
        for name in ["a", "b", "c"] {
            write(
                tmp.path(),
                &format!("{name}.toml"),
                "duration_minutes = 5\n",
            );
        }

        let (presets, _) = load_presets(tmp.path()).unwrap();
        let ids: HashSet<GrainId> = presets.iter().map(|p| p.id).collect();
        assert_eq!(ids.len(), 3, "each preset must get its own id");
    }

    #[test]
    fn a_copied_preset_file_is_given_a_fresh_id() {
        let tmp = tempfile::tempdir().unwrap();
        let body = "id = \"gkq44t7\"\nduration_minutes = 5\n";
        write(tmp.path(), "original.toml", body);
        write(tmp.path(), "copy.toml", body);

        let (presets, rewritten) = load_presets(tmp.path()).unwrap();
        let ids: HashSet<GrainId> = presets.iter().map(|p| p.id).collect();
        assert_eq!(ids.len(), 2, "a duplicated id must be reassigned");
        assert_eq!(rewritten.len(), 1, "only the loser is rewritten");

        // Path order decides the winner, so this is stable across runs.
        let (again, rewritten) = load_presets(tmp.path()).unwrap();
        assert!(rewritten.is_empty(), "ids should have settled");
        assert_eq!(
            again.iter().map(|p| p.id).collect::<Vec<_>>(),
            presets.iter().map(|p| p.id).collect::<Vec<_>>()
        );
    }

    #[test]
    fn name_falls_back_to_the_filename() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "break.toml", "duration_minutes = 5\n");

        let (presets, _) = load_presets(tmp.path()).unwrap();
        assert_eq!(presets[0].name, "break");
    }

    #[test]
    fn renaming_a_preset_keeps_its_id() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "pomodoro.toml",
            "name = \"pomodoro\"\nduration_minutes = 25\n",
        );
        let (before, _) = load_presets(tmp.path()).unwrap();
        let id = before[0].id;

        // Rename in place, keeping the id — what a user editing the file does.
        let renamed = Preset {
            name: "focus".into(),
            ..before[0].clone()
        };
        std::fs::write(tmp.path().join("pomodoro.toml"), renamed.to_toml().unwrap()).unwrap();

        let (after, _) = load_presets(tmp.path()).unwrap();
        assert_eq!(after[0].name, "focus");
        assert_eq!(after[0].id, id, "a rename must not change the id");
    }

    #[test]
    fn rejects_a_zero_duration() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "bad.toml", "duration_minutes = 0\n");
        assert!(load_presets(tmp.path()).is_err());
    }

    #[test]
    fn find_by_name_reports_a_missing_preset() {
        assert!(matches!(
            find_by_name(&[], "nope"),
            Err(Error::PresetNotFound(_))
        ));
    }
}
