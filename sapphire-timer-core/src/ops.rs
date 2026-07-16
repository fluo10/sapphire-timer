//! High-level operations shared by frontends.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use chrono::Utc;
use grain_id::GrainId;

use crate::{
    error::Result,
    preset::{self, Preset},
    session::{self, Outcome, Session},
    timer::Timer,
};

/// Presets in the workspace, sorted by name.
///
/// Presets written without an `id` are assigned one and rewritten; the paths
/// touched are returned so the caller can index and stage them.
pub fn list_presets(timer: &Timer) -> Result<(Vec<Preset>, Vec<PathBuf>)> {
    preset::load_presets(&timer.presets_dir()?)
}

/// Every recorded session, oldest first.
pub fn list_sessions(timer: &Timer) -> Result<Vec<Session>> {
    session::load_sessions(&timer.logs_dir()?)
}

/// Why [`run_timer`] stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// The countdown reached zero.
    Elapsed,
    /// The caller asked to stop early (Ctrl-C).
    Interrupted,
}

/// Run a countdown in the foreground, then record the session.
///
/// `tick` is called roughly once a second with the remaining duration so a
/// frontend can render a countdown; this crate does no I/O of its own.
/// `should_stop` is polled on each tick — returning `true` ends the session as
/// [`Outcome::Interrupted`].
///
/// An interrupted session is still written: a countdown the user abandoned
/// after 20 minutes is data, and dropping it would lose real work.
///
/// Returns the recorded session and the log file written, so the caller can
/// index and stage it.
pub fn run_timer(
    timer: &Timer,
    preset: &Preset,
    comment: impl FnOnce(Stop) -> String,
    mut tick: impl FnMut(Duration),
    mut should_stop: impl FnMut() -> bool,
) -> Result<(Session, PathBuf)> {
    let total = preset.duration();
    let started_at = Utc::now();
    let start = Instant::now();

    let stop = loop {
        let elapsed = start.elapsed();
        if elapsed >= total {
            tick(Duration::ZERO);
            break Stop::Elapsed;
        }
        if should_stop() {
            break Stop::Interrupted;
        }
        tick(total - elapsed);

        // Sleep to the next whole second, or to the end, whichever is sooner,
        // so the final tick lands on zero rather than overshooting.
        let remaining = total - start.elapsed();
        std::thread::sleep(remaining.min(Duration::from_secs(1)));
    };

    let ended_at = Utc::now();
    let session = Session {
        id: GrainId::now_unix(),
        preset_id: preset.id,
        preset_name: preset.name.clone(),
        started_at,
        ended_at,
        elapsed_secs: start.elapsed().as_secs(),
        outcome: match stop {
            Stop::Elapsed => Outcome::Completed,
            Stop::Interrupted => Outcome::Interrupted,
        },
        comment: comment(stop),
    };

    let path = session::append(&timer.logs_dir()?, &session)?;
    Ok((session, path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn workspace() -> (tempfile::TempDir, Timer) {
        let tmp = tempfile::tempdir().unwrap();
        let timer = crate::timer::init_workspace(tmp.path()).unwrap();
        (tmp, timer)
    }

    fn preset(minutes: u32) -> Preset {
        Preset {
            id: GrainId::now_unix(),
            name: "test".into(),
            duration_minutes: minutes,
            description: String::new(),
        }
    }

    #[test]
    fn an_interrupted_run_is_still_recorded() {
        let (_tmp, timer) = workspace();
        // An hour long, so it can only end by interruption.
        let (session, path) = run_timer(
            &timer,
            &preset(60),
            |stop| {
                assert_eq!(stop, Stop::Interrupted);
                "gave up".into()
            },
            |_| {},
            || true,
        )
        .unwrap();

        assert_eq!(session.outcome, Outcome::Interrupted);
        assert_eq!(session.comment, "gave up");
        assert!(path.exists(), "an abandoned session must not be dropped");
        assert_eq!(list_sessions(&timer).unwrap().len(), 1);
    }

    #[test]
    fn a_session_references_its_preset_by_id() {
        let (_tmp, timer) = workspace();
        let preset = preset(60);
        let (session, _) = run_timer(&timer, &preset, |_| String::new(), |_| {}, || true).unwrap();

        assert_eq!(session.preset_id, preset.id);
        // Name is denormalised, so the log stays readable after a rename.
        assert_eq!(session.preset_name, preset.name);
    }

    #[test]
    fn ticks_report_remaining_time_before_stopping() {
        let (_tmp, timer) = workspace();
        let ticks = AtomicUsize::new(0);
        let stops = AtomicUsize::new(0);

        run_timer(
            &timer,
            &preset(60),
            |_| String::new(),
            |_| {
                ticks.fetch_add(1, Ordering::Relaxed);
            },
            // Let one tick through, then stop.
            || stops.fetch_add(1, Ordering::Relaxed) > 0,
        )
        .unwrap();

        assert_eq!(ticks.load(Ordering::Relaxed), 1);
    }
}
