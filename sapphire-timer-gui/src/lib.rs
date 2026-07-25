//! Shared egui/eframe GUI for sapphire-timer.
//!
//! This library holds the whole UI so the desktop binary (`main.rs`) and the
//! future mobile / WASM binaries (framework issue #86 Steps C'/E) can all reuse
//! it. It consumes the framework's async [`WorkspaceBackend`] through the
//! `sapphire-framework` facade (one dependency).
//!
//! egui runs on the UI thread; blocking/async workspace operations run on a
//! tokio runtime and report back over a channel, at which point the UI is asked
//! to repaint (the pattern the framework's `AppContext` docs describe).

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use eframe::egui;
use sapphire_framework::backend::{FileSearchResult, LocalBackend, SearchMode, WorkspaceBackend};
use sapphire_framework::workspace::{Workspace, WorkspaceState};
use sapphire_timer_core::{
    Outcome, Preset, Session, TIMER_CTX, Timer, init_app_context, ops, timer::init_workspace,
};

/// Messages sent from background tasks back to the UI thread.
enum Msg {
    SearchDone(Vec<FileSearchResult>),
    Synced { upserted: usize, removed: usize },
    Error(String),
}

/// A running countdown, driven by the egui frame loop.
struct Countdown {
    preset: Preset,
    started_at: DateTime<Utc>,
    start: Instant,
    total: Duration,
    comment: String,
}

/// The sapphire-timer GUI application.
pub struct TimerApp {
    rt: tokio::runtime::Runtime,
    backend: Arc<LocalBackend>,
    timer: Timer,

    presets: Vec<Preset>,
    sessions: Vec<Session>,
    selected: Option<usize>,
    countdown: Option<Countdown>,

    query: String,
    results: Vec<FileSearchResult>,

    status: String,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
}

impl TimerApp {
    /// Open (or create) the default local workspace and build the app.
    ///
    /// The workspace lives at `<data_dir>/workspace`; it is created with the
    /// starter presets on first launch.
    pub fn new() -> anyhow::Result<Self> {
        init_app_context();
        let root = TIMER_CTX.data_dir().join("workspace");
        let timer = match Timer::resolve(Some(&root)) {
            Ok(t) => t,
            Err(_) => init_workspace(&root)?,
        };

        let workspace = Workspace::from_root(&TIMER_CTX, &timer.root)?;
        let state = Arc::new(WorkspaceState::open(workspace)?);
        // Build the index once up front so search works immediately.
        let _ = state.sync();
        let backend = Arc::new(LocalBackend::new(state));

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;

        let (tx, rx) = channel();
        let mut app = Self {
            rt,
            backend,
            timer,
            presets: Vec::new(),
            sessions: Vec::new(),
            selected: None,
            countdown: None,
            query: String::new(),
            results: Vec::new(),
            status: String::new(),
            tx,
            rx,
        };
        app.reload();
        Ok(app)
    }

    /// Reload presets and sessions from disk.
    fn reload(&mut self) {
        match ops::list_presets(&self.timer) {
            Ok((presets, _)) => self.presets = presets,
            Err(e) => self.status = format!("failed to load presets: {e}"),
        }
        match ops::list_sessions(&self.timer) {
            Ok(sessions) => self.sessions = sessions,
            Err(e) => self.status = format!("failed to load sessions: {e}"),
        }
    }

    /// Spawn a full-text search on the runtime; result arrives via [`Msg`].
    fn start_search(&self, ctx: &egui::Context) {
        let backend = Arc::clone(&self.backend);
        let query = self.query.clone();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        self.rt.spawn(async move {
            let msg = match backend.search(&query, 20, SearchMode::Fts).await {
                Ok(results) => Msg::SearchDone(results),
                Err(e) => Msg::Error(e.to_string()),
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }

    /// Spawn a re-index (`sync`) on the runtime.
    fn start_sync(&self, ctx: &egui::Context) {
        let backend = Arc::clone(&self.backend);
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        self.rt.spawn(async move {
            let msg = match backend.sync().await {
                Ok(s) => Msg::Synced {
                    upserted: s.upserted,
                    removed: s.removed,
                },
                Err(e) => Msg::Error(e.to_string()),
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }

    /// Finish the current countdown, record the session, and re-index.
    fn finish_countdown(&mut self, outcome: Outcome, ctx: &egui::Context) {
        let Some(cd) = self.countdown.take() else {
            return;
        };
        let ended_at = Utc::now();
        let elapsed = cd.start.elapsed().as_secs();
        match ops::record_session(
            &self.timer,
            &cd.preset,
            cd.started_at,
            ended_at,
            elapsed,
            outcome,
            cd.comment,
        ) {
            Ok((session, _)) => {
                self.status = format!(
                    "{} {} ({})",
                    match session.outcome {
                        Outcome::Completed => "completed",
                        Outcome::Interrupted => "stopped",
                    },
                    session.preset_name,
                    hms(session.elapsed_secs),
                );
                self.reload();
                // Re-index so the new session is searchable.
                self.start_sync(ctx);
            }
            Err(e) => self.status = format!("failed to record session: {e}"),
        }
    }

    /// Drain any pending background messages.
    fn drain_messages(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::SearchDone(results) => {
                    self.status = format!("{} match(es)", results.len());
                    self.results = results;
                }
                Msg::Synced { upserted, removed } => {
                    self.status = format!("indexed: {upserted} upserted, {removed} removed");
                }
                Msg::Error(e) => self.status = format!("error: {e}"),
            }
        }
    }
}

impl eframe::App for TimerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_messages();

        // ── search bar + status ───────────────────────────────────────────
        egui::TopBottomPanel::top("search").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Search:");
                let resp = ui.text_edit_singleline(&mut self.query);
                let go = ui.button("Go").clicked();
                if go || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                    self.start_search(ctx);
                }
                if ui.button("Re-index").clicked() {
                    self.start_sync(ctx);
                }
            });
            if !self.status.is_empty() {
                ui.small(&self.status);
            }
        });

        // ── presets ───────────────────────────────────────────────────────
        egui::SidePanel::left("presets").show(ctx, |ui| {
            ui.heading("Presets");
            let running = self.countdown.is_some();
            for (i, p) in self.presets.iter().enumerate() {
                let label = format!("{}  ({} min)", p.name, p.duration_minutes);
                if ui
                    .selectable_label(self.selected == Some(i), label)
                    .clicked()
                    && !running
                {
                    self.selected = Some(i);
                }
            }
            if self.presets.is_empty() {
                ui.label("no presets");
            }
        });

        // ── search results / session log ──────────────────────────────────
        egui::TopBottomPanel::bottom("log")
            .resizable(true)
            .show(ctx, |ui| {
                if !self.results.is_empty() {
                    ui.heading("Search results");
                    egui::ScrollArea::vertical()
                        .max_height(140.0)
                        .id_salt("results")
                        .show(ui, |ui| {
                            for r in &self.results {
                                let rel = std::path::Path::new(&r.path)
                                    .strip_prefix(&self.timer.root)
                                    .unwrap_or_else(|_| std::path::Path::new(&r.path));
                                ui.label(rel.display().to_string());
                            }
                        });
                    ui.separator();
                }
                ui.heading("Recent sessions");
                egui::ScrollArea::vertical()
                    .max_height(160.0)
                    .id_salt("sessions")
                    .show(ui, |ui| {
                        for s in self.sessions.iter().rev().take(50) {
                            let mark = match s.outcome {
                                Outcome::Completed => "✓",
                                Outcome::Interrupted => "×",
                            };
                            ui.label(format!(
                                "{mark} {}  {:<12} {:>8}  {}",
                                s.started_at.format("%Y-%m-%d %H:%M"),
                                s.preset_name,
                                hms(s.elapsed_secs),
                                s.comment,
                            ));
                        }
                        if self.sessions.is_empty() {
                            ui.label("no sessions yet");
                        }
                    });
            });

        // ── countdown / start ─────────────────────────────────────────────
        // Decide inside the borrow, act after it is released (so the self-method
        // calls below don't overlap the `&mut self.countdown` borrow).
        let mut finish: Option<Outcome> = None;
        let mut start_selected = false;
        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(cd) = &mut self.countdown {
                let elapsed = cd.start.elapsed();
                if elapsed >= cd.total {
                    finish = Some(Outcome::Completed);
                } else {
                    let remaining = cd.total - elapsed;
                    ui.vertical_centered(|ui| {
                        ui.add_space(20.0);
                        ui.heading(&cd.preset.name);
                        ui.label(
                            egui::RichText::new(hms(remaining.as_secs()))
                                .size(64.0)
                                .monospace(),
                        );
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.label("comment:");
                            ui.text_edit_singleline(&mut cd.comment);
                        });
                        ui.add_space(8.0);
                        if ui.button("■ Stop").clicked() {
                            finish = Some(Outcome::Interrupted);
                        }
                    });
                    ctx.request_repaint_after(Duration::from_millis(200));
                }
            } else {
                ui.vertical_centered(|ui| {
                    ui.add_space(20.0);
                    match self.selected.and_then(|i| self.presets.get(i)) {
                        Some(p) => {
                            ui.heading(&p.name);
                            ui.label(format!("{} minutes", p.duration_minutes));
                            if !p.description.is_empty() {
                                ui.label(&p.description);
                            }
                            ui.add_space(12.0);
                            if ui.button("▶ Start").clicked() {
                                start_selected = true;
                            }
                        }
                        None => {
                            ui.label("select a preset to start");
                        }
                    }
                });
            }
        });

        if let Some(outcome) = finish {
            self.finish_countdown(outcome, ctx);
        } else if start_selected {
            if let Some(p) = self.selected.and_then(|i| self.presets.get(i)) {
                self.countdown = Some(Countdown {
                    started_at: Utc::now(),
                    start: Instant::now(),
                    total: p.duration(),
                    preset: p.clone(),
                    comment: String::new(),
                });
                ctx.request_repaint();
            }
        }
    }
}

/// Format seconds as `MM:SS`, or `H:MM:SS` past an hour.
fn hms(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}
