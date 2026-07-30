//! Shared egui/eframe GUI for sapphire-timer.
//!
//! Two screens: a **workspace manager** (the shared
//! [`sapphire_framework::gui::WorkspaceManager`], so local and remote
//! workspaces are registered/created/opened the same way as the CLI) and the
//! **timer view** for the open workspace (presets, countdown, log, search).
//!
//! The UI lives in this library so the desktop binary (`main.rs`) and the future
//! mobile / WASM binaries (framework #86 Steps C'/E) can reuse it. Blocking /
//! async workspace work runs on a tokio runtime and reports back over a channel.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use eframe::egui;
use sapphire_framework::backend::{
    FileSearchResult, LocalBackend, RemoteBackend, RemoteClient, SearchMode, WorkspaceBackend,
    WorkspaceLocator,
};
use sapphire_framework::gui::{WorkspaceAction, WorkspaceHost, WorkspaceManager};
use sapphire_framework::workspace::{Workspace, WorkspaceState};
use sapphire_timer_core::{
    Outcome, Preset, Session, TIMER_CTX, Timer, init_app_context, ops, timer::init_workspace,
    user_config::UserConfig,
};

/// Which screen is showing.
enum Screen {
    /// The workspace list / management screen.
    Manager,
    /// The timer view for the open workspace.
    Timer,
}

/// Timer's implementation of the shared workspace-host hooks.
struct TimerHost;

impl WorkspaceHost for TimerHost {
    fn app_name(&self) -> &str {
        TIMER_CTX.app_name
    }
    fn default_workspaces_dir(&self) -> PathBuf {
        TIMER_CTX.data_dir().join("workspaces")
    }
    fn create_local(&self, path: &Path, _name: &str) -> Result<(), String> {
        init_workspace(path).map(|_| ()).map_err(|e| e.to_string())
    }
}

/// The application: a workspace manager plus (once opened) an active workspace.
pub struct TimerApp {
    rt: Arc<tokio::runtime::Runtime>,
    config: UserConfig,
    manager: WorkspaceManager,
    host: TimerHost,
    screen: Screen,
    active: Option<Active>,
    status: String,
}

impl TimerApp {
    /// Build the app, loading the workspace registry from the user config.
    pub fn new() -> anyhow::Result<Self> {
        init_app_context();
        let rt = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?,
        );
        let config = UserConfig::load()?;
        Ok(Self {
            rt,
            config,
            manager: WorkspaceManager::new(),
            host: TimerHost,
            screen: Screen::Manager,
            active: None,
            status: String::new(),
        })
    }

    /// Open the workspace referenced by `locator` into an [`Active`] view.
    fn open(&self, locator: WorkspaceLocator) -> Result<Active, String> {
        match locator {
            WorkspaceLocator::Local(path) => {
                let timer = match Timer::resolve(Some(&path)) {
                    Ok(t) => t,
                    Err(_) => init_workspace(&path).map_err(|e| e.to_string())?,
                };
                let ws =
                    Workspace::from_root(&TIMER_CTX, &timer.root).map_err(|e| e.to_string())?;
                let state = WorkspaceState::open(ws).map_err(|e| e.to_string())?;
                let _ = state.sync(); // build/refresh the index up front
                let backend: Arc<dyn WorkspaceBackend> =
                    Arc::new(LocalBackend::new(Arc::new(state)));
                Ok(Active::new(Arc::clone(&self.rt), backend, timer))
            }
            WorkspaceLocator::Remote { url, ws, token } => {
                let ctx = &TIMER_CTX;
                let cache_root = ctx
                    .cache_dir()
                    .join("remotes")
                    .join(mirror_dir_name(&url, &ws));
                std::fs::create_dir_all(&cache_root).map_err(|e| e.to_string())?;
                std::fs::create_dir_all(cache_root.join(format!(".{}", ctx.app_name)))
                    .map_err(|e| e.to_string())?;
                let workspace =
                    Workspace::from_root(ctx, &cache_root).map_err(|e| e.to_string())?;
                let state = Arc::new(WorkspaceState::open(workspace).map_err(|e| e.to_string())?);
                let mut client = RemoteClient::new(url);
                if let Some(t) = &token {
                    client = client.with_token(t);
                }
                let backend: Arc<dyn WorkspaceBackend> =
                    Arc::new(RemoteBackend::new(client, ws, state));
                // Best-effort initial pull so the mirror is populated.
                let _ = self.rt.block_on(backend.sync());
                let timer = Timer::resolve(Some(&cache_root)).map_err(|e| e.to_string())?;
                Ok(Active::new(Arc::clone(&self.rt), backend, timer))
            }
        }
    }
}

impl eframe::App for TimerApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        match self.screen {
            Screen::Manager => {
                if !self.status.is_empty() {
                    ui.small(self.status.as_str());
                }
                let action = self.manager.ui(ui, &mut self.config.workspace, &self.host);
                if let Some(action) = action {
                    match action {
                        WorkspaceAction::Open(id) => {
                            match self
                                .config
                                .workspace
                                .get(&id)
                                .ok_or_else(|| "workspace not found".to_owned())
                                .and_then(|e| e.locator().map_err(|e| e.to_string()))
                                .and_then(|loc| self.open(loc))
                            {
                                Ok(active) => {
                                    self.active = Some(active);
                                    self.screen = Screen::Timer;
                                    self.status.clear();
                                }
                                Err(e) => self.status = format!("could not open '{id}': {e}"),
                            }
                        }
                        WorkspaceAction::Created(_) | WorkspaceAction::Deleted(_) => {
                            if let Err(e) = self.config.save() {
                                self.status = format!("could not save config: {e}");
                            }
                        }
                    }
                }
            }
            Screen::Timer => {
                let back = self.active.as_mut().map(|a| a.ui(ui)).unwrap_or(true);
                if back {
                    self.active = None;
                    self.screen = Screen::Manager;
                }
            }
        }
    }
}

// ── the open-workspace view ──────────────────────────────────────────────────

enum Msg {
    SearchDone(Vec<FileSearchResult>),
    Synced { upserted: usize, removed: usize },
    Error(String),
}

struct Countdown {
    preset: Preset,
    started_at: DateTime<Utc>,
    start: Instant,
    total: Duration,
    comment: String,
}

/// The timer view for one open workspace (local or remote).
struct Active {
    rt: Arc<tokio::runtime::Runtime>,
    backend: Arc<dyn WorkspaceBackend>,
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

impl Active {
    fn new(rt: Arc<tokio::runtime::Runtime>, backend: Arc<dyn WorkspaceBackend>, timer: Timer) -> Self {
        let (tx, rx) = channel();
        let mut a = Self {
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
        a.reload();
        a
    }

    fn reload(&mut self) {
        match ops::list_presets(&self.timer) {
            Ok((p, _)) => self.presets = p,
            Err(e) => self.status = format!("failed to load presets: {e}"),
        }
        match ops::list_sessions(&self.timer) {
            Ok(s) => self.sessions = s,
            Err(e) => self.status = format!("failed to load sessions: {e}"),
        }
    }

    fn drain(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::SearchDone(r) => {
                    self.status = format!("{} match(es)", r.len());
                    self.results = r;
                }
                Msg::Synced { upserted, removed } => {
                    self.status = format!("indexed: {upserted} upserted, {removed} removed");
                }
                Msg::Error(e) => self.status = format!("error: {e}"),
            }
        }
    }

    fn start_search(&self, ctx: &egui::Context) {
        let (backend, query, tx, ctx) =
            (Arc::clone(&self.backend), self.query.clone(), self.tx.clone(), ctx.clone());
        self.rt.spawn(async move {
            let msg = match backend.search(&query, 20, SearchMode::Fts).await {
                Ok(r) => Msg::SearchDone(r),
                Err(e) => Msg::Error(e.to_string()),
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }

    fn start_sync(&self, ctx: &egui::Context) {
        let (backend, tx, ctx) = (Arc::clone(&self.backend), self.tx.clone(), ctx.clone());
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

    fn finish_countdown(&mut self, outcome: Outcome, ctx: &egui::Context) {
        let Some(cd) = self.countdown.take() else {
            return;
        };
        let elapsed = cd.start.elapsed().as_secs();
        match ops::record_session(
            &self.timer,
            &cd.preset,
            cd.started_at,
            Utc::now(),
            elapsed,
            outcome,
            cd.comment,
        ) {
            Ok((session, path)) => {
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
                // Index (local) / push (remote) the written log via the backend.
                if let Ok(content) = std::fs::read_to_string(&path) {
                    let rel = path
                        .strip_prefix(&self.timer.root)
                        .unwrap_or(&path)
                        .to_path_buf();
                    let (backend, tx, ctx) =
                        (Arc::clone(&self.backend), self.tx.clone(), ctx.clone());
                    self.rt.spawn(async move {
                        if let Err(e) = backend.write_file(&rel, &content).await {
                            let _ = tx.send(Msg::Error(e.to_string()));
                        }
                        ctx.request_repaint();
                    });
                }
            }
            Err(e) => self.status = format!("failed to record session: {e}"),
        }
    }

    /// Render the view into `ui`. Returns `true` when the user asked to go back
    /// to the workspace manager.
    fn ui(&mut self, ui: &mut egui::Ui) -> bool {
        self.drain();
        let ctx = ui.ctx().clone();
        let ctx = &ctx;
        let mut back = false;

        egui::Panel::top("bar").show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.button("← Workspaces").clicked() {
                    back = true;
                }
                ui.separator();
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
                ui.small(self.status.as_str());
            }
        });

        egui::Panel::left("presets").show_inside(ui, |ui| {
            ui.heading("Presets");
            let running = self.countdown.is_some();
            for (i, p) in self.presets.iter().enumerate() {
                let label = format!("{}  ({} min)", p.name, p.duration_minutes);
                if ui.selectable_label(self.selected == Some(i), label).clicked() && !running {
                    self.selected = Some(i);
                }
            }
            if self.presets.is_empty() {
                ui.label("no presets");
            }
        });

        egui::Panel::bottom("log").resizable(true).show_inside(ui, |ui| {
            if !self.results.is_empty() {
                ui.heading("Search results");
                egui::ScrollArea::vertical().max_height(120.0).id_salt("results").show(ui, |ui| {
                    for r in &self.results {
                        let rel = Path::new(&r.path).strip_prefix(&self.timer.root).unwrap_or(Path::new(&r.path));
                        ui.label(rel.display().to_string());
                    }
                });
                ui.separator();
            }
            ui.heading("Recent sessions");
            egui::ScrollArea::vertical().max_height(150.0).id_salt("sessions").show(ui, |ui| {
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

        let mut finish: Option<Outcome> = None;
        let mut start_selected = false;
        egui::CentralPanel::default().show_inside(ui, |ui| {
            if let Some(cd) = &mut self.countdown {
                let elapsed = cd.start.elapsed();
                if elapsed >= cd.total {
                    finish = Some(Outcome::Completed);
                } else {
                    let remaining = cd.total - elapsed;
                    ui.vertical_centered(|ui| {
                        ui.add_space(20.0);
                        ui.heading(&cd.preset.name);
                        ui.label(egui::RichText::new(hms(remaining.as_secs())).size(64.0).monospace());
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

        back
    }
}

/// A filesystem-safe directory name for a remote mirror.
fn mirror_dir_name(url: &str, ws: &str) -> String {
    let s = |v: &str| -> String {
        v.chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
            .collect()
    };
    format!("{}_{}", s(url), s(ws))
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
