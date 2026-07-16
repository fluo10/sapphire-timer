//! Timer session log: append-only JSONL under `logs/`.
//!
//! One line per finished session. The log is never rewritten, only appended
//! to, which is what makes the framework's [`JsonlChunker`] a good fit: it
//! keys chunks by line, so appending a session does not re-index — or, when
//! embeddings are on, re-embed — the sessions already there.
//!
//! Logs live outside the `.sapphire-timer/` marker directory, because the
//! framework's indexer skips dot-prefixed directories at any depth.
//!
//! [`JsonlChunker`]: sapphire_workspace::Document

use std::path::{Path, PathBuf};

use chrono::{DateTime, Datelike, Utc};
use grain_id::GrainId;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// How a session ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The countdown ran to zero.
    Completed,
    /// The user interrupted it (Ctrl-C).
    Interrupted,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Completed => "completed",
            Outcome::Interrupted => "interrupted",
        }
    }
}

/// One finished timer session — a single line of `logs/{YYYY-MM}.jsonl`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub id: GrainId,
    /// The preset this ran, by stable id. Renaming a preset keeps this valid.
    pub preset_id: GrainId,
    /// The preset's name *at the time it ran*, denormalised so the log stays
    /// readable after the preset is renamed or deleted.
    pub preset_name: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub elapsed_secs: u64,
    pub outcome: Outcome,
    /// The user's note about the session. This is the main full-text search
    /// target of this app.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub comment: String,
}

/// Path of the log file a session at `at` belongs to: one file per month.
pub fn log_path(logs_dir: &Path, at: DateTime<Utc>) -> PathBuf {
    logs_dir.join(format!("{:04}-{:02}.jsonl", at.year(), at.month()))
}

/// Append one session to its month's log, creating the file if needed.
///
/// Returns the file written, so the caller can hand it to
/// `WorkspaceState::on_file_updated` (index + git stage).
pub fn append(logs_dir: &Path, session: &Session) -> Result<PathBuf> {
    use std::io::Write as _;

    std::fs::create_dir_all(logs_dir)?;
    let path = log_path(logs_dir, session.ended_at);

    // One JSON object per line, LF-terminated. Never pretty-printed: a session
    // must occupy exactly one line for the JSONL chunker to key it by line.
    let mut line = serde_json::to_string(session).map_err(|e| Error::InvalidSession {
        path: path.clone(),
        line: 0,
        message: e.to_string(),
    })?;
    line.push('\n');

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    file.write_all(line.as_bytes())?;
    Ok(path)
}

/// Read every session from one log file, oldest first.
fn read_log(path: &Path) -> Result<Vec<Session>> {
    let text = std::fs::read_to_string(path)?;
    let mut sessions = Vec::new();
    for (idx, raw) in text.lines().enumerate() {
        if raw.trim().is_empty() {
            continue;
        }
        let session: Session = serde_json::from_str(raw).map_err(|e| Error::InvalidSession {
            path: path.to_path_buf(),
            line: idx + 1,
            message: e.to_string(),
        })?;
        sessions.push(session);
    }
    Ok(sessions)
}

/// Read every session across every month, oldest first.
pub fn load_sessions(logs_dir: &Path) -> Result<Vec<Session>> {
    let Ok(entries) = std::fs::read_dir(logs_dir) else {
        return Ok(Vec::new());
    };

    // Month files sort chronologically by name (YYYY-MM), so sorting the paths
    // is enough to order sessions across files.
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("jsonl"))
        .collect();
    paths.sort();

    let mut sessions = Vec::new();
    for path in paths {
        sessions.extend(read_log(&path)?);
    }
    Ok(sessions)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(comment: &str) -> Session {
        let started_at = DateTime::parse_from_rfc3339("2026-07-16T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let ended_at = DateTime::parse_from_rfc3339("2026-07-16T10:25:00Z")
            .unwrap()
            .with_timezone(&Utc);
        Session {
            id: GrainId::now_unix(),
            preset_id: GrainId::now_unix(),
            preset_name: "pomodoro".into(),
            started_at,
            ended_at,
            elapsed_secs: 1500,
            outcome: Outcome::Completed,
            comment: comment.into(),
        }
    }

    #[test]
    fn roundtrips_through_jsonl_including_cjk() {
        let original = session("framework の PR レビュー");
        let line = serde_json::to_string(&original).unwrap();
        assert!(
            !line.contains('\n'),
            "a session must occupy exactly one line"
        );
        let parsed: Session = serde_json::from_str(&line).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn log_path_is_monthly() {
        let at = DateTime::parse_from_rfc3339("2026-07-16T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            log_path(Path::new("/logs"), at).file_name().unwrap(),
            "2026-07.jsonl"
        );
    }

    #[test]
    fn appends_without_rewriting_earlier_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let first = session("first");
        let second = session("second");

        let path = append(tmp.path(), &first).unwrap();
        let after_first = std::fs::read_to_string(&path).unwrap();
        append(tmp.path(), &second).unwrap();
        let after_second = std::fs::read_to_string(&path).unwrap();

        // The whole point of the JSONL log: existing bytes are untouched, so
        // the framework's line-keyed chunker leaves earlier sessions alone.
        assert!(after_second.starts_with(&after_first));
        assert_eq!(after_second.lines().count(), 2);
    }

    #[test]
    fn loads_sessions_across_months_in_order() {
        let tmp = tempfile::tempdir().unwrap();
        let mut july = session("july");
        let mut august = session("august");
        august.ended_at = DateTime::parse_from_rfc3339("2026-08-01T09:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        july.comment = "july".into();

        append(tmp.path(), &august).unwrap();
        append(tmp.path(), &july).unwrap();

        let loaded = load_sessions(tmp.path()).unwrap();
        let comments: Vec<&str> = loaded.iter().map(|s| s.comment.as_str()).collect();
        assert_eq!(comments, ["july", "august"]);
    }

    #[test]
    fn missing_logs_dir_is_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(load_sessions(&tmp.path().join("nope")).unwrap().is_empty());
    }

    #[test]
    fn a_corrupt_line_reports_its_number() {
        let tmp = tempfile::tempdir().unwrap();
        let path = append(tmp.path(), &session("ok")).unwrap();
        std::fs::write(
            &path,
            format!(
                "{}\nnot json\n",
                serde_json::to_string(&session("ok")).unwrap()
            ),
        )
        .unwrap();

        match load_sessions(tmp.path()) {
            Err(Error::InvalidSession { line, .. }) => assert_eq!(line, 2),
            other => panic!("expected InvalidSession, got {other:?}"),
        }
    }
}
