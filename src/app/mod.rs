//! Elm-style `App` struct + `Screen` enum + `Action`/`AppEvent` reducer.
//!
//! M0: project browser shell, theme/banner prefs, splash, help, quit confirm,
//! mouse click regions, ephemeral status bar messages, frame-ms samples.
//! M1: workspace scan Command + cache hydrate + ScanFinished inventory.
//! M1.5: in-TUI workspace root add/edit/delete.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};

use crate::config::{self, Config};
use crate::events::{Action, AppEvent, Command};
use crate::exec::{
    build_publish_and_rebuild, build_publish_and_redeploy, CascadePlan, CascadeStepKind,
    CascadeStepStatus,
};
use crate::graph::DependencyGraph;
use crate::jobs::{Job, JobKind, JobStatus};
use crate::ring_buffer::RingBuffer;
use crate::scan::{self, DiscoveredProject};
use crate::ui::theme::Theme;

mod root_editor;
pub mod settings;

// Re-export so call sites can use `crate::app::BannerMode`.
pub use crate::config::BannerMode;
pub use root_editor::normalize_exclude_pattern;
pub use root_editor::normalize_root_path;
pub use root_editor::RootEditorState;
pub use root_editor::WorkspacePanel;
pub use settings::{SettingId, SettingsEditorState};
#[allow(unused_imports)]
pub use root_editor::RootEditorMode;
#[allow(unused_imports)]
pub use root_editor::WorkspaceEditorKind;

/// How many recent per-paint duration samples the banner's frame-cost graph keeps.
const FRAME_MS_SAMPLE_CAPACITY: usize = 30;

/// How long ephemeral status stays on the bottom status bar before auto-clear.
const EPHEMERAL_STATUS_TTL: Duration = Duration::from_millis(2_500);

/// True for short-lived statuses scheduled via [`App::set_ephemeral_status`].
/// Shown on the bottom status bar and auto-cleared by
/// [`App::expire_status_if_due`]. Sticky messages (`"scanning…"`) are not
/// ephemeral — only scan completion / failure messages are.
///
/// Note: `"scanning…"` is **sticky** (in-flight work) — only scan
/// completion / failure messages count as ephemeral.
pub fn is_ephemeral_status_text(s: &str) -> bool {
    let s = s.to_ascii_lowercase();
    s.starts_with("copied")
        || s.starts_with("pasted")
        || s.starts_with("copy ")
        || s.starts_with("paste ")
        || s.starts_with("pull")
        || s.starts_with("build")
        || s.starts_with("publish")
        // "scan complete…" / "scan failed…" — not "scanning…"
        || (s.starts_with("scan") && !s.starts_with("scanning"))
        || s.starts_with("no scan roots")
        || s.starts_with("no root")
        || s.starts_with("no roots")
        || s.starts_with("added root")
        || s.starts_with("updated root")
        || s.starts_with("deleted root")
        || s.starts_with("cascade")
        || s.starts_with("skaffold")
        || s.starts_with("clean")
        || s.starts_with("git")
        || s.starts_with("cancelled")
        || s.starts_with("job ")
        || s.starts_with("kube ")
        || s.starts_with("filter:")
        || s.contains("fail")
        || s.contains("error")
}

/// Which pane has keyboard focus on the Jobs screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JobsFocus {
    #[default]
    List,
    Log,
}

/// A clickable region of the last-rendered frame, in terminal cell coordinates.
/// `ui::draw` clears these at the start of every frame; screens re-register them
/// while rendering (geometry-only here — plain `u16`s, not `ratatui::Rect` —
/// so `App` stays ratatui-agnostic).
pub struct ClickRegion {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    pub action: Action,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    ProjectBrowser,
    /// Job console (M3+); present in M0 for view-switcher chrome.
    Jobs,
    /// Workspace / scan roots (M1+); present in M0 for view-switcher chrome.
    Workspace,
    /// Config.toml settings editor (all scalar / list fields).
    Settings,
}

/// Project kind heuristic (scan fills this; UI may override later).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectKind {
    Service,
    Library,
    Avro,
    #[default]
    Unknown,
}

impl ProjectKind {
    pub fn label(self) -> &'static str {
        match self {
            ProjectKind::Service => "service",
            ProjectKind::Library => "library",
            ProjectKind::Avro => "avro",
            ProjectKind::Unknown => "unknown",
        }
    }
}

/// Drift between local Gradle version and cluster deployed version (phase 2).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Drift {
    /// Libraries / non-service rows — no cluster mapping.
    #[default]
    NotApplicable,
    /// Service/skaffold row that has never been probed (press **K**).
    NotProbed,
    /// Probed but no Deployment / no version signal found.
    Unknown,
    /// Local == deployed.
    Match,
    /// Local version sorts newer than cluster.
    LocalAhead,
    /// Cluster version sorts newer than local.
    ClusterAhead,
}

impl Drift {
    /// Short browser column label.
    pub fn label(self) -> &'static str {
        match self {
            Drift::NotApplicable | Drift::NotProbed => "—",
            Drift::Unknown => "unknown",
            Drift::Match => "match",
            Drift::LocalAhead => "local▲",
            Drift::ClusterAhead => "cluster▲",
        }
    }

    /// True when the row is interesting for the drift-only filter.
    pub fn is_drift(self) -> bool {
        matches!(self, Drift::LocalAhead | Drift::ClusterAhead | Drift::Unknown)
    }
}

/// One row in the project browser (filled by M1 scan / cache).
///
/// Field set is coordinated with [`scan::DiscoveredProject::to_row`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectRow {
    pub name: String,
    pub kind: ProjectKind,
    pub version: String,
    pub branch: String,
    pub has_skaffold: bool,
    pub git_dirty: bool,
    /// Last job / scan status chip text (e.g. "idle", "ok").
    pub status: String,
    /// First path segment under the scan root (UI group key).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_group: Option<String>,
    /// Absolute filesystem path for the project.
    pub path: PathBuf,
    /// Nearest git root, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_root: Option<PathBuf>,
    /// Primary skaffold file path (first of yaml/yml), when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skaffold_path: Option<PathBuf>,
    /// Nearest Gradle settings/build root for `gradle -p` (when known).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gradle_root: Option<PathBuf>,
    /// Cluster deployed version (phase 2 kube probe); not cached to disk.
    #[serde(default, skip_serializing)]
    pub deployed_version: Option<String>,
    /// Local vs deployed drift (phase 2).
    #[serde(default, skip_serializing)]
    pub drift: Drift,
    /// Soft owner hint from cluster labels (`argocd`, `helm`, …).
    #[serde(default, skip_serializing)]
    pub deploy_owner: Option<String>,
}

impl ProjectRow {
    pub fn new(
        name: impl Into<String>,
        path: impl Into<PathBuf>,
        kind: ProjectKind,
        version: impl Into<String>,
        branch: impl Into<String>,
    ) -> Self {
        let kind = kind;
        Self {
            name: name.into(),
            path: path.into(),
            kind,
            version: version.into(),
            branch: branch.into(),
            has_skaffold: matches!(kind, ProjectKind::Service),
            git_dirty: false,
            status: "idle".into(),
            folder_group: None,
            git_root: None,
            skaffold_path: None,
            gradle_root: None,
            deployed_version: None,
            drift: Drift::NotApplicable,
            deploy_owner: None,
        }
    }

    /// Directory passed to `gradle -p` (gradle_root, else project path).
    pub fn gradle_project_dir(&self) -> &std::path::Path {
        self.gradle_root.as_deref().unwrap_or(self.path.as_path())
    }

    /// Name column: `group/leaf` when grouped, else `name`.
    pub fn display_name(&self) -> String {
        match &self.folder_group {
            Some(g) if !g.is_empty() && !self.name.starts_with(g) => {
                // name may already be "group/leaf" from scan relative_display_name
                if self.name.contains('/') {
                    self.name.clone()
                } else {
                    format!("{g}/{}", self.name)
                }
            }
            _ => self.name.clone(),
        }
    }
}

/// Stable display order: folder_group then name; returns indices into `projects`.
pub fn project_display_order(projects: &[ProjectRow]) -> Vec<usize> {
    let mut idxs: Vec<usize> = (0..projects.len()).collect();
    idxs.sort_by(|&a, &b| {
        let ga = projects[a].folder_group.as_deref().unwrap_or("");
        let gb = projects[b].folder_group.as_deref().unwrap_or("");
        ga.cmp(gb)
            .then_with(|| projects[a].name.cmp(&projects[b].name))
            .then_with(|| projects[a].path.cmp(&projects[b].path))
    });
    idxs
}

pub struct App {
    pub screen: Screen,
    pub config: Config,
    /// Path to `config.toml` (used when persisting UI prefs).
    pub config_path: PathBuf,
    /// Workspace inventory cache path (`…/cache/workspace.json`).
    pub cache_path: PathBuf,
    /// Project inventory rows (empty until M1 scan / cache hydrate).
    pub projects: Vec<ProjectRow>,
    /// Static dependency graph (built on scan finish; empty until then).
    pub graph: DependencyGraph,
    /// Inventory indices that the selected project depends on (cyan tint).
    pub dep_indices: HashSet<usize>,
    /// Inventory indices that depend on the selected project (warning tint).
    pub dependent_indices: HashSet<usize>,
    /// Enter opens a **full-screen** project detail (produces/depends + actions).
    pub project_detail_visible: bool,
    /// Scroll offset (lines) into the project detail body.
    pub project_detail_scroll: usize,
    /// Selection cursor into `projects` (storage index — same space as j/k target).
    pub selected_index: usize,
    /// Multi-selected project indices into `projects` (`Space` toggles; Esc clears).
    /// Indices match `selected_index` / storage order, not display order.
    pub multi_selected: HashSet<usize>,
    /// Transient status text shown on the bottom status bar (job feedback,
    /// scan progress, errors).
    pub status_message: Option<String>,
    /// When set, [`Self::expire_status_if_due`] clears `status_message` after this
    /// instant — used for ephemeral feedback so it doesn't stick forever.
    pub status_clear_at: Option<Instant>,
    pub should_quit: bool,
    /// When true, a centered "quit?" dialog is open (`q`); y/Enter exits, n/Esc cancels.
    pub quit_confirm: bool,
    /// Braille stream banner animation frame index.
    pub banner_frame: usize,
    /// Cycles wave → ms/frame → paint-capacity fps → off (`A`). Loaded from /
    /// saved to `[ui].banner_mode`.
    pub banner_mode: BannerMode,
    /// Active color theme (from `[ui].theme`); cycled with `T` and persisted.
    pub theme: Theme,
    /// Global help overlay (`?`).
    pub help_visible: bool,
    /// Recent per-paint durations in milliseconds (oldest first).
    pub frame_ms_samples: RingBuffer<f64>,
    /// Full-screen splash — shown once at startup until dismissed.
    pub show_splash: bool,
    /// True while a scan Command is in flight (sticky status).
    pub scanning: bool,
    /// When the last successful scan finished (for Workspace "last scan" label).
    pub last_scan_at: Option<SystemTime>,
    /// Selection cursor into `config.scan.roots` on the Workspace screen.
    pub selected_root_index: usize,
    /// Selection cursor into `config.scan.exclude` on the Workspace screen.
    pub selected_exclude_index: usize,
    /// Focused list on Workspace: scan roots vs project excludes (`Tab`).
    pub workspace_panel: WorkspacePanel,
    /// Path / pattern form overlay for add/edit root or exclude (Workspace).
    pub root_editor: Option<RootEditorState>,
    /// When set, a "delete this root?" dialog is open for that index.
    pub root_delete_confirm: Option<usize>,
    /// When set, a "delete this exclude?" dialog is open for that index.
    pub exclude_delete_confirm: Option<usize>,
    /// Settings screen cursor into [`SettingId::ALL`].
    pub selected_setting: usize,
    /// Text editor overlay for a settings field.
    pub settings_editor: Option<SettingsEditorState>,
    /// Background job console (M3+). Newest jobs are pushed at the end.
    pub jobs: Vec<Job>,
    /// Selection cursor into `jobs`.
    pub selected_job: usize,
    /// Jobs list vs log pane focus (`Tab`).
    pub jobs_focus: JobsFocus,
    /// Vertical scroll offset into the selected job's log (line index).
    pub job_log_scroll: usize,
    /// Open cascade plan (confirm overlay when `!executing`; sequencing when executing).
    pub plan: Option<CascadePlan>,
    /// Phase 2: kube probe in flight.
    pub kube_probing: bool,
    /// When the last successful kube probe finished.
    pub kube_probed_at: Option<Instant>,
    /// Phase 2: project browser shows only rows with drift / unknown deployed.
    pub filter_drift_only: bool,
    /// Monotonic job id allocator.
    next_job_id: u64,
    /// Clickable regions registered by the most recent `ui::draw` call.
    click_regions: RefCell<Vec<ClickRegion>>,
    /// Last-known mouse cell position for hover highlighting.
    mouse_pos: Cell<Option<(u16, u16)>>,
}

impl App {
    pub fn new(config: Config, config_path: PathBuf) -> Self {
        let banner_mode = config.ui.banner_mode;
        let theme = Theme::from_name(config.ui.theme);
        let cache_path = config_path
            .parent()
            .map(config::workspace_cache_path_in)
            .unwrap_or_else(|| PathBuf::from("cache/workspace.json"));

        // Instant list from cache while a background rescan may still run.
        let (projects, last_scan_at) = match scan::load_workspace_cache(&cache_path) {
            Some(c) => {
                let at = SystemTime::UNIX_EPOCH + Duration::from_secs(c.scanned_at);
                (c.projects, Some(at))
            }
            None => (Vec::new(), None),
        };

        Self {
            screen: Screen::ProjectBrowser,
            config,
            config_path,
            cache_path,
            projects,
            graph: DependencyGraph::empty(),
            dep_indices: HashSet::new(),
            dependent_indices: HashSet::new(),
            project_detail_visible: false,
            project_detail_scroll: 0,
            selected_index: 0,
            multi_selected: HashSet::new(),
            status_message: None,
            status_clear_at: None,
            should_quit: false,
            quit_confirm: false,
            banner_frame: 0,
            banner_mode,
            theme,
            help_visible: false,
            frame_ms_samples: RingBuffer::new(FRAME_MS_SAMPLE_CAPACITY),
            show_splash: true,
            scanning: false,
            last_scan_at,
            selected_root_index: 0,
            selected_exclude_index: 0,
            workspace_panel: WorkspacePanel::Roots,
            exclude_delete_confirm: None,
            selected_setting: 0,
            settings_editor: None,
            root_editor: None,
            root_delete_confirm: None,
            jobs: Vec::new(),
            selected_job: 0,
            jobs_focus: JobsFocus::List,
            job_log_scroll: 0,
            plan: None,
            kube_probing: false,
            kube_probed_at: None,
            filter_drift_only: false,
            next_job_id: 1,
            click_regions: RefCell::new(Vec::new()),
            mouse_pos: Cell::new(None),
        }
    }

    /// True when a cascade plan confirm overlay owns keyboard focus.
    pub fn plan_confirming(&self) -> bool {
        self.plan.as_ref().is_some_and(|p| p.is_confirming())
    }

    /// Recompute dep/dependent highlight sets for the current selection.
    pub fn recompute_highlights(&mut self) {
        if self.projects.is_empty() || self.graph.is_empty() {
            self.dep_indices.clear();
            self.dependent_indices.clear();
            return;
        }
        let (deps, dependents) = self.graph.highlight_for_index(self.selected_index);
        self.dep_indices = deps;
        self.dependent_indices = dependents;
    }

    /// Replace inventory + graph from a scan result and refresh highlights.
    fn apply_scan_projects(&mut self, discovered: &[DiscoveredProject]) {
        self.graph = DependencyGraph::from_projects(discovered);
        self.projects = discovered.iter().map(DiscoveredProject::to_row).collect();
        self.clamp_selection();
        self.prune_multi_selected();
        self.recompute_highlights();
    }

    /// Commands to run once after `App::new` (initial scan when roots configured).
    pub fn startup_commands(&mut self) -> Vec<Command> {
        if self.config.scan.roots.is_empty() {
            return vec![];
        }
        self.begin_scan_status();
        vec![Command::ScanWorkspace]
    }

    /// Sticky in-flight status for a workspace walk.
    pub(crate) fn begin_scan_status(&mut self) {
        self.scanning = true;
        self.status_message = Some("scanning…".into());
        self.status_clear_at = None;
    }

    /// Persist `[ui]` fields currently reflected in `self` (theme, banner_mode).
    /// Soft-fails into `status_message` so a write error never blocks the TUI.
    fn persist_ui_prefs(&mut self) {
        self.config.ui.theme = self.theme.name;
        self.config.ui.banner_mode = self.banner_mode;
        if let Err(err) = config::save(&self.config_path, &self.config) {
            self.status_message = Some(format!("failed to save UI prefs: {err}"));
            self.status_clear_at = None;
        }
    }

    /// Soft-fail cache write after a successful scan.
    fn persist_workspace_cache(&mut self) {
        if let Err(err) = scan::save_workspace_cache(&self.cache_path, &self.projects) {
            tracing::warn!("failed to save workspace cache: {err}");
        }
    }

    /// Flash a short-lived status-bar message. Cleared by [`Self::expire_status_if_due`]
    /// after [`EPHEMERAL_STATUS_TTL`].
    pub fn set_ephemeral_status(&mut self, msg: impl Into<String>) {
        self.status_message = Some(msg.into());
        self.status_clear_at = Some(Instant::now() + EPHEMERAL_STATUS_TTL);
    }

    /// Clears an ephemeral status once its deadline has passed. Returns `true`
    /// when the message was removed (so the event loop knows a redraw matters).
    /// Sticky statuses (`status_clear_at == None`, or a sticky message that
    /// replaced an ephemeral one before the deadline) are left alone.
    pub fn expire_status_if_due(&mut self) -> bool {
        let Some(at) = self.status_clear_at else {
            return false;
        };
        if Instant::now() < at {
            return false;
        }
        self.status_clear_at = None;
        if self
            .status_message
            .as_deref()
            .is_some_and(is_ephemeral_status_text)
        {
            self.status_message = None;
            true
        } else {
            false
        }
    }

    /// Discards last frame's clickable regions; called once at the top of `ui::draw`.
    pub fn clear_click_regions(&self) {
        self.click_regions.borrow_mut().clear();
    }

    /// Records one paint's wall duration in milliseconds (timed around
    /// `terminal.draw` — not the gap between draws).
    pub fn push_frame_ms_sample(&mut self, frame_ms: f64) {
        self.frame_ms_samples.push(frame_ms);
    }

    /// Registers a clickable region for the frame currently being drawn.
    pub fn register_click(&self, x: u16, y: u16, width: u16, height: u16, action: Action) {
        self.click_regions.borrow_mut().push(ClickRegion {
            x,
            y,
            width,
            height,
            action,
        });
    }

    /// The action for the topmost region containing `(x, y)`, if any — later
    /// registrations win (dialogs over screens).
    pub fn action_at(&self, x: u16, y: u16) -> Option<Action> {
        self.click_regions
            .borrow()
            .iter()
            .rev()
            .find(|r| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height)
            .map(|r| r.action.clone())
    }

    /// Records the mouse's current cell position for hover highlighting.
    pub fn set_mouse_pos(&self, x: u16, y: u16) {
        self.mouse_pos.set(Some((x, y)));
    }

    /// Whether the last-known mouse position falls inside the given rect.
    pub fn is_hovered(&self, x: u16, y: u16, width: u16, height: u16) -> bool {
        self.mouse_pos
            .get()
            .is_some_and(|(mx, my)| mx >= x && mx < x + width && my >= y && my < y + height)
    }

    fn move_selection(&mut self, delta: i32) {
        match self.screen {
            Screen::Jobs => self.move_job_selection(delta),
            _ => {
                let len = self.projects.len();
                if len == 0 {
                    return;
                }
                // Move through display order so j/k matches the on-screen list.
                let order = project_display_order(&self.projects);
                let cur_pos = order
                    .iter()
                    .position(|&i| i == self.selected_index)
                    .unwrap_or(0) as i32;
                let next_pos = (cur_pos + delta).clamp(0, (order.len() as i32) - 1) as usize;
                self.selected_index = order[next_pos];
                self.recompute_highlights();
            }
        }
    }

    fn move_job_selection(&mut self, delta: i32) {
        if self.jobs_focus == JobsFocus::Log {
            self.scroll_job_log(delta);
            return;
        }
        let len = self.jobs.len();
        if len == 0 {
            return;
        }
        let cur = self.selected_job as i32;
        let next = (cur + delta).clamp(0, (len as i32) - 1) as usize;
        if next != self.selected_job {
            self.selected_job = next;
            self.job_log_scroll = 0;
        }
    }

    fn scroll_job_log(&mut self, delta: i32) {
        let Some(job) = self.jobs.get(self.selected_job) else {
            return;
        };
        let max = job.log.len().saturating_sub(1);
        let cur = self.job_log_scroll as i32;
        self.job_log_scroll = (cur + delta).clamp(0, max as i32) as usize;
    }

    fn set_selection(&mut self, index: usize) {
        match self.screen {
            Screen::Jobs => {
                if index < self.jobs.len() {
                    self.selected_job = index;
                    self.job_log_scroll = 0;
                }
            }
            _ => {
                if index < self.projects.len() {
                    self.selected_index = index;
                    self.project_detail_scroll = 0;
                    self.recompute_highlights();
                }
            }
        }
    }

    fn open_project_detail(&mut self) {
        self.project_detail_visible = true;
        self.project_detail_scroll = 0;
        self.recompute_highlights();
    }

    fn close_project_detail(&mut self) {
        self.project_detail_visible = false;
        self.project_detail_scroll = 0;
    }

    /// Open `publish_and_rebuild_consumers` plan (**B**): publish cursor project,
    /// then sequential gradle build of graph **dependents** (no skaffold).
    /// Consumer builds use the SNAPSHOT-refresh init script when configured.
    fn start_build_with_deps(&mut self) -> Vec<Command> {
        if self.plan.as_ref().is_some_and(|p| p.executing) {
            self.set_ephemeral_status("cascade already running");
            return vec![];
        }
        if self.projects.is_empty() {
            self.set_ephemeral_status("no project selected");
            return vec![];
        }
        self.project_detail_visible = false;
        self.help_visible = false;

        match build_publish_and_rebuild(
            &self.projects,
            &self.graph,
            self.selected_index,
            &self.config,
        ) {
            Ok(plan) => {
                let n = plan.steps.len();
                let name = plan.source_name.clone();
                self.plan = Some(plan);
                self.status_message = Some(format!(
                    "rebuild plan: {name} ({n} steps) — y run / n cancel"
                ));
                self.status_clear_at = None;
            }
            Err(err) => {
                self.plan = None;
                self.set_ephemeral_status(format!("rebuild: {err}"));
            }
        }
        vec![]
    }

    fn clamp_selection(&mut self) {
        if self.projects.is_empty() {
            self.selected_index = 0;
        } else if self.selected_index >= self.projects.len() {
            self.selected_index = self.projects.len() - 1;
        }
        if self.jobs.is_empty() {
            self.selected_job = 0;
        } else if self.selected_job >= self.jobs.len() {
            self.selected_job = self.jobs.len() - 1;
        }
    }

    /// Drop multi-select indices that no longer exist after rescan / prune.
    fn prune_multi_selected(&mut self) {
        let n = self.projects.len();
        self.multi_selected.retain(|&i| i < n);
    }

    fn selected_project(&self) -> Option<&ProjectRow> {
        self.projects.get(self.selected_index)
    }

    /// Targets for bulk-capable actions: multi-set when non-empty, else cursor row.
    /// Returns storage indices into `projects`, sorted ascending.
    pub fn action_target_indices(&self) -> Vec<usize> {
        if !self.multi_selected.is_empty() {
            let mut idxs: Vec<usize> = self
                .multi_selected
                .iter()
                .copied()
                .filter(|&i| i < self.projects.len())
                .collect();
            idxs.sort_unstable();
            idxs
        } else if !self.projects.is_empty() && self.selected_index < self.projects.len() {
            vec![self.selected_index]
        } else {
            vec![]
        }
    }

    /// Toggle multi-select on the cursor project (no-op if inventory empty).
    fn toggle_multi_select(&mut self) {
        if self.projects.is_empty() || self.selected_index >= self.projects.len() {
            return;
        }
        let idx = self.selected_index;
        if !self.multi_selected.remove(&idx) {
            self.multi_selected.insert(idx);
        }
    }

    fn alloc_job_id(&mut self) -> u64 {
        let id = self.next_job_id;
        self.next_job_id = self.next_job_id.saturating_add(1);
        id
    }

    /// Push a job, select it, update the project status chip.
    fn register_job(&mut self, job: Job) -> u64 {
        let id = job.id;
        let path = job.project_path.clone();
        let chip = job.kind.running_chip();
        self.set_project_status_by_path(&path, chip);
        self.jobs.push(job);
        self.selected_job = self.jobs.len() - 1;
        self.job_log_scroll = 0;
        id
    }

    fn set_project_status_by_path(&mut self, path: &std::path::Path, status: impl Into<String>) {
        let status = status.into();
        if let Some(p) = self.projects.iter_mut().find(|p| p.path == path) {
            p.status = status;
        }
    }

    fn find_job_mut(&mut self, id: u64) -> Option<&mut Job> {
        self.jobs.iter_mut().find(|j| j.id == id)
    }

    /// Start Gradle job(s) for action targets (cursor or multi-select).
    fn start_gradle(&mut self, tasks: Vec<String>) -> Vec<Command> {
        let targets = self.action_target_indices();
        self.start_gradle_on(targets, tasks)
    }

    /// Start Gradle on the cursor project only (publish / non-bulk verbs).
    fn start_gradle_single(&mut self, tasks: Vec<String>) -> Vec<Command> {
        if self.projects.is_empty() || self.selected_index >= self.projects.len() {
            self.set_ephemeral_status("no project selected");
            return vec![];
        }
        self.start_gradle_on(vec![self.selected_index], tasks)
    }

    fn start_gradle_on(&mut self, targets: Vec<usize>, tasks: Vec<String>) -> Vec<Command> {
        if tasks.is_empty() {
            self.set_ephemeral_status("no gradle tasks configured");
            return vec![];
        }
        if targets.is_empty() {
            self.set_ephemeral_status("no project selected");
            return vec![];
        }

        let label = tasks.join(" ");
        let mut cmds = Vec::with_capacity(targets.len());
        let mut names: Vec<String> = Vec::with_capacity(targets.len());
        for &idx in &targets {
            let Some(project) = self.projects.get(idx).cloned() else {
                continue;
            };
            let id = self.alloc_job_id();
            let path = project.gradle_project_dir().to_path_buf();
            let kind = JobKind::Gradle {
                tasks: tasks.clone(),
            };
            let job = Job::new(id, project.name.clone(), project.path.clone(), kind);
            self.register_job(job);
            names.push(project.name.clone());
            cmds.push(Command::RunGradle {
                id,
                path,
                tasks: tasks.clone(),
                project_name: project.name,
                force_latest_snapshots: false,
            });
        }
        if cmds.is_empty() {
            self.set_ephemeral_status("no project selected");
            return vec![];
        }
        self.screen = Screen::Jobs;
        let status = if names.len() == 1 {
            format!("{label}… {}", names[0])
        } else {
            format!("{label}… {} projects", names.len())
        };
        self.status_message = Some(status);
        self.status_clear_at = None;
        cmds
    }

    fn start_skaffold(&mut self, subcommand: &str, extra_args: Vec<String>) -> Vec<Command> {
        // Skaffold stays single-project (cursor); multi-select bulk is build/clean/pull.
        let Some(project) = self.selected_project().cloned() else {
            self.set_ephemeral_status("no project selected");
            return vec![];
        };
        let Some(skaffold_file) = project.skaffold_path.clone() else {
            self.set_ephemeral_status(format!("no skaffold file: {}", project.name));
            return vec![];
        };
        // Soft warn when cluster probe saw Argo ownership (still allows run).
        let argo_warn = project.deploy_owner.as_deref() == Some("argocd");
        if argo_warn {
            tracing::warn!(
                project = %project.name,
                "skaffold on Argo-managed workload may fail or be reverted"
            );
        }
        let id = self.alloc_job_id();
        let mut args = vec![subcommand.to_string()];
        args.extend(extra_args);
        // Profile from config when set.
        let profile = self.config.skaffold.default_profile.clone();
        if !profile.is_empty() {
            args.push(format!("-p={profile}"));
        }
        let kind = JobKind::Skaffold {
            subcommand: subcommand.to_string(),
        };
        let job = Job::new(id, project.name.clone(), project.path.clone(), kind);
        self.register_job(job);
        self.screen = Screen::Jobs;
        self.status_message = Some(if argo_warn {
            format!(
                "skaffold {subcommand}… {} · ⚠ Argo-managed (may fail/revert)",
                project.name
            )
        } else {
            format!("skaffold {subcommand}… {}", project.name)
        });
        self.status_clear_at = None;
        vec![Command::RunSkaffold {
            id,
            path: project.path.clone(),
            args,
            skaffold_file,
            project_name: project.name,
        }]
    }

    /// Git pull for action targets: one job per unique `git_root` among targets.
    fn start_git_pull(&mut self) -> Vec<Command> {
        let targets = self.action_target_indices();
        if targets.is_empty() {
            self.set_ephemeral_status("no project selected");
            return vec![];
        }

        // First project path per git_root (stable by target order).
        let mut planned: Vec<(PathBuf, String, PathBuf)> = Vec::new();
        let mut seen_roots: HashSet<PathBuf> = HashSet::new();
        let mut missing_git = 0usize;
        for &idx in &targets {
            let Some(project) = self.projects.get(idx) else {
                continue;
            };
            let Some(git_root) = project.git_root.clone() else {
                missing_git += 1;
                continue;
            };
            if !seen_roots.insert(git_root.clone()) {
                continue;
            }
            planned.push((git_root, project.name.clone(), project.path.clone()));
        }

        if planned.is_empty() {
            if missing_git > 0 {
                self.set_ephemeral_status("no git root on selected project(s)");
            } else {
                self.set_ephemeral_status("no project selected");
            }
            return vec![];
        }

        let mut cmds = Vec::new();
        let mut started_names: Vec<String> = Vec::new();
        let mut skipped_active = 0usize;
        for (git_root, name, path) in planned {
            // Dedupe against already-running pulls for the same root.
            let already = self.jobs.iter().any(|j| {
                j.status.is_active()
                    && matches!(j.kind, JobKind::GitPull)
                    && j.git_root.as_ref() == Some(&git_root)
            });
            if already {
                skipped_active += 1;
                continue;
            }
            let id = self.alloc_job_id();
            let job = Job::new(id, name.clone(), path.clone(), JobKind::GitPull)
                .with_git_root(git_root.clone());
            self.register_job(job);
            started_names.push(name.clone());
            cmds.push(Command::GitPull {
                id,
                git_root,
                project_name: name,
                project_path: path,
                ff_only: self.config.git.pull_ff_only,
            });
        }

        if cmds.is_empty() {
            if skipped_active > 0 {
                self.set_ephemeral_status("pull already running for selected root(s)");
            } else {
                self.set_ephemeral_status("no git root on selected project(s)");
            }
            return vec![];
        }

        self.screen = Screen::Jobs;
        let status = if started_names.len() == 1 {
            format!("pull… {}", started_names[0])
        } else {
            format!("pull… {} repos", started_names.len())
        };
        self.status_message = Some(status);
        self.status_clear_at = None;
        cmds
    }

    fn cancel_focused_job(&mut self) -> Vec<Command> {
        let Some(job) = self.jobs.get(self.selected_job) else {
            return vec![];
        };
        if !job.status.is_active() {
            self.set_ephemeral_status("job not running");
            return vec![];
        }
        let id = job.id;
        vec![Command::CancelJob { id }]
    }

    /// Open `publish_and_redeploy_consumers` plan for the cursor project (`P`).
    fn open_cascade_publish(&mut self) -> Vec<Command> {
        if self.plan.as_ref().is_some_and(|p| p.executing) {
            self.set_ephemeral_status("cascade already running");
            return vec![];
        }
        if self.projects.is_empty() {
            self.set_ephemeral_status("no project selected");
            return vec![];
        }
        self.project_detail_visible = false;
        self.help_visible = false;

        match build_publish_and_redeploy(
            &self.projects,
            &self.graph,
            self.selected_index,
            &self.config,
        ) {
            Ok(plan) => {
                let n = plan.steps.len();
                let name = plan.source_name.clone();
                self.plan = Some(plan);
                self.status_message =
                    Some(format!("cascade plan: {name} ({n} steps) — y run / n cancel"));
                self.status_clear_at = None;
            }
            Err(err) => {
                self.plan = None;
                self.set_ephemeral_status(format!("cascade: {err}"));
            }
        }
        vec![]
    }

    fn cancel_cascade_plan(&mut self) {
        if self.plan.as_ref().is_some_and(|p| p.executing) {
            return;
        }
        if self.plan.is_some() {
            self.plan = None;
            self.set_ephemeral_status("cascade cancelled");
        }
    }

    /// Confirm plan → mark executing and fill the parallel job pool.
    fn confirm_cascade_plan(&mut self) -> Vec<Command> {
        let Some(plan) = self.plan.as_mut() else {
            return vec![];
        };
        if plan.executing {
            return vec![];
        }
        if plan.steps.is_empty() {
            self.plan = None;
            self.set_ephemeral_status("cascade: empty plan");
            return vec![];
        }
        plan.executing = true;
        plan.cursor = 0;
        plan.active_jobs.clear();
        let name = plan.source_name.clone();
        let n = plan.steps.len();
        self.screen = Screen::Jobs;
        self.status_message = Some(format!("cascade… {name} (0/{n})"));
        self.status_clear_at = None;
        self.dispatch_cascade_ready()
    }

    /// Start as many ready cascade steps as `max_parallel` allows.
    fn dispatch_cascade_ready(&mut self) -> Vec<Command> {
        let max_parallel = self.config.cascade.max_parallel.max(1);
        let mut cmds = Vec::new();

        // Loop so skipped steps (e.g. missing skaffold file) free the pool for others.
        for _ in 0..64 {
            let Some(plan) = self.plan.as_ref() else {
                break;
            };
            if !plan.executing {
                break;
            }
            let indices = plan.indices_to_dispatch(max_parallel);
            if indices.is_empty() {
                break;
            }
            let mut spawned = 0usize;
            for idx in indices {
                if let Some(cmd) = self.spawn_cascade_step_at(idx) {
                    cmds.push(cmd);
                    spawned += 1;
                }
            }
            if spawned == 0 {
                // Only skips — statuses changed; re-evaluate once more, then stop.
                let still_ready = self
                    .plan
                    .as_ref()
                    .map(|p| !p.indices_to_dispatch(max_parallel).is_empty())
                    .unwrap_or(false);
                if !still_ready {
                    break;
                }
            }
        }

        if self.plan.as_ref().is_some_and(|p| p.is_fully_done()) {
            let name = self
                .plan
                .as_ref()
                .map(|p| p.source_name.clone())
                .unwrap_or_default();
            let n = self.plan.as_ref().map(|p| p.steps.len()).unwrap_or(0);
            self.plan = None;
            self.set_ephemeral_status(format!("cascade complete: {name} ({n} steps ok)"));
        } else {
            self.refresh_cascade_status();
        }
        cmds
    }

    fn refresh_cascade_status(&mut self) {
        let Some(plan) = self.plan.as_ref() else {
            return;
        };
        let n = plan.steps.len();
        let done = plan.completed_count();
        let running = plan.running_count();
        let name = plan.source_name.clone();
        let running_names: Vec<&str> = plan
            .active_jobs
            .values()
            .filter_map(|&i| plan.steps.get(i).map(|s| s.project_name.as_str()))
            .collect();
        let detail = if running_names.is_empty() {
            String::new()
        } else if running_names.len() == 1 {
            format!(" · {}", running_names[0])
        } else {
            format!(" · {} jobs", running_names.len())
        };
        self.status_message = Some(format!(
            "cascade… {name} ({done}/{n} done, {running} running{detail})"
        ));
        self.status_clear_at = None;
    }

    /// Spawn one cascade step at `idx` (must be Pending). Returns None if skipped.
    fn spawn_cascade_step_at(&mut self, idx: usize) -> Option<Command> {
        let step = self.plan.as_ref()?.steps.get(idx)?.clone();
        if step.status != CascadeStepStatus::Pending {
            return None;
        }

        match step.kind {
            CascadeStepKind::Gradle {
                tasks,
                force_latest_snapshots,
            } => {
                let id = self.alloc_job_id();
                if let Some(p) = self.plan.as_mut() {
                    p.register_active(id, idx);
                }
                let kind = JobKind::Gradle {
                    tasks: tasks.clone(),
                };
                let job = Job::new(id, step.project_name.clone(), step.project_path.clone(), kind);
                self.register_job(job);
                Some(Command::RunGradle {
                    id,
                    path: step.gradle_dir,
                    tasks,
                    project_name: step.project_name,
                    force_latest_snapshots,
                })
            }
            CascadeStepKind::Skaffold { subcommand } => {
                let Some(skaffold_file) = step.skaffold_file.clone() else {
                    // Missing file — skip and try to fill the pool again.
                    if let Some(p) = self.plan.as_mut() {
                        if let Some(s) = p.steps.get_mut(idx) {
                            s.status = CascadeStepStatus::Skipped;
                            s.job_id = None;
                        }
                    }
                    return None;
                };
                let id = self.alloc_job_id();
                if let Some(p) = self.plan.as_mut() {
                    p.register_active(id, idx);
                }
                let mut args = vec![subcommand.clone()];
                let profile = self.config.skaffold.default_profile.clone();
                if !profile.is_empty() {
                    args.push(format!("-p={profile}"));
                }
                let kind = JobKind::Skaffold {
                    subcommand: subcommand.clone(),
                };
                let job = Job::new(id, step.project_name.clone(), step.project_path.clone(), kind);
                self.register_job(job);
                Some(Command::RunSkaffold {
                    id,
                    path: step.project_path,
                    args,
                    skaffold_file,
                    project_name: step.project_name,
                })
            }
        }
    }

    /// Handle JobFinished for an in-flight cascade step; may return more Commands.
    fn advance_cascade_on_job_finished(
        &mut self,
        job_id: u64,
        ok: bool,
        cancelled: bool,
        job_name: &str,
        kind_label: &str,
        log_tail: &str,
    ) -> Option<Vec<Command>> {
        let is_cascade = self.plan.as_ref().is_some_and(|p| p.owns_job(job_id));
        if !is_cascade {
            return None;
        }

        let plan = self.plan.as_mut().unwrap();
        let step_idx = plan.finish_job(job_id, ok, cancelled)?;
        let n = plan.steps.len();

        if cancelled || !ok {
            plan.mark_remaining_skipped();
            // Leave other in-flight jobs to finish naturally; they won't re-enter
            // cascade once plan is cleared.
            self.plan = None;
            let toast = if cancelled {
                format!("cascade cancelled at {job_name} (step {}/{n})", step_idx + 1)
            } else {
                format!(
                    "cascade failed at {job_name} ({kind_label}, step {}/{n}) — {log_tail}",
                    step_idx + 1
                )
            };
            self.set_ephemeral_status(toast);
            return Some(vec![]);
        }

        // Fill free slots with newly unblocked work (e.g. more consumers).
        Some(self.dispatch_cascade_ready())
    }

    /// Elm-style reducer. Stays synchronous; background I/O is returned as
    /// `Command`s for main to spawn.
    pub fn update(&mut self, action: Action) -> Vec<Command> {
        match action {
            Action::ToggleHelp => {
                self.help_visible = !self.help_visible;
                vec![]
            }
            Action::Quit => {
                self.help_visible = false;
                self.quit_confirm = true;
                vec![]
            }
            Action::ForceQuit => {
                self.should_quit = true;
                vec![]
            }
            Action::ConfirmQuit => {
                self.quit_confirm = false;
                self.should_quit = true;
                vec![]
            }
            Action::CancelQuit => {
                self.quit_confirm = false;
                vec![]
            }
            Action::MoveSelectionUp => {
                if self.plan_confirming() {
                    if let Some(p) = self.plan.as_mut() {
                        p.move_cursor(-1);
                    }
                    return vec![];
                }
                if self.project_detail_visible {
                    self.project_detail_scroll = self.project_detail_scroll.saturating_sub(1);
                    return vec![];
                }
                if self.screen == Screen::Settings && self.settings_editor.is_none() {
                    self.move_settings_selection(-1);
                } else if self.screen == Screen::Workspace && self.root_editor.is_none() {
                    match self.workspace_panel {
                        WorkspacePanel::Roots => self.move_root_selection(-1),
                        WorkspacePanel::Excludes => self.move_exclude_selection(-1),
                    }
                } else {
                    self.move_selection(-1);
                }
                vec![]
            }
            Action::MoveSelectionDown => {
                if self.plan_confirming() {
                    if let Some(p) = self.plan.as_mut() {
                        p.move_cursor(1);
                    }
                    return vec![];
                }
                if self.project_detail_visible {
                    self.project_detail_scroll = self.project_detail_scroll.saturating_add(1);
                    return vec![];
                }
                if self.screen == Screen::Settings && self.settings_editor.is_none() {
                    self.move_settings_selection(1);
                } else if self.screen == Screen::Workspace && self.root_editor.is_none() {
                    match self.workspace_panel {
                        WorkspacePanel::Roots => self.move_root_selection(1),
                        WorkspacePanel::Excludes => self.move_exclude_selection(1),
                    }
                } else {
                    self.move_selection(1);
                }
                vec![]
            }
            Action::SelectRow(index) => {
                if self.plan_confirming() {
                    return vec![];
                }
                if self.screen == Screen::Workspace {
                    if index < self.config.scan.roots.len() {
                        self.workspace_panel = WorkspacePanel::Roots;
                        self.selected_root_index = index;
                    }
                } else {
                    self.set_selection(index);
                }
                vec![]
            }
            Action::SelectExcludeRow(index) => {
                if self.screen == Screen::Workspace && index < self.config.scan.exclude.len() {
                    self.workspace_panel = WorkspacePanel::Excludes;
                    self.selected_exclude_index = index;
                }
                vec![]
            }
            Action::Confirm => {
                if self.plan_confirming() {
                    return self.confirm_cascade_plan();
                }
                // Root delete dialog: Enter confirms (also via ConfirmDeleteRoot).
                if self.root_delete_confirm.is_some() {
                    return self.confirm_delete_root();
                }
                if self.exclude_delete_confirm.is_some() {
                    return self.confirm_delete_exclude();
                }
                if self.screen == Screen::Settings && self.settings_editor.is_none() {
                    return self.activate_setting();
                }
                // Workspace: Enter jumps to project browser (when not editing).
                if self.screen == Screen::Workspace && self.root_editor.is_none() {
                    self.screen = Screen::ProjectBrowser;
                    return vec![];
                }
                // Project browser: open/close full-screen project detail.
                if self.screen == Screen::ProjectBrowser && !self.projects.is_empty() {
                    if self.project_detail_visible {
                        self.close_project_detail();
                    } else {
                        self.open_project_detail();
                    }
                }
                vec![]
            }
            Action::Back => {
                if self.plan_confirming() {
                    self.cancel_cascade_plan();
                    return vec![];
                }
                // Clear multi-select first on the browser (before detail / help / other Esc).
                if self.screen == Screen::ProjectBrowser
                    && !self.project_detail_visible
                    && !self.multi_selected.is_empty()
                {
                    self.multi_selected.clear();
                    return vec![];
                }
                if self.project_detail_visible {
                    self.close_project_detail();
                    return vec![];
                }
                if self.help_visible {
                    self.help_visible = false;
                    return vec![];
                }
                if self.root_delete_confirm.is_some() {
                    self.cancel_delete_root();
                    return vec![];
                }
                if self.exclude_delete_confirm.is_some() {
                    self.cancel_delete_exclude();
                    return vec![];
                }
                if self.settings_editor.is_some() {
                    self.cancel_settings_editor();
                    return vec![];
                }
                if self.root_editor.is_some() {
                    self.cancel_root_editor();
                    return vec![];
                }
                // Esc on Jobs: cancel focused running job when possible.
                if self.screen == Screen::Jobs {
                    let active = self
                        .jobs
                        .get(self.selected_job)
                        .is_some_and(|j| j.status.is_active());
                    if active {
                        return self.cancel_focused_job();
                    }
                    self.screen = Screen::ProjectBrowser;
                    return vec![];
                }
                if self.screen == Screen::Workspace {
                    self.screen = Screen::ProjectBrowser;
                }
                vec![]
            }
            Action::Refresh => {
                if self.config.scan.roots.is_empty() {
                    self.set_ephemeral_status("no scan roots configured");
                    return vec![];
                }
                self.begin_scan_status();
                vec![Command::ScanWorkspace]
            }
            Action::BannerTick => {
                if self.banner_mode != BannerMode::Off {
                    self.banner_frame = self.banner_frame.wrapping_add(1);
                }
                vec![]
            }
            Action::CycleBannerMode => {
                self.banner_mode = self.banner_mode.next();
                if self.banner_mode == BannerMode::Off {
                    self.banner_frame = 0;
                }
                self.persist_ui_prefs();
                // Mode is visible on the banner itself — no status toast.
                vec![]
            }
            Action::CycleTheme => {
                let next = self.theme.name.next();
                self.theme = Theme::from_name(next);
                self.persist_ui_prefs();
                // Theme is visible on chrome — no toast (SPEC / rakko 0.13.2 lesson).
                vec![]
            }
            Action::DismissSplash => {
                self.show_splash = false;
                vec![]
            }
            Action::SwitchToProjects => {
                self.root_editor = None;
                self.root_delete_confirm = None;
                self.settings_editor = None;
                self.screen = Screen::ProjectBrowser;
                vec![]
            }
            Action::SwitchToJobs => {
                self.root_editor = None;
                self.root_delete_confirm = None;
                self.settings_editor = None;
                self.screen = Screen::Jobs;
                vec![]
            }
            Action::SwitchToWorkspace => {
                self.settings_editor = None;
                self.screen = Screen::Workspace;
                self.clamp_root_selection();
                vec![]
            }
            Action::SwitchToSettings => {
                self.screen = Screen::Settings;
                self.settings_editor = None;
                self.clamp_settings_selection();
                vec![]
            }
            Action::ActivateSetting => {
                if self.screen == Screen::Settings && self.settings_editor.is_none() {
                    return self.activate_setting();
                }
                vec![]
            }
            Action::NudgeSettingLeft => {
                if self.screen == Screen::Settings && self.settings_editor.is_none() {
                    return self.nudge_setting_int(-1);
                }
                vec![]
            }
            Action::NudgeSettingRight => {
                if self.screen == Screen::Settings && self.settings_editor.is_none() {
                    return self.nudge_setting_int(1);
                }
                vec![]
            }
            Action::SelectSettingRow(i) => {
                if self.screen == Screen::Settings && i < SettingId::ALL.len() {
                    self.selected_setting = i;
                }
                vec![]
            }
            Action::SaveSettingsEditor => self.save_settings_editor(),
            Action::CancelSettingsEditor => {
                self.cancel_settings_editor();
                vec![]
            }
            Action::SettingsEditorChar(c) => {
                if let Some(ed) = self.settings_editor.as_mut() {
                    ed.insert_char(c);
                }
                vec![]
            }
            Action::SettingsEditorBackspace => {
                if let Some(ed) = self.settings_editor.as_mut() {
                    ed.backspace();
                }
                vec![]
            }
            Action::SettingsEditorDelete => {
                if let Some(ed) = self.settings_editor.as_mut() {
                    ed.delete_forward();
                }
                vec![]
            }
            Action::SettingsEditorCursorLeft => {
                if let Some(ed) = self.settings_editor.as_mut() {
                    ed.cursor_left();
                }
                vec![]
            }
            Action::SettingsEditorCursorRight => {
                if let Some(ed) = self.settings_editor.as_mut() {
                    ed.cursor_right();
                }
                vec![]
            }
            Action::SettingsEditorCursorHome => {
                if let Some(ed) = self.settings_editor.as_mut() {
                    ed.cursor_home();
                }
                vec![]
            }
            Action::SettingsEditorCursorEnd => {
                if let Some(ed) = self.settings_editor.as_mut() {
                    ed.cursor_end();
                }
                vec![]
            }
            Action::StartAddRoot => {
                if self.screen == Screen::Workspace {
                    match self.workspace_panel {
                        WorkspacePanel::Roots => self.start_add_root(),
                        WorkspacePanel::Excludes => self.start_add_exclude(),
                    }
                }
                vec![]
            }
            Action::StartEditRoot => {
                if self.screen == Screen::Workspace {
                    match self.workspace_panel {
                        WorkspacePanel::Roots => self.start_edit_root(),
                        WorkspacePanel::Excludes => self.start_edit_exclude(),
                    }
                }
                vec![]
            }
            Action::StartDeleteRoot => {
                if self.screen == Screen::Workspace {
                    match self.workspace_panel {
                        WorkspacePanel::Roots => self.start_delete_root(),
                        WorkspacePanel::Excludes => self.start_delete_exclude(),
                    }
                }
                vec![]
            }
            Action::ConfirmDeleteRoot => self.confirm_delete_root(),
            Action::CancelDeleteRoot => {
                self.cancel_delete_root();
                vec![]
            }
            Action::StartAddExclude => {
                if self.screen == Screen::Workspace {
                    self.start_add_exclude();
                }
                vec![]
            }
            Action::StartEditExclude => {
                if self.screen == Screen::Workspace {
                    self.start_edit_exclude();
                }
                vec![]
            }
            Action::StartDeleteExclude => {
                if self.screen == Screen::Workspace {
                    self.start_delete_exclude();
                }
                vec![]
            }
            Action::ConfirmDeleteExclude => self.confirm_delete_exclude(),
            Action::CancelDeleteExclude => {
                self.cancel_delete_exclude();
                vec![]
            }
            Action::ToggleWorkspacePanel => {
                if self.screen == Screen::Workspace
                    && self.root_editor.is_none()
                    && self.root_delete_confirm.is_none()
                    && self.exclude_delete_confirm.is_none()
                {
                    self.toggle_workspace_panel();
                }
                vec![]
            }
            Action::CancelRootEditor => {
                self.cancel_root_editor();
                vec![]
            }
            Action::RootEditorChar(c) => {
                if let Some(state) = self.root_editor.as_mut() {
                    state.insert_char(c);
                }
                vec![]
            }
            Action::RootEditorBackspace => {
                if let Some(state) = self.root_editor.as_mut() {
                    state.backspace();
                }
                vec![]
            }
            Action::RootEditorDelete => {
                if let Some(state) = self.root_editor.as_mut() {
                    state.delete_forward();
                }
                vec![]
            }
            Action::RootEditorCursorLeft => {
                if let Some(state) = self.root_editor.as_mut() {
                    state.cursor_left();
                }
                vec![]
            }
            Action::RootEditorCursorRight => {
                if let Some(state) = self.root_editor.as_mut() {
                    state.cursor_right();
                }
                vec![]
            }
            Action::RootEditorCursorHome => {
                if let Some(state) = self.root_editor.as_mut() {
                    state.cursor_home();
                }
                vec![]
            }
            Action::RootEditorCursorEnd => {
                if let Some(state) = self.root_editor.as_mut() {
                    state.cursor_end();
                }
                vec![]
            }
            Action::SaveRoot => self.save_root(),

            // ── M3/M5 project exec (single or multi-select bulk for b/c/G) ──
            Action::Build => {
                let tasks = self.config.gradle.default_tasks_build.clone();
                self.start_gradle(tasks)
            }
            Action::BuildWithDeps => self.start_build_with_deps(),
            Action::Clean => {
                let tasks = self.config.gradle.default_tasks_clean.clone();
                self.start_gradle(tasks)
            }
            Action::Publish => {
                // Publish stays single-project (cursor); cascade is a separate action.
                let tasks = self.config.gradle.default_tasks_publish.clone();
                self.start_gradle_single(tasks)
            }
            Action::SkaffoldDev => {
                let extra = self.config.skaffold.dev_args.clone();
                self.start_skaffold("dev", extra)
            }
            Action::SkaffoldDebug => {
                let extra = self.config.skaffold.debug_args.clone();
                self.start_skaffold("debug", extra)
            }
            Action::SkaffoldDelete => self.start_skaffold("delete", vec![]),
            Action::SkaffoldRun => self.start_skaffold("run", vec![]),
            Action::GitPull => self.start_git_pull(),
            Action::ToggleMultiSelect => {
                if self.screen == Screen::ProjectBrowser {
                    self.toggle_multi_select();
                }
                vec![]
            }
            Action::CancelJob => self.cancel_focused_job(),
            Action::ToggleJobsFocus => {
                if self.screen == Screen::Jobs {
                    self.jobs_focus = match self.jobs_focus {
                        JobsFocus::List => JobsFocus::Log,
                        JobsFocus::Log => JobsFocus::List,
                    };
                }
                vec![]
            }
            // M4 cascade — open/confirm/cancel plan.
            Action::CascadePublish => self.open_cascade_publish(),
            Action::ConfirmCascadePlan => self.confirm_cascade_plan(),
            Action::CancelCascadePlan => {
                self.cancel_cascade_plan();
                vec![]
            }
            // Phase 2 kube.
            Action::RefreshDeployedVersions => self.start_kube_probe(),
            Action::ToggleDriftFilter => {
                self.filter_drift_only = !self.filter_drift_only;
                let msg = if self.filter_drift_only {
                    "filter: drift only"
                } else {
                    "filter: all projects"
                };
                self.set_ephemeral_status(msg);
                vec![]
            }
            Action::ExcludeSelectedProjects => self.exclude_selected_projects(),
        }
    }

    /// Add cursor / multi-selected projects to `[scan].exclude`, save, rescan.
    fn exclude_selected_projects(&mut self) -> Vec<Command> {
        if self.screen != Screen::ProjectBrowser {
            return vec![];
        }
        let targets = self.action_target_indices();
        if targets.is_empty() {
            self.set_ephemeral_status("no project selected");
            return vec![];
        }

        let mut added: Vec<String> = Vec::new();
        for idx in targets {
            let Some(p) = self.projects.get(idx) else {
                continue;
            };
            let pat = crate::kube::exclude_pattern_for(p);
            if self.config.scan.exclude.iter().any(|e| e == &pat) {
                continue;
            }
            self.config.scan.exclude.push(pat.clone());
            added.push(pat);
        }

        if added.is_empty() {
            self.set_ephemeral_status("already excluded (or nothing to add)");
            return vec![];
        }

        if let Err(err) = crate::config::save(&self.config_path, &self.config) {
            // Roll back patterns we just pushed.
            self.config
                .scan
                .exclude
                .retain(|e| !added.iter().any(|a| a == e));
            self.set_ephemeral_status(format!("failed to save exclude: {err}"));
            return vec![];
        }

        self.multi_selected.clear();
        self.project_detail_visible = false;
        let msg = if added.len() == 1 {
            format!("excluded '{}' — rescanning…", added[0])
        } else {
            format!("excluded {} projects — rescanning…", added.len())
        };
        self.set_ephemeral_status(msg);
        vec![Command::ScanWorkspace]
    }

    /// Kick off a background kubectl probe for deployed versions.
    fn start_kube_probe(&mut self) -> Vec<Command> {
        if self.kube_probing {
            self.set_ephemeral_status("kube probe already running");
            return vec![];
        }
        if !self.config.kube.enabled {
            self.set_ephemeral_status(
                "kube disabled — set [kube] enabled=true (and namespaces) in config",
            );
            return vec![];
        }
        if self.config.kube.namespaces.is_empty() {
            self.set_ephemeral_status("kube: set [kube] namespaces = [\"dev\", …]");
            return vec![];
        }
        self.kube_probing = true;
        self.status_message = Some(format!(
            "kube probe… ns={} src={}",
            self.config.kube.namespaces.join(","),
            self.config.kube.version_source
        ));
        self.status_clear_at = None;
        vec![Command::ProbeKubeVersions]
    }

    /// Apply a background-task result.
    pub fn apply_event(&mut self, event: AppEvent) -> Vec<Command> {
        match event {
            AppEvent::ScanStarted => {
                self.begin_scan_status();
                vec![]
            }
            AppEvent::ScanFinished { projects, error } => {
                self.scanning = false;
                if let Some(err) = error {
                    // Keep previous inventory on failure; surface toast.
                    self.set_ephemeral_status(format!("scan failed: {err}"));
                    return vec![];
                }
                self.apply_scan_projects(&projects);
                self.last_scan_at = Some(SystemTime::now());
                self.persist_workspace_cache();
                let n = self.projects.len();
                let n_edges: usize = projects.iter().map(|p| p.depends.len()).sum();
                self.set_ephemeral_status(format!(
                    "scan complete: {n} projects ({n_edges} deps)"
                ));
                // Auto-probe when kube is configured.
                if crate::kube::kube_probe_enabled(&self.config.kube) && !self.kube_probing {
                    self.kube_probing = true;
                    return vec![Command::ProbeKubeVersions];
                }
                vec![]
            }
            AppEvent::KubeProbeFinished { batch } => {
                self.kube_probing = false;
                if let Some(err) = &batch.error {
                    self.set_ephemeral_status(format!("kube probe failed: {err}"));
                    // Leave rows as NotProbed when the whole batch failed.
                    return vec![];
                }
                crate::kube::apply_probes(&mut self.projects, &batch);
                self.kube_probed_at = Some(Instant::now());
                let n_found = batch
                    .probes
                    .iter()
                    .filter(|p| p.deployed_version.is_some())
                    .count();
                let n_miss = batch
                    .probes
                    .iter()
                    .filter(|p| p.deployed_version.is_none())
                    .count();
                let n_drift = self
                    .projects
                    .iter()
                    .filter(|p| matches!(p.drift, Drift::LocalAhead | Drift::ClusterAhead))
                    .count();
                let hint = batch
                    .probes
                    .iter()
                    .find(|p| p.error.is_some() && p.deployed_version.is_none())
                    .and_then(|p| p.error.as_deref())
                    .unwrap_or("");
                let msg = if n_found == 0 && n_miss > 0 {
                    if hint.is_empty() {
                        format!("kube probe: 0 deployed / {n_miss} unknown — check namespaces & Deployment names")
                    } else {
                        format!("kube probe: 0 deployed / {n_miss} unknown — {hint}")
                    }
                } else {
                    format!(
                        "kube probe: {n_found} deployed · {n_miss} unknown · {n_drift} drift"
                    )
                };
                self.set_ephemeral_status(msg);
                vec![]
            }
            AppEvent::ScanProgress { message } => {
                // Sticky progress under the same "scanning…" family (not a toast).
                if self.scanning {
                    self.status_message = Some(message);
                    self.status_clear_at = None;
                }
                vec![]
            }
            AppEvent::JobStarted { id } => {
                let chip = if let Some(job) = self.find_job_mut(id) {
                    job.mark_running();
                    Some((job.project_path.clone(), job.kind.running_chip()))
                } else {
                    None
                };
                if let Some((path, chip)) = chip {
                    self.set_project_status_by_path(&path, chip);
                }
                vec![]
            }
            AppEvent::JobLog { id, line } => {
                if let Some(job) = self.find_job_mut(id) {
                    job.push_log(line);
                }
                vec![]
            }
            AppEvent::JobFinished {
                id,
                ok,
                cancelled,
                summary,
            } => {
                if let Some(job) = self.find_job_mut(id) {
                    job.mark_finished(ok, cancelled, summary.clone());
                    let name = job.project_name.clone();
                    let kind_label = job.kind.label();
                    let path = job.project_path.clone();
                    let git_root = job.git_root.clone();
                    let is_pull = matches!(job.kind, JobKind::GitPull);
                    // Prefer last log line (stderr often ends there) on failure.
                    let log_tail = job
                        .log
                        .last()
                        .cloned()
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| summary.clone());
                    let chip = match job.status {
                        JobStatus::Pending | JobStatus::Running => job.kind.running_chip(),
                        JobStatus::Ok => job.kind.ok_chip().to_string(),
                        JobStatus::Failed => job.kind.fail_chip().to_string(),
                        JobStatus::Cancelled => "cancelled".into(),
                    };
                    self.set_project_status_by_path(&path, &chip);
                    // Fan-out pull status to every row sharing the git root.
                    if is_pull {
                        if let Some(root) = git_root {
                            for p in &mut self.projects {
                                if p.git_root.as_ref() == Some(&root) {
                                    p.status = chip.clone();
                                }
                            }
                        }
                    }
                    // Cascade sequencing: on success start next step; on fail stop.
                    if let Some(cmds) = self.advance_cascade_on_job_finished(
                        id,
                        ok,
                        cancelled,
                        &name,
                        &kind_label,
                        &log_tail,
                    ) {
                        return cmds;
                    }

                    let toast = if cancelled {
                        format!("cancelled: {name} ({kind_label})")
                    } else if ok {
                        format!("{kind_label} ok: {name}")
                    } else {
                        format!("{kind_label} failed: {name} — {log_tail}")
                    };
                    self.set_ephemeral_status(toast);
                }
                vec![]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BannerMode, Config, ScanConfig, UiConfig};
    use crate::ui::theme::ThemeName;
    use std::path::PathBuf;

    fn test_config_path() -> PathBuf {
        // Unique *directory* per call so workspace cache (`…/cache/workspace.json`)
        // never collides across parallel tests (parent must not be shared temp_dir).
        let dir = std::env::temp_dir().join(format!(
            "tako-app-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::create_dir_all(&dir);
        dir.join("config.toml")
    }

    fn sample_project(name: &str) -> ProjectRow {
        ProjectRow::new(
            name,
            format!("/tmp/{name}"),
            ProjectKind::Service,
            "1.0",
            "main",
        )
    }

    fn discovered(
        name: &str,
        kind: ProjectKind,
        version: &str,
        branch: &str,
        has_skaffold: bool,
        git_dirty: bool,
    ) -> DiscoveredProject {
        DiscoveredProject {
            id: name.into(),
            path: PathBuf::from(format!("/tmp/{name}")),
            git_root: Some(PathBuf::from(format!("/tmp/{name}"))),
            name: name.into(),
            kind,
            version: version.into(),
            branch: branch.into(),
            git_dirty,
            has_skaffold,
            skaffold_files: if has_skaffold {
                vec![PathBuf::from(format!("/tmp/{name}/skaffold.yaml"))]
            } else {
                vec![]
            },
            gradle_root: Some(PathBuf::from(format!("/tmp/{name}"))),
            folder_group: name.into(),
            status: "idle".into(),
            depends: vec![],
            produces: vec![],
        }
    }

    #[test]
    fn new_app_starts_on_project_browser_with_splash() {
        let app = App::new(Config::default(), test_config_path());
        assert_eq!(app.screen, Screen::ProjectBrowser);
        assert!(app.show_splash);
        assert!(app.projects.is_empty());
        assert!(!app.should_quit);
        assert!(!app.quit_confirm);
    }

    #[test]
    fn dismiss_splash() {
        let mut app = App::new(Config::default(), test_config_path());
        assert!(app.show_splash);
        app.update(Action::DismissSplash);
        assert!(!app.show_splash);
    }

    #[test]
    fn quit_confirm_and_confirm_quit() {
        let mut app = App::new(Config::default(), test_config_path());
        app.update(Action::Quit);
        assert!(app.quit_confirm);
        assert!(!app.should_quit);
        app.update(Action::ConfirmQuit);
        assert!(app.should_quit);
        assert!(!app.quit_confirm);
    }

    #[test]
    fn cancel_quit() {
        let mut app = App::new(Config::default(), test_config_path());
        app.update(Action::Quit);
        app.update(Action::CancelQuit);
        assert!(!app.quit_confirm);
        assert!(!app.should_quit);
    }

    #[test]
    fn force_quit() {
        let mut app = App::new(Config::default(), test_config_path());
        app.update(Action::ForceQuit);
        assert!(app.should_quit);
    }

    #[test]
    fn cycle_banner_mode_goes_wave_ms_fps_off_and_back() {
        let path = test_config_path();
        let mut app = App::new(Config::default(), path.clone());
        assert_eq!(app.banner_mode, BannerMode::Wave);
        app.update(Action::BannerTick);
        assert_eq!(app.banner_frame, 1);

        app.update(Action::CycleBannerMode);
        assert_eq!(app.banner_mode, BannerMode::Ms);
        assert!(app.status_message.is_none(), "banner cycle should not toast");

        app.update(Action::CycleBannerMode);
        assert_eq!(app.banner_mode, BannerMode::Fps);

        app.update(Action::CycleBannerMode);
        assert_eq!(app.banner_mode, BannerMode::Off);
        app.update(Action::BannerTick);
        assert_eq!(app.banner_frame, 0);

        app.update(Action::CycleBannerMode);
        assert_eq!(app.banner_mode, BannerMode::Wave);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn cycle_theme_and_toggle_help() {
        let path = test_config_path();
        let mut app = App::new(Config::default(), path.clone());
        assert_eq!(app.theme.name, ThemeName::Dark);
        assert!(!app.help_visible);

        app.update(Action::CycleTheme);
        assert_eq!(app.theme.name, ThemeName::Light);
        assert_eq!(app.config.ui.theme, ThemeName::Light);
        assert!(
            app.status_message.is_none(),
            "theme cycle should not toast: {:?}",
            app.status_message
        );

        app.update(Action::ToggleHelp);
        assert!(app.help_visible);
        app.update(Action::ToggleHelp);
        assert!(!app.help_visible);

        let loaded = crate::config::load(&path).expect("load after cycle");
        assert_eq!(loaded.ui.theme, ThemeName::Light);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn ui_prefs_load_from_config() {
        let config = Config {
            ui: UiConfig {
                theme: ThemeName::Light,
                banner_mode: BannerMode::Off,
            },
            ..Default::default()
        };
        let app = App::new(config, test_config_path());
        assert_eq!(app.theme.name, ThemeName::Light);
        assert_eq!(app.banner_mode, BannerMode::Off);
    }

    #[test]
    fn selection_moves_and_clamps() {
        let mut app = App::new(Config::default(), test_config_path());
        app.projects = vec![sample_project("a"), sample_project("b")];
        app.update(Action::MoveSelectionDown);
        assert_eq!(app.selected_index, 1);
        app.update(Action::MoveSelectionDown);
        assert_eq!(app.selected_index, 1);
        app.update(Action::MoveSelectionUp);
        assert_eq!(app.selected_index, 0);
        app.update(Action::SelectRow(1));
        assert_eq!(app.selected_index, 1);
    }

    #[test]
    fn ephemeral_status_expires() {
        let mut app = App::new(Config::default(), test_config_path());
        app.set_ephemeral_status("scan complete: 3 projects");
        assert!(app.status_message.is_some());
        assert!(app.status_clear_at.is_some());
        // Force deadline into the past.
        app.status_clear_at = Some(Instant::now() - Duration::from_millis(1));
        assert!(app.expire_status_if_due());
        assert!(app.status_message.is_none());
        assert!(app.status_clear_at.is_none());
    }

    #[test]
    fn scanning_status_is_not_ephemeral() {
        assert!(!is_ephemeral_status_text("scanning…"));
        assert!(is_ephemeral_status_text("scan complete: 2 projects"));
        assert!(is_ephemeral_status_text("no scan roots configured"));
    }

    #[test]
    fn click_regions_and_hover() {
        let app = App::new(Config::default(), test_config_path());
        app.register_click(0, 0, 10, 1, Action::SelectRow(0));
        app.register_click(0, 1, 10, 1, Action::SelectRow(1));
        assert_eq!(app.action_at(3, 0), Some(Action::SelectRow(0)));
        assert_eq!(app.action_at(3, 1), Some(Action::SelectRow(1)));
        assert_eq!(app.action_at(99, 99), None);

        app.set_mouse_pos(3, 0);
        assert!(app.is_hovered(0, 0, 10, 1));
        assert!(!app.is_hovered(0, 1, 10, 1));

        app.clear_click_regions();
        assert_eq!(app.action_at(3, 0), None);
    }

    #[test]
    fn frame_ms_samples_record() {
        let mut app = App::new(Config::default(), test_config_path());
        app.push_frame_ms_sample(1.5);
        app.push_frame_ms_sample(2.5);
        assert_eq!(app.frame_ms_samples.len(), 2);
        assert_eq!(app.frame_ms_samples.last().copied(), Some(2.5));
    }

    #[test]
    fn refresh_with_empty_roots_sets_status() {
        let mut app = App::new(Config::default(), test_config_path());
        assert!(app.config.scan.roots.is_empty());
        let cmds = app.update(Action::Refresh);
        assert!(cmds.is_empty());
        assert_eq!(
            app.status_message.as_deref(),
            Some("no scan roots configured")
        );
        assert!(app.status_clear_at.is_some(), "empty-roots toast is ephemeral");
    }

    #[test]
    fn refresh_with_roots_emits_scan_command() {
        let mut app = App::new(
            Config {
                scan: ScanConfig {
                    roots: vec!["/tmp/services".into()],
                    ..Default::default()
                },
                ..Default::default()
            },
            test_config_path(),
        );
        let cmds = app.update(Action::Refresh);
        assert!(matches!(cmds.as_slice(), [Command::ScanWorkspace]));
        assert_eq!(app.status_message.as_deref(), Some("scanning…"));
        assert!(app.status_clear_at.is_none(), "scanning… is sticky");
        assert!(app.scanning);
    }

    #[test]
    fn apply_event_scan_finished_populates_projects() {
        let path = test_config_path();
        let mut app = App::new(Config::default(), path.clone());
        app.selected_index = 99; // will clamp

        let found = vec![
            discovered(
                "payments-api",
                ProjectKind::Service,
                "1.4.2",
                "main",
                true,
                false,
            ),
            discovered(
                "common-lib",
                ProjectKind::Library,
                "0.1.0",
                "dev",
                false,
                true,
            ),
        ];

        let cmds = app.apply_event(AppEvent::ScanFinished {
            projects: found,
            error: None,
        });
        assert!(cmds.is_empty());
        assert_eq!(app.projects.len(), 2);
        assert_eq!(app.projects[0].name, "payments-api");
        assert_eq!(app.projects[0].kind, ProjectKind::Service);
        assert_eq!(app.projects[0].version, "1.4.2");
        assert!(app.projects[0].skaffold_path.is_some());
        assert_eq!(app.projects[1].name, "common-lib");
        assert_eq!(app.projects[1].kind, ProjectKind::Library);
        assert!(app.projects[1].git_dirty);
        assert_eq!(app.selected_index, 1, "selection clamped to last row");
        assert!(
            app.status_message
                .as_deref()
                .is_some_and(|s| s.contains("scan complete") && s.contains('2')),
            "toast: {:?}",
            app.status_message
        );
        assert!(!app.scanning);
        assert!(app.last_scan_at.is_some());

        assert!(
            app.cache_path.exists(),
            "cache written to {}",
            app.cache_path.display()
        );
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&app.cache_path);
        if let Some(parent) = app.cache_path.parent() {
            let _ = std::fs::remove_dir(parent);
        }
    }

    #[test]
    fn apply_event_scan_finished_error_keeps_inventory() {
        let mut app = App::new(Config::default(), test_config_path());
        app.projects = vec![sample_project("kept")];
        app.scanning = true;
        app.apply_event(AppEvent::ScanFinished {
            projects: vec![],
            error: Some("root does not exist: /nope".into()),
        });
        assert_eq!(app.projects.len(), 1);
        assert_eq!(app.projects[0].name, "kept");
        assert!(
            app.status_message
                .as_deref()
                .is_some_and(|s| s.starts_with("scan failed")),
            "{:?}",
            app.status_message
        );
        assert!(!app.scanning);
    }

    #[test]
    fn startup_commands_scan_when_roots_set() {
        let mut app = App::new(
            Config {
                scan: ScanConfig {
                    roots: vec!["/tmp".into()],
                    ..Default::default()
                },
                ..Default::default()
            },
            test_config_path(),
        );
        let cmds = app.startup_commands();
        assert!(matches!(cmds.as_slice(), [Command::ScanWorkspace]));
    }

    #[test]
    fn startup_commands_empty_when_no_roots() {
        let mut app = App::new(Config::default(), test_config_path());
        assert!(app.startup_commands().is_empty());
    }

    #[test]
    fn loads_projects_from_workspace_cache() {
        let config_path = test_config_path();
        let cache_path = config::workspace_cache_path_in(config_path.parent().unwrap());
        let projects = vec![discovered(
            "cached-svc",
            ProjectKind::Service,
            "9.9.9",
            "main",
            true,
            false,
        )
        .to_row()];
        scan::save_workspace_cache(&cache_path, &projects).expect("seed cache");

        let app = App::new(Config::default(), config_path.clone());
        assert_eq!(app.projects.len(), 1);
        assert_eq!(app.projects[0].name, "cached-svc");
        assert_eq!(app.projects[0].version, "9.9.9");
        assert!(app.last_scan_at.is_some());

        let _ = std::fs::remove_file(&config_path);
        let _ = std::fs::remove_file(&cache_path);
        if let Some(parent) = cache_path.parent() {
            let _ = std::fs::remove_dir(parent);
        }
    }

    #[test]
    fn confirm_on_workspace_opens_projects() {
        let mut app = App::new(Config::default(), test_config_path());
        app.screen = Screen::Workspace;
        app.update(Action::Confirm);
        assert_eq!(app.screen, Screen::ProjectBrowser);
    }

    #[test]
    fn project_display_order_sorts_by_group() {
        let mut a = sample_project("zeta");
        a.folder_group = Some("b".into());
        let mut b = sample_project("alpha");
        b.folder_group = Some("a".into());
        let order = project_display_order(&[a, b]);
        assert_eq!(order, vec![1, 0]);
    }

    // —— M1.5 workspace roots ——

    #[test]
    fn add_root_expands_tilde_and_saves() {
        let path = test_config_path();
        let mut app = App::new(Config::default(), path.clone());
        app.screen = Screen::Workspace;
        app.show_splash = false;

        app.update(Action::StartAddRoot);
        assert!(app.root_editor.is_some());

        for c in "~/Developer".chars() {
            app.update(Action::RootEditorChar(c));
        }
        let cmds = app.update(Action::SaveRoot);
        assert!(app.root_editor.is_none());
        assert_eq!(app.config.scan.roots.len(), 1);

        let home = dirs::home_dir().expect("home");
        let expected = home.join("Developer");
        assert_eq!(
            PathBuf::from(&app.config.scan.roots[0]),
            expected,
            "saved root should expand ~/"
        );
        assert!(
            matches!(cmds.as_slice(), [Command::ScanWorkspace]),
            "non-empty roots should rescan: {cmds:?}"
        );
        assert!(
            app.status_message
                .as_deref()
                .is_some_and(|s| s.starts_with("added root")),
            "{:?}",
            app.status_message
        );

        let loaded = crate::config::load(&path).expect("reload config");
        assert_eq!(loaded.scan.roots, app.config.scan.roots);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn save_root_empty_path_sets_error() {
        let mut app = App::new(Config::default(), test_config_path());
        app.screen = Screen::Workspace;
        app.update(Action::StartAddRoot);
        let cmds = app.update(Action::SaveRoot);
        assert!(cmds.is_empty());
        assert!(app.root_editor.is_some());
        assert_eq!(
            app.root_editor.as_ref().and_then(|s| s.error.as_deref()),
            Some("path is required")
        );
        assert!(app.config.scan.roots.is_empty());
    }

    #[test]
    fn save_root_relative_path_sets_error() {
        let mut app = App::new(Config::default(), test_config_path());
        app.screen = Screen::Workspace;
        app.update(Action::StartAddRoot);
        for c in "relative/path".chars() {
            app.update(Action::RootEditorChar(c));
        }
        app.update(Action::SaveRoot);
        let err = app
            .root_editor
            .as_ref()
            .and_then(|s| s.error.clone())
            .unwrap_or_default();
        assert!(
            err.contains("absolute") || err.contains("~/"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn delete_root_removes_and_saves() {
        let path = test_config_path();
        let mut app = App::new(
            Config {
                scan: ScanConfig {
                    roots: vec!["/tmp/a".into(), "/tmp/b".into()],
                    ..Default::default()
                },
                ..Default::default()
            },
            path.clone(),
        );
        // Persist initial so delete's save is meaningful.
        crate::config::save(&path, &app.config).expect("seed");
        app.screen = Screen::Workspace;
        app.selected_root_index = 0;

        app.update(Action::StartDeleteRoot);
        assert_eq!(app.root_delete_confirm, Some(0));
        let cmds = app.update(Action::ConfirmDeleteRoot);
        assert!(app.root_delete_confirm.is_none());
        assert_eq!(app.config.scan.roots, vec!["/tmp/b".to_string()]);
        assert!(
            matches!(cmds.as_slice(), [Command::ScanWorkspace]),
            "remaining roots should rescan"
        );

        let loaded = crate::config::load(&path).expect("reload");
        assert_eq!(loaded.scan.roots, vec!["/tmp/b".to_string()]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn delete_last_root_clears_without_scan() {
        let path = test_config_path();
        let mut app = App::new(
            Config {
                scan: ScanConfig {
                    roots: vec!["/tmp/only".into()],
                    ..Default::default()
                },
                ..Default::default()
            },
            path.clone(),
        );
        crate::config::save(&path, &app.config).expect("seed");
        app.projects = vec![sample_project("x")];
        app.screen = Screen::Workspace;

        app.update(Action::StartDeleteRoot);
        let cmds = app.update(Action::ConfirmDeleteRoot);
        assert!(cmds.is_empty(), "no roots → no scan");
        assert!(app.config.scan.roots.is_empty());
        assert!(app.projects.is_empty(), "inventory cleared when roots empty");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn edit_root_replaces_path() {
        let path = test_config_path();
        let mut app = App::new(
            Config {
                scan: ScanConfig {
                    roots: vec!["/tmp/old".into()],
                    ..Default::default()
                },
                ..Default::default()
            },
            path.clone(),
        );
        crate::config::save(&path, &app.config).expect("seed");
        app.screen = Screen::Workspace;
        app.selected_root_index = 0;

        app.update(Action::StartEditRoot);
        // Clear field and type new path.
        if let Some(s) = app.root_editor.as_mut() {
            s.path.clear();
            s.cursor = 0;
        }
        for c in "/tmp/new".chars() {
            app.update(Action::RootEditorChar(c));
        }
        let cmds = app.update(Action::SaveRoot);
        assert_eq!(app.config.scan.roots, vec!["/tmp/new".to_string()]);
        assert!(matches!(cmds.as_slice(), [Command::ScanWorkspace]));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn cancel_root_editor_discards() {
        let mut app = App::new(Config::default(), test_config_path());
        app.screen = Screen::Workspace;
        app.update(Action::StartAddRoot);
        app.update(Action::RootEditorChar('x'));
        app.update(Action::CancelRootEditor);
        assert!(app.root_editor.is_none());
        assert!(app.config.scan.roots.is_empty());
    }

    #[test]
    fn workspace_tab_and_exclude_crud() {
        let path = test_config_path();
        let mut app = App::new(Config::default(), path.clone());
        app.show_splash = false;
        app.screen = Screen::Workspace;
        app.config.scan.roots = vec!["/tmp/a".into()];
        assert_eq!(app.workspace_panel, WorkspacePanel::Roots);

        app.update(Action::ToggleWorkspacePanel);
        assert_eq!(app.workspace_panel, WorkspacePanel::Excludes);

        app.update(Action::StartAddRoot); // n on excludes → add exclude
        assert!(app.root_editor.as_ref().is_some_and(|s| s.is_exclude()));
        if let Some(ed) = app.root_editor.as_mut() {
            for c in "legacy-api".chars() {
                ed.insert_char(c);
            }
        }
        let cmds = app.update(Action::SaveRoot);
        assert!(
            app.config.scan.exclude.iter().any(|e| e == "legacy-api"),
            "exclude: {:?}",
            app.config.scan.exclude
        );
        // Non-empty roots → rescan
        assert!(cmds.iter().any(|c| matches!(c, Command::ScanWorkspace)));

        app.workspace_panel = WorkspacePanel::Excludes;
        app.selected_exclude_index = 0;
        app.update(Action::StartDeleteExclude);
        assert_eq!(app.exclude_delete_confirm, Some(0));
        app.update(Action::ConfirmDeleteExclude);
        assert!(app.config.scan.exclude.is_empty());
    }

    #[test]
    fn workspace_j_k_moves_root_selection() {
        let mut app = App::new(
            Config {
                scan: ScanConfig {
                    roots: vec!["/a".into(), "/b".into(), "/c".into()],
                    ..Default::default()
                },
                ..Default::default()
            },
            test_config_path(),
        );
        app.screen = Screen::Workspace;
        assert_eq!(app.selected_root_index, 0);
        app.update(Action::MoveSelectionDown);
        assert_eq!(app.selected_root_index, 1);
        app.update(Action::MoveSelectionDown);
        assert_eq!(app.selected_root_index, 2);
        app.update(Action::MoveSelectionDown);
        assert_eq!(app.selected_root_index, 2);
        app.update(Action::MoveSelectionUp);
        assert_eq!(app.selected_root_index, 1);
    }

    // —— M3 single-project exec ——

    #[test]
    fn build_emits_run_gradle_and_registers_job() {
        let mut app = App::new(Config::default(), test_config_path());
        app.show_splash = false;
        let mut p = sample_project("payments-api");
        p.gradle_root = Some(PathBuf::from("/tmp/payments-api"));
        p.skaffold_path = Some(PathBuf::from("/tmp/payments-api/skaffold.yaml"));
        app.projects = vec![p];
        app.selected_index = 0;

        let cmds = app.update(Action::Build);
        assert_eq!(app.screen, Screen::Jobs);
        assert_eq!(app.jobs.len(), 1);
        assert_eq!(app.projects[0].status, "build…");
        match cmds.as_slice() {
            [Command::RunGradle {
                path,
                tasks,
                project_name,
                force_latest_snapshots,
                ..
            }] => {
                assert_eq!(path, &PathBuf::from("/tmp/payments-api"));
                assert_eq!(tasks, &vec!["build".to_string()]);
                assert_eq!(project_name, "payments-api");
                assert!(!*force_latest_snapshots);
            }
            other => panic!("expected RunGradle, got {other:?}"),
        }
    }

    #[test]
    fn kube_probe_finished_applies_deployed_and_drift() {
        let mut app = App::new(Config::default(), test_config_path());
        app.show_splash = false;
        let mut svc = sample_project("payments-api");
        svc.has_skaffold = true;
        svc.version = "3.2.0".into();
        svc.drift = Drift::NotProbed;
        app.projects = vec![svc];
        app.kube_probing = true;

        let batch = crate::kube::ProbeBatch {
            probes: vec![crate::kube::DeployedProbe {
                project_path: PathBuf::from("/tmp/payments-api"),
                project_name: "payments-api".into(),
                deployment: "payments-api".into(),
                namespace: "dev".into(),
                deployed_version: Some("3.1.0".into()),
                deploy_owner: Some("argocd".into()),
                error: None,
            }],
            error: None,
        };
        app.apply_event(AppEvent::KubeProbeFinished { batch });
        assert!(!app.kube_probing);
        assert_eq!(app.projects[0].deployed_version.as_deref(), Some("3.1.0"));
        assert_eq!(app.projects[0].drift, Drift::LocalAhead);
        assert_eq!(app.projects[0].deploy_owner.as_deref(), Some("argocd"));
        assert!(app.kube_probed_at.is_some());
    }

    #[test]
    fn refresh_deployed_versions_requires_kube_enabled() {
        let mut app = App::new(Config::default(), test_config_path());
        assert!(!app.config.kube.enabled);
        let cmds = app.update(Action::RefreshDeployedVersions);
        assert!(cmds.is_empty());
        assert!(
            app.status_message
                .as_deref()
                .is_some_and(|s| s.contains("disabled") || s.contains("enabled=true")),
            "{:?}",
            app.status_message
        );
    }

    #[test]
    fn settings_toggle_kube_enabled_saves() {
        let path = test_config_path();
        let mut app = App::new(Config::default(), path.clone());
        app.show_splash = false;
        app.screen = Screen::Settings;
        // Jump to kube.enabled row
        let idx = SettingId::ALL
            .iter()
            .position(|id| *id == SettingId::KubeEnabled)
            .expect("kube.enabled in ALL");
        app.selected_setting = idx;
        assert!(!app.config.kube.enabled);
        app.update(Action::ActivateSetting);
        assert!(app.config.kube.enabled);
        assert!(
            !app.config.kube.namespaces.is_empty(),
            "enabling seeds a default namespace"
        );
        // Round-trip file
        let loaded = crate::config::load(&path).expect("load");
        assert!(loaded.kube.enabled);
    }

    #[test]
    fn exclude_selected_from_project_browser() {
        let path = test_config_path();
        let mut app = App::new(Config::default(), path);
        app.show_splash = false;
        app.screen = Screen::ProjectBrowser;
        app.config.scan.roots = vec!["/tmp".into()];
        app.projects = vec![
            sample_project("keep-me"),
            sample_project("drop-me"),
        ];
        app.selected_index = 1;
        let cmds = app.update(Action::ExcludeSelectedProjects);
        assert!(
            app.config.scan.exclude.iter().any(|e| e == "drop-me"),
            "exclude: {:?}",
            app.config.scan.exclude
        );
        assert!(cmds.iter().any(|c| matches!(c, Command::ScanWorkspace)));
    }

    #[test]
    fn build_with_deps_opens_rebuild_plan_not_parallel_builds() {
        use crate::exec::RECIPE_PUBLISH_AND_REBUILD;
        use crate::gradle::model::{Coordinate, DepReq, Produces, VersionSpec};

        let mut app = App::new(Config::default(), test_config_path());
        app.show_splash = false;
        let mut lib = discovered(
            "common-lib",
            ProjectKind::Library,
            "1.4.2",
            "main",
            false,
            false,
        );
        lib.produces = vec![Produces::new(
            Coordinate::new("com.example", "common-lib"),
            "1.4.2",
        )];
        let mut svc = discovered(
            "payments-api",
            ProjectKind::Service,
            "3.0.0",
            "main",
            true,
            false,
        );
        svc.depends = vec![DepReq::external(
            Coordinate::new("com.example", "common-lib"),
            VersionSpec::Range("[1.0.0, 2.0.0)".into()),
            "implementation",
            "x",
        )];
        app.apply_scan_projects(&[lib, svc]);
        // Cursor on library producer (not display-sorted index).
        app.selected_index = app
            .projects
            .iter()
            .position(|p| p.name == "common-lib")
            .expect("lib");

        let cmds = app.update(Action::BuildWithDeps);
        assert!(cmds.is_empty(), "plan open is confirm-only: {cmds:?}");
        assert!(app.plan_confirming());
        let plan = app.plan.as_ref().expect("plan");
        assert_eq!(plan.recipe, RECIPE_PUBLISH_AND_REBUILD);
        // publish + consumer build — no skaffold
        assert_eq!(
            plan.steps.len(),
            2,
            "steps: {:?}",
            plan.steps.iter().map(|s| s.step_label()).collect::<Vec<_>>()
        );
        assert!(
            app.status_message
                .as_deref()
                .is_some_and(|s| s.contains("rebuild plan")),
            "{:?}",
            app.status_message
        );
    }

    #[test]
    fn skaffold_dev_requires_skaffold_file() {
        let mut app = App::new(Config::default(), test_config_path());
        let mut p = sample_project("lib");
        p.has_skaffold = false;
        p.skaffold_path = None;
        app.projects = vec![p];
        let cmds = app.update(Action::SkaffoldDev);
        assert!(cmds.is_empty());
        assert!(
            app.status_message
                .as_deref()
                .is_some_and(|s| s.starts_with("no skaffold")),
            "{:?}",
            app.status_message
        );
    }

    #[test]
    fn skaffold_run_emits_command_with_file() {
        let mut app = App::new(Config::default(), test_config_path());
        let mut p = sample_project("svc");
        p.skaffold_path = Some(PathBuf::from("/tmp/svc/skaffold.yaml"));
        app.projects = vec![p];
        let cmds = app.update(Action::SkaffoldRun);
        match cmds.as_slice() {
            [Command::RunSkaffold {
                args,
                skaffold_file,
                ..
            }] => {
                assert_eq!(args.first().map(String::as_str), Some("run"));
                assert_eq!(skaffold_file, &PathBuf::from("/tmp/svc/skaffold.yaml"));
            }
            other => panic!("expected RunSkaffold, got {other:?}"),
        }
        assert_eq!(app.screen, Screen::Jobs);
    }

    #[test]
    fn git_pull_emits_ff_only_command() {
        let mut app = App::new(Config::default(), test_config_path());
        let mut p = sample_project("svc");
        p.git_root = Some(PathBuf::from("/tmp/svc"));
        app.projects = vec![p];
        let cmds = app.update(Action::GitPull);
        match cmds.as_slice() {
            [Command::GitPull {
                git_root,
                ff_only,
                ..
            }] => {
                assert_eq!(git_root, &PathBuf::from("/tmp/svc"));
                assert!(*ff_only);
            }
            other => panic!("expected GitPull, got {other:?}"),
        }
    }

    #[test]
    fn git_pull_dedupes_by_git_root() {
        let mut app = App::new(Config::default(), test_config_path());
        let mut a = sample_project("orders");
        a.git_root = Some(PathBuf::from("/tmp/mono"));
        a.path = PathBuf::from("/tmp/mono/orders");
        let mut b = sample_project("payments");
        b.git_root = Some(PathBuf::from("/tmp/mono"));
        b.path = PathBuf::from("/tmp/mono/payments");
        app.projects = vec![a, b];
        app.selected_index = 0;
        let first = app.update(Action::GitPull);
        assert_eq!(first.len(), 1);
        // Same root via other project row.
        app.selected_index = 1;
        let second = app.update(Action::GitPull);
        assert!(second.is_empty(), "should dedupe active pull for same root");
        assert!(
            app.status_message
                .as_deref()
                .is_some_and(|s| s.starts_with("pull already running")),
            "{:?}",
            app.status_message
        );
    }

    #[test]
    fn job_finished_updates_status_chip_and_toast() {
        let mut app = App::new(Config::default(), test_config_path());
        let mut p = sample_project("svc");
        p.gradle_root = Some(PathBuf::from("/tmp/svc"));
        app.projects = vec![p];
        let _ = app.update(Action::Build);
        let id = app.jobs[0].id;
        app.apply_event(AppEvent::JobStarted { id });
        app.apply_event(AppEvent::JobLog {
            id,
            line: "BUILD SUCCESSFUL".into(),
        });
        app.apply_event(AppEvent::JobFinished {
            id,
            ok: true,
            cancelled: false,
            summary: "exit 0".into(),
        });
        assert_eq!(app.projects[0].status, "ok");
        assert!(
            app.status_message
                .as_deref()
                .is_some_and(|s| s.contains("ok") && s.contains("svc")),
            "{:?}",
            app.status_message
        );
        assert_eq!(app.jobs[0].status, crate::jobs::JobStatus::Ok);
    }

    #[test]
    fn esc_on_jobs_cancels_running_job() {
        let mut app = App::new(Config::default(), test_config_path());
        let mut p = sample_project("svc");
        p.gradle_root = Some(PathBuf::from("/tmp/svc"));
        app.projects = vec![p];
        let _ = app.update(Action::Build);
        let id = app.jobs[0].id;
        app.apply_event(AppEvent::JobStarted { id });
        assert_eq!(app.screen, Screen::Jobs);
        let cmds = app.update(Action::Back);
        assert!(matches!(cmds.as_slice(), [Command::CancelJob { id: cid }] if *cid == id));
    }

    // —— M4 cascade ——

    #[test]
    fn cascade_publish_builds_plan_from_graph() {
        use crate::gradle::model::{Coordinate, DepReq, Produces, VersionSpec};

        let mut app = App::new(Config::default(), test_config_path());
        app.show_splash = false;
        let lib = discovered(
            "common-lib",
            ProjectKind::Library,
            "1.4.2",
            "main",
            false,
            false,
        );
        let mut lib = lib;
        lib.produces = vec![Produces::new(
            Coordinate::new("com.example", "common-lib"),
            "1.4.2",
        )];
        let mut svc = discovered(
            "payments-api",
            ProjectKind::Service,
            "3.0.0",
            "main",
            true,
            false,
        );
        svc.depends = vec![DepReq::external(
            Coordinate::new("com.example", "common-lib"),
            VersionSpec::Range("[1.0.0, 2.0.0)".into()),
            "implementation",
            "libs.common.lib",
        )];
        app.apply_scan_projects(&[lib, svc]);
        // select common-lib
        app.selected_index = 0;

        let cmds = app.update(Action::CascadePublish);
        assert!(cmds.is_empty());
        let plan = app.plan.as_ref().expect("plan open");
        assert!(!plan.executing);
        assert_eq!(plan.recipe, crate::exec::RECIPE_PUBLISH_AND_REDEPLOY);
        // publish + build + delete + run = 4
        assert_eq!(plan.steps.len(), 4);
        assert_eq!(plan.steps[0].project_name, "common-lib");
        assert_eq!(plan.steps[1].project_name, "payments-api");
        assert!(plan.steps[2].step_label().contains("delete"));
        assert!(plan.steps[3].step_label().contains("run"));
    }

    #[test]
    fn cascade_sequential_advance_on_job_finished() {
        use crate::gradle::model::{Coordinate, DepReq, Produces, VersionSpec};

        let mut app = App::new(Config::default(), test_config_path());
        app.show_splash = false;
        let mut lib = discovered(
            "common-lib",
            ProjectKind::Library,
            "1.4.2",
            "main",
            false,
            false,
        );
        lib.produces = vec![Produces::new(
            Coordinate::new("com.example", "common-lib"),
            "1.4.2",
        )];
        let mut svc = discovered(
            "payments-api",
            ProjectKind::Service,
            "3.0.0",
            "main",
            true,
            false,
        );
        svc.depends = vec![DepReq::external(
            Coordinate::new("com.example", "common-lib"),
            VersionSpec::Range("[1.0.0, 2.0.0)".into()),
            "implementation",
            "x",
        )];
        app.apply_scan_projects(&[lib, svc]);
        app.selected_index = 0;
        app.update(Action::CascadePublish);
        assert!(app.plan_confirming());

        // Confirm → first gradle job
        let cmds = app.update(Action::ConfirmCascadePlan);
        assert_eq!(app.screen, Screen::Jobs);
        assert!(app.plan.as_ref().is_some_and(|p| p.executing));
        assert_eq!(cmds.len(), 1);
        let id1 = match &cmds[0] {
            Command::RunGradle { id, .. } => *id,
            other => panic!("expected RunGradle, got {other:?}"),
        };

        // Finish step 1 ok → step 2 (consumer build)
        let next = app.apply_event(AppEvent::JobFinished {
            id: id1,
            ok: true,
            cancelled: false,
            summary: "exit 0".into(),
        });
        assert_eq!(next.len(), 1);
        let id2 = match &next[0] {
            Command::RunGradle {
                id,
                project_name,
                force_latest_snapshots,
                ..
            } => {
                assert_eq!(project_name, "payments-api");
                assert!(
                    *force_latest_snapshots,
                    "consumer rebuild should force SNAPSHOT re-resolve"
                );
                *id
            }
            other => panic!("expected consumer gradle, got {other:?}"),
        };

        // Fail step 2 → stop, no more commands, plan cleared
        let stopped = app.apply_event(AppEvent::JobFinished {
            id: id2,
            ok: false,
            cancelled: false,
            summary: "exit 1".into(),
        });
        assert!(stopped.is_empty());
        assert!(app.plan.is_none());
        assert!(
            app.status_message
                .as_deref()
                .is_some_and(|s| s.contains("cascade failed")),
            "{:?}",
            app.status_message
        );
    }

    #[test]
    fn cascade_cancel_plan_before_run() {
        let mut app = App::new(Config::default(), test_config_path());
        app.show_splash = false;
        app.projects = vec![sample_project("lib")];
        app.graph = DependencyGraph::from_projects(&[]);
        // Even with empty graph, plan can open as publish-only with warning.
        // Need graph aligned with projects:
        let d = discovered("lib", ProjectKind::Library, "1.0", "main", false, false);
        app.apply_scan_projects(&[d]);
        app.update(Action::CascadePublish);
        assert!(app.plan.is_some());
        app.update(Action::CancelCascadePlan);
        assert!(app.plan.is_none());
        assert!(
            app.status_message
                .as_deref()
                .is_some_and(|s| s.contains("cascade cancelled")),
            "{:?}",
            app.status_message
        );
    }

    #[test]
    fn scan_finished_builds_graph_and_highlights() {
        use crate::gradle::model::{Coordinate, DepReq, Produces, VersionSpec};

        let mut lib = discovered(
            "common-lib",
            ProjectKind::Library,
            "1.4.2",
            "main",
            false,
            false,
        );
        lib.produces = vec![Produces::new(
            Coordinate::new("com.example", "common-lib"),
            "1.4.2",
        )];

        let mut svc = discovered(
            "payments-api",
            ProjectKind::Service,
            "3.0.0",
            "main",
            true,
            false,
        );
        svc.depends = vec![DepReq::external(
            Coordinate::new("com.example", "common-lib"),
            VersionSpec::Range("[1.0.0, 2.0.0)".into()),
            "implementation",
            "libs.common.lib",
        )];

        let mut app = App::new(Config::default(), test_config_path());
        app.apply_event(AppEvent::ScanFinished {
            projects: vec![lib, svc],
            error: None,
        });

        assert_eq!(app.graph.len(), 2);
        // Selected index clamps to last (1) after empty→2 projects with prior 0?
        // clamp keeps 0 if was 0.
        app.set_selection(0); // common-lib
        assert!(
            app.dependent_indices.contains(&1),
            "payments is dependent of common-lib: {:?}",
            app.dependent_indices
        );
        assert!(app.dep_indices.is_empty());

        app.set_selection(1); // payments
        assert!(
            app.dep_indices.contains(&0),
            "common-lib is a dep of payments: {:?}",
            app.dep_indices
        );
        assert!(app.dependent_indices.is_empty());
    }

    // —— M5 multi-select bulk ——

    #[test]
    fn toggle_multi_select_adds_and_removes_cursor() {
        let mut app = App::new(Config::default(), test_config_path());
        app.projects = vec![sample_project("a"), sample_project("b")];
        app.selected_index = 0;
        app.screen = Screen::ProjectBrowser;

        app.update(Action::ToggleMultiSelect);
        assert!(app.multi_selected.contains(&0));
        assert_eq!(app.multi_selected.len(), 1);

        app.update(Action::MoveSelectionDown);
        assert_eq!(app.selected_index, 1);
        app.update(Action::ToggleMultiSelect);
        assert_eq!(app.multi_selected, HashSet::from([0, 1]));

        // Toggle off the second.
        app.update(Action::ToggleMultiSelect);
        assert_eq!(app.multi_selected, HashSet::from([0]));
    }

    #[test]
    fn esc_closes_detail_then_clears_multi_select() {
        let mut app = App::new(Config::default(), test_config_path());
        app.projects = vec![sample_project("a")];
        app.screen = Screen::ProjectBrowser;
        app.multi_selected.insert(0);
        app.project_detail_visible = true;

        // Full-screen detail: Esc returns to list first (keeps multi-select).
        app.update(Action::Back);
        assert!(!app.project_detail_visible);
        assert_eq!(app.multi_selected.len(), 1);

        app.update(Action::Back);
        assert!(app.multi_selected.is_empty());
    }

    #[test]
    fn bulk_build_emits_commands_for_multi_selected() {
        let mut app = App::new(Config::default(), test_config_path());
        app.show_splash = false;
        let mut a = sample_project("alpha");
        a.gradle_root = Some(PathBuf::from("/tmp/alpha"));
        let mut b = sample_project("beta");
        b.gradle_root = Some(PathBuf::from("/tmp/beta"));
        let mut c = sample_project("gamma");
        c.gradle_root = Some(PathBuf::from("/tmp/gamma"));
        app.projects = vec![a, b, c];
        app.multi_selected = HashSet::from([0, 2]);
        app.selected_index = 1; // cursor on non-selected row — bulk ignores cursor

        let cmds = app.update(Action::Build);
        assert_eq!(cmds.len(), 2, "expected two RunGradle: {cmds:?}");
        assert_eq!(app.jobs.len(), 2);
        let paths: Vec<_> = cmds
            .iter()
            .filter_map(|c| match c {
                Command::RunGradle { path, .. } => Some(path.clone()),
                _ => None,
            })
            .collect();
        assert!(paths.contains(&PathBuf::from("/tmp/alpha")));
        assert!(paths.contains(&PathBuf::from("/tmp/beta")) == false);
        assert!(paths.contains(&PathBuf::from("/tmp/gamma")));
        assert!(
            app.status_message
                .as_deref()
                .is_some_and(|s| s.contains("2 projects")),
            "{:?}",
            app.status_message
        );
    }

    #[test]
    fn bulk_clean_uses_clean_tasks() {
        let mut app = App::new(Config::default(), test_config_path());
        let mut a = sample_project("a");
        a.gradle_root = Some(PathBuf::from("/tmp/a"));
        let mut b = sample_project("b");
        b.gradle_root = Some(PathBuf::from("/tmp/b"));
        app.projects = vec![a, b];
        app.multi_selected = HashSet::from([0, 1]);

        let cmds = app.update(Action::Clean);
        assert_eq!(cmds.len(), 2);
        for c in &cmds {
            match c {
                Command::RunGradle { tasks, .. } => {
                    assert_eq!(tasks, &vec!["clean".to_string()]);
                }
                other => panic!("expected RunGradle, got {other:?}"),
            }
        }
    }

    #[test]
    fn bulk_git_pull_dedupes_unique_roots() {
        let mut app = App::new(Config::default(), test_config_path());
        let mut a = sample_project("orders");
        a.git_root = Some(PathBuf::from("/tmp/mono"));
        a.path = PathBuf::from("/tmp/mono/orders");
        let mut b = sample_project("payments");
        b.git_root = Some(PathBuf::from("/tmp/mono"));
        b.path = PathBuf::from("/tmp/mono/payments");
        let mut c = sample_project("standalone");
        c.git_root = Some(PathBuf::from("/tmp/standalone"));
        c.path = PathBuf::from("/tmp/standalone");
        app.projects = vec![a, b, c];
        app.multi_selected = HashSet::from([0, 1, 2]);

        let cmds = app.update(Action::GitPull);
        assert_eq!(cmds.len(), 2, "two unique roots: {cmds:?}");
        let roots: HashSet<_> = cmds
            .iter()
            .filter_map(|c| match c {
                Command::GitPull { git_root, .. } => Some(git_root.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            roots,
            HashSet::from([
                PathBuf::from("/tmp/mono"),
                PathBuf::from("/tmp/standalone")
            ])
        );
    }

    #[test]
    fn empty_multi_select_build_uses_cursor_only() {
        let mut app = App::new(Config::default(), test_config_path());
        let mut a = sample_project("a");
        a.gradle_root = Some(PathBuf::from("/tmp/a"));
        let mut b = sample_project("b");
        b.gradle_root = Some(PathBuf::from("/tmp/b"));
        app.projects = vec![a, b];
        app.selected_index = 1;
        assert!(app.multi_selected.is_empty());

        let cmds = app.update(Action::Build);
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            Command::RunGradle {
                path, project_name, ..
            } => {
                assert_eq!(path, &PathBuf::from("/tmp/b"));
                assert_eq!(project_name, "b");
            }
            other => panic!("expected RunGradle, got {other:?}"),
        }
    }

    #[test]
    fn enter_toggles_project_detail() {
        let mut app = App::new(Config::default(), test_config_path());
        app.projects = vec![sample_project("a")];
        app.screen = Screen::ProjectBrowser;
        assert!(!app.project_detail_visible);
        app.update(Action::Confirm);
        assert!(app.project_detail_visible);
        app.update(Action::Confirm);
        assert!(!app.project_detail_visible);
        app.update(Action::Confirm);
        assert!(app.project_detail_visible);
        app.update(Action::Back);
        assert!(!app.project_detail_visible);
    }
}
