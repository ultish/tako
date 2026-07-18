//! tako entry point: terminal bootstrap + Elm event loop (rakko-shaped).

mod app;
mod cli;
mod config;
mod deploy;
mod error;
mod events;
mod exec;
mod git;
mod gradle;
mod graph;
mod jobs;
mod kube;
mod nexus;
mod ring_buffer;
mod scan;
mod text_field;
mod ui;
mod version_bump;

use std::collections::HashMap;
use std::io::Stdout;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use clap::Parser;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::{mpsc, watch};

use app::{App, BannerMode, Screen};
use cli::Cli;
use config::expand_user_path;
use error::AppResult;
use events::{Action, AppEvent, Command};
use exec::{spawn_and_stream, SpawnOpts};
use scan::ScanOptions;

type Term = Terminal<CrosstermBackend<Stdout>>;

/// Leaves the alternate screen and disables raw mode. Shared between the normal
/// exit path and the panic hook so a mid-render panic never leaves the terminal
/// broken.
fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(
        std::io::stdout(),
        DisableMouseCapture,
        LeaveAlternateScreen
    );
}

fn init_terminal() -> AppResult<Term> {
    enable_raw_mode()?;
    execute!(
        std::io::stdout(),
        EnterAlternateScreen,
        EnableMouseCapture
    )?;
    let backend = CrosstermBackend::new(std::io::stdout());
    Ok(Terminal::new(backend)?)
}

/// Logs go to a file under the config dir only — never stdout/stderr, which
/// would corrupt the alternate screen.
fn init_tracing(log_dir: &Path) -> AppResult<tracing_appender::non_blocking::WorkerGuard> {
    std::fs::create_dir_all(log_dir)?;
    let file_appender = tracing_appender::rolling::never(log_dir, "tako.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
    tracing_subscriber::fmt()
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    Ok(guard)
}

/// Shifted letter: `Char('U')` **or** `Char('u')`+SHIFT (terminal-dependent).
fn is_shift_letter(key: KeyEvent, lower: char) -> bool {
    let upper = lower.to_ascii_uppercase();
    match key.code {
        KeyCode::Char(c) if c == upper => true,
        KeyCode::Char(c) if c == lower && key.modifiers.contains(KeyModifiers::SHIFT) => true,
        _ => false,
    }
}

/// Plain lowercase letter without SHIFT (so Shift+u does not match **u**).
fn is_plain_letter(key: KeyEvent, lower: char) -> bool {
    matches!(key.code, KeyCode::Char(c) if c == lower)
        && !key.modifiers.contains(KeyModifiers::SHIFT)
}

/// Translates a raw key press into an `Action`.
fn key_to_action(key: KeyEvent, app: &App) -> Option<Action> {
    if key.kind != KeyEventKind::Press {
        return None;
    }

    // Quit confirmation owns the keyboard while open.
    if app.quit_confirm {
        return match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(Action::ForceQuit)
            }
            KeyCode::Char('y') | KeyCode::Enter => Some(Action::ConfirmQuit),
            KeyCode::Char('n') | KeyCode::Esc => Some(Action::CancelQuit),
            _ => None,
        };
    }

    // Root delete confirm owns the keyboard (mutually exclusive with path editor).
    if app.root_delete_confirm.is_some() {
        return match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(Action::ForceQuit)
            }
            KeyCode::Char('y') | KeyCode::Enter => Some(Action::ConfirmDeleteRoot),
            KeyCode::Char('n') | KeyCode::Esc => Some(Action::CancelDeleteRoot),
            _ => None,
        };
    }

    // Exclude delete confirm.
    if app.exclude_delete_confirm.is_some() {
        return match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(Action::ForceQuit)
            }
            KeyCode::Char('y') | KeyCode::Enter => Some(Action::ConfirmDeleteExclude),
            KeyCode::Char('n') | KeyCode::Esc => Some(Action::CancelDeleteExclude),
            _ => None,
        };
    }

    // Argo skaffold confirm.
    if app.pending_argo_skaffold.is_some() {
        return match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(Action::ForceQuit)
            }
            KeyCode::Char('y') | KeyCode::Enter => Some(Action::ConfirmArgoSkaffold),
            KeyCode::Char('n') | KeyCode::Esc => Some(Action::CancelArgoSkaffold),
            _ => None,
        };
    }

    // Project filter typing mode.
    if app.project_filter.is_some() && app.screen == Screen::ProjectBrowser {
        // Multi-select + bulk still work while filtering (Space is not typed into the query).
        if matches!(key.code, KeyCode::Char(' ') | KeyCode::Char('m')) {
            return Some(Action::ToggleMultiSelect);
        }
        if is_shift_letter(key, 'b') {
            return Some(Action::BuildForce);
        }
        if is_plain_letter(key, 'b') {
            return Some(Action::Build);
        }
        if is_plain_letter(key, 'c') && !key.modifiers.contains(KeyModifiers::CONTROL) {
            return Some(Action::Clean);
        }
        if is_shift_letter(key, 'g') {
            return Some(Action::GitPull);
        }
        return match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(Action::ForceQuit)
            }
            KeyCode::Esc => Some(Action::ClearProjectFilter),
            KeyCode::Enter => Some(Action::ClearProjectFilter),
            KeyCode::Backspace => Some(Action::ProjectFilterBackspace),
            KeyCode::Char(c)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                Some(Action::ProjectFilterChar(c))
            }
            KeyCode::Up | KeyCode::Char('k') => Some(Action::MoveSelectionUp),
            KeyCode::Down | KeyCode::Char('j') => Some(Action::MoveSelectionDown),
            _ => None,
        };
    }

    // Bump dependents plan owns the keyboard.
    if app.bump_confirming() {
        return match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(Action::ForceQuit)
            }
            KeyCode::Char('y') | KeyCode::Enter => Some(Action::ConfirmBumpDependents),
            KeyCode::Char('n') | KeyCode::Esc => Some(Action::CancelBumpDependents),
            KeyCode::Tab => Some(Action::CycleBumpKind),
            KeyCode::Char('1') => {
                Some(Action::SetBumpKind(crate::version_bump::BumpKind::Major))
            }
            KeyCode::Char('2') => {
                Some(Action::SetBumpKind(crate::version_bump::BumpKind::Minor))
            }
            KeyCode::Char('3') => {
                Some(Action::SetBumpKind(crate::version_bump::BumpKind::Patch))
            }
            KeyCode::Up | KeyCode::Char('k') => Some(Action::MoveSelectionUp),
            KeyCode::Down | KeyCode::Char('j') => Some(Action::MoveSelectionDown),
            KeyCode::Char('q') => Some(Action::Quit),
            _ => None,
        };
    }

    // Cascade plan confirm owns the keyboard (y/Enter run, n/Esc cancel, j/k scroll).
    if app.plan_confirming() {
        return match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(Action::ForceQuit)
            }
            KeyCode::Char('y') | KeyCode::Enter => Some(Action::ConfirmCascadePlan),
            KeyCode::Char('n') | KeyCode::Esc => Some(Action::CancelCascadePlan),
            KeyCode::Up | KeyCode::Char('k') => Some(Action::MoveSelectionUp),
            KeyCode::Down | KeyCode::Char('j') => Some(Action::MoveSelectionDown),
            KeyCode::Char('q') => Some(Action::Quit),
            KeyCode::Char('?') => Some(Action::ToggleHelp),
            _ => None,
        };
    }

    // Help overlay owns the keyboard (close with ? / Esc; Tab switches page).
    if app.help_visible {
        return match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(Action::ForceQuit)
            }
            KeyCode::Char('q') => Some(Action::Quit),
            KeyCode::Char('?') | KeyCode::Esc => Some(Action::ToggleHelp),
            KeyCode::Tab | KeyCode::Left | KeyCode::Right => Some(Action::CycleHelpPage),
            _ => None,
        };
    }

    // Startup splash: any key dismisses.
    if app.show_splash {
        return match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(Action::ForceQuit)
            }
            KeyCode::Char('q') => Some(Action::Quit),
            _ => Some(Action::DismissSplash),
        };
    }

    // Root path editor: type path; Enter saves; Esc cancels.
    if app.root_editor.is_some() {
        return match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(Action::ForceQuit)
            }
            KeyCode::Esc => Some(Action::CancelRootEditor),
            KeyCode::Enter => Some(Action::SaveRoot),
            KeyCode::Backspace => Some(Action::RootEditorBackspace),
            KeyCode::Delete => Some(Action::RootEditorDelete),
            KeyCode::Left => Some(Action::RootEditorCursorLeft),
            KeyCode::Right => Some(Action::RootEditorCursorRight),
            KeyCode::Home => Some(Action::RootEditorCursorHome),
            KeyCode::End => Some(Action::RootEditorCursorEnd),
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                Some(Action::RootEditorChar(c))
            }
            _ => None,
        };
    }

    // Workspace screen: root / exclude list keybinds when no editor open.
    if app.screen == Screen::Workspace {
        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Some(Action::ForceQuit);
            }
            KeyCode::Char('q') => return Some(Action::Quit),
            KeyCode::Tab => return Some(Action::ToggleWorkspacePanel),
            KeyCode::Char('n') => return Some(Action::StartAddRoot),
            KeyCode::Char('e') => return Some(Action::StartEditRoot),
            KeyCode::Char('z') => return Some(Action::StartDeleteRoot),
            KeyCode::Up | KeyCode::Char('k') => return Some(Action::MoveSelectionUp),
            KeyCode::Down | KeyCode::Char('j') => return Some(Action::MoveSelectionDown),
            KeyCode::Enter => return Some(Action::Confirm),
            KeyCode::Esc => return Some(Action::Back),
            KeyCode::Char('w') => return Some(Action::Refresh),
            KeyCode::Char('?') => return Some(Action::ToggleHelp),
            KeyCode::Char('A') => return Some(Action::CycleBannerMode),
            KeyCode::Char('T') => return Some(Action::CycleTheme),
            KeyCode::Char('1') => return Some(Action::SwitchToProjects),
            KeyCode::Char('2') => return Some(Action::SwitchToJobs),
            KeyCode::Char('3') => return Some(Action::SwitchToWorkspace),
            KeyCode::Char('4') => return Some(Action::SwitchToSettings),
            _ => {}
        }
    }

    // Settings screen editor / navigation.
    if app.settings_editor.is_some() {
        return match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(Action::ForceQuit)
            }
            KeyCode::Esc => Some(Action::CancelSettingsEditor),
            KeyCode::Enter => Some(Action::SaveSettingsEditor),
            KeyCode::Backspace => Some(Action::SettingsEditorBackspace),
            KeyCode::Delete => Some(Action::SettingsEditorDelete),
            KeyCode::Left => Some(Action::SettingsEditorCursorLeft),
            KeyCode::Right => Some(Action::SettingsEditorCursorRight),
            KeyCode::Home => Some(Action::SettingsEditorCursorHome),
            KeyCode::End => Some(Action::SettingsEditorCursorEnd),
            KeyCode::Char(c)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                Some(Action::SettingsEditorChar(c))
            }
            _ => None,
        };
    }

    if app.screen == Screen::Settings {
        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Some(Action::ForceQuit);
            }
            KeyCode::Char('q') => return Some(Action::Quit),
            KeyCode::Up | KeyCode::Char('k') => return Some(Action::MoveSelectionUp),
            KeyCode::Down | KeyCode::Char('j') => return Some(Action::MoveSelectionDown),
            KeyCode::Enter | KeyCode::Char(' ') => return Some(Action::ActivateSetting),
            KeyCode::Left => return Some(Action::NudgeSettingLeft),
            KeyCode::Right => return Some(Action::NudgeSettingRight),
            KeyCode::Esc => return Some(Action::Back),
            KeyCode::Char('?') => return Some(Action::ToggleHelp),
            KeyCode::Char('A') => return Some(Action::CycleBannerMode),
            KeyCode::Char('T') => return Some(Action::CycleTheme),
            KeyCode::Char('1') => return Some(Action::SwitchToProjects),
            KeyCode::Char('2') => return Some(Action::SwitchToJobs),
            KeyCode::Char('3') => return Some(Action::SwitchToWorkspace),
            KeyCode::Char('4') => return Some(Action::SwitchToSettings),
            _ => {}
        }
    }

    // Jobs screen: Tab toggles list/log focus; Esc handled via Back.
    if app.screen == Screen::Jobs {
        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Some(Action::ForceQuit);
            }
            KeyCode::Char('q') => return Some(Action::Quit),
            KeyCode::Tab => return Some(Action::ToggleJobsFocus),
            KeyCode::Up | KeyCode::Char('k') => return Some(Action::MoveSelectionUp),
            KeyCode::Down | KeyCode::Char('j') => return Some(Action::MoveSelectionDown),
            KeyCode::Esc => return Some(Action::Back),
            KeyCode::Char('?') => return Some(Action::ToggleHelp),
            KeyCode::Char('A') => return Some(Action::CycleBannerMode),
            KeyCode::Char('T') => return Some(Action::CycleTheme),
            KeyCode::Char('1') => return Some(Action::SwitchToProjects),
            KeyCode::Char('2') => return Some(Action::SwitchToJobs),
            KeyCode::Char('3') => return Some(Action::SwitchToWorkspace),
            KeyCode::Char('4') => return Some(Action::SwitchToSettings),
            _ => {}
        }
    }

    // Project browser: workflow keys.
    // Capital letters: handle both Char('U') and Char('u')+SHIFT (terminal-dependent).
    // Critical: plain `u` must NOT match Shift+u, or Update Dependents never fires.
    if app.screen == Screen::ProjectBrowser {
        if is_shift_letter(key, 'b') {
            return Some(Action::BuildForce);
        }
        if is_plain_letter(key, 'b') {
            return Some(Action::Build);
        }
        if is_plain_letter(key, 'c') && !key.modifiers.contains(KeyModifiers::CONTROL) {
            return Some(Action::Clean);
        }
        if is_plain_letter(key, 'p') {
            return Some(Action::Publish);
        }
        if is_shift_letter(key, 'u') {
            return Some(Action::UpdateDependents);
        }
        if is_plain_letter(key, 'u') {
            return Some(Action::SkaffoldRedeploy);
        }
        if is_plain_letter(key, 'x') {
            return Some(Action::SkaffoldDelete);
        }
        if is_shift_letter(key, 'v') {
            return Some(Action::OpenBumpDependents);
        }
        if is_plain_letter(key, 'v') {
            return Some(Action::OpenBumpVersion);
        }
        if is_plain_letter(key, 'i') {
            return Some(Action::ToggleImpact);
        }
        if is_shift_letter(key, 'g') {
            return Some(Action::GitPull);
        }
        if is_plain_letter(key, 'r') {
            return Some(Action::RefreshStats);
        }
        if is_shift_letter(key, 'k') {
            return Some(Action::RefreshDeployedVersions);
        }
        if is_plain_letter(key, 'w') {
            return Some(Action::Refresh);
        }
        if is_shift_letter(key, 'f') {
            return Some(Action::ToggleFavorite);
        }
        if is_plain_letter(key, 'f') {
            return Some(Action::ToggleDriftFilter);
        }
        match key.code {
            KeyCode::Char(' ') | KeyCode::Char('m') => return Some(Action::ToggleMultiSelect),
            KeyCode::Char('/') => return Some(Action::StartProjectFilter),
            KeyCode::Char('-') => return Some(Action::ExcludeSelectedProjects),
            _ => {}
        }
    }

    match key.code {
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(Action::ForceQuit)
        }
        KeyCode::Char('q') => Some(Action::Quit),
        KeyCode::Up | KeyCode::Char('k') => Some(Action::MoveSelectionUp),
        KeyCode::Down | KeyCode::Char('j') => Some(Action::MoveSelectionDown),
        KeyCode::Enter => Some(Action::Confirm),
        KeyCode::Esc => Some(Action::Back),
        KeyCode::Char('w') => Some(Action::Refresh),
        KeyCode::Char('?') => Some(Action::ToggleHelp),
        KeyCode::Char('A') => Some(Action::CycleBannerMode),
        KeyCode::Char('T') => Some(Action::CycleTheme),
        KeyCode::Char('1') => Some(Action::SwitchToProjects),
        KeyCode::Char('2') => Some(Action::SwitchToJobs),
        KeyCode::Char('3') => Some(Action::SwitchToWorkspace),
        KeyCode::Char('4') => Some(Action::SwitchToSettings),
        KeyCode::Tab if app.screen == Screen::Jobs => Some(Action::ToggleJobsFocus),
        _ => None,
    }
}

/// Scroll wheel nudges selection; left click looks up the last frame's region map.
fn mouse_to_action(mouse: MouseEvent, app: &App) -> Option<Action> {
    match mouse.kind {
        MouseEventKind::ScrollUp => Some(Action::MoveSelectionUp),
        MouseEventKind::ScrollDown => Some(Action::MoveSelectionDown),
        MouseEventKind::Down(MouseButton::Left) => {
            if app.show_splash {
                Some(Action::DismissSplash)
            } else {
                app.action_at(mouse.column, mouse.row)
            }
        }
        _ => None,
    }
}

/// How close together two clicks on the same row need to be to count as a double-click.
const DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(400);

/// Double-click only applies to row selection (open-on-double-click).
fn check_double_click(action: &Action, last: &mut Option<(Instant, Action)>) -> bool {
    if !matches!(action, Action::SelectRow(_)) {
        *last = None;
        return false;
    }
    let is_double = last
        .as_ref()
        .is_some_and(|(t, prev)| prev == action && t.elapsed() < DOUBLE_CLICK_WINDOW);
    *last = if is_double {
        None
    } else {
        Some((Instant::now(), action.clone()))
    };
    is_double
}

fn dispatch_action(action: Action, app: &mut App, tx: &mpsc::UnboundedSender<AppEvent>) {
    let commands = app.update(action);
    for command in commands {
        handle_command(command, app, tx);
    }
}

fn handle_command(command: Command, app: &mut App, tx: &mpsc::UnboundedSender<AppEvent>) {
    match command {
        Command::ScanWorkspace => spawn_workspace_scan(app, tx),
        Command::RunGradle {
            id,
            path,
            tasks,
            project_name: _,
            force_latest_snapshots,
        } => {
            let program = app.config.gradle.command.clone();
            let config_dir = app
                .config_path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| app.config_path.clone());
            let init_path = if force_latest_snapshots {
                match exec::ensure_snapshot_init_script(&config_dir) {
                    Ok(p) => Some(p),
                    Err(err) => {
                        app.set_ephemeral_status(format!("init script: {err}"));
                        None
                    }
                }
            } else {
                None
            };
            let planned = exec::gradle_argv(&exec::GradlePlan {
                command: &program,
                project_dir: &path,
                tasks: &tasks,
                init_script: init_path.as_deref(),
            });
            spawn_job(id, planned, tx);
        }
        Command::RunSkaffold {
            id,
            path: _,
            args,
            skaffold_file,
            project_name: _,
        } => {
            let program = app.config.skaffold.command.clone();
            // args[0] is subcommand; remainder are extras (profile etc.).
            let (subcommand, extra) = match args.split_first() {
                Some((sub, rest)) => (sub.as_str(), rest),
                None => ("run", &[][..]),
            };
            let planned = exec::skaffold_argv(&exec::SkaffoldPlan {
                command: &program,
                subcommand,
                skaffold_file: &skaffold_file,
                extra_args: extra,
            });
            spawn_job(id, planned, tx);
        }
        Command::GitPull {
            id,
            git_root,
            project_name: _,
            project_path: _,
            ff_only,
        } => {
            let extra = app.config.git.pull_args.clone();
            let planned = exec::git_pull_argv(&exec::GitPullPlan {
                git_root: &git_root,
                ff_only,
                extra_args: &extra,
            });
            spawn_job(id, planned, tx);
        }
        Command::CancelJob { id } => {
            // Best-effort: signal cancel watch for this job if we track one.
            // Full cancel map is registered when the job is spawned.
            request_job_cancel(id);
        }
        Command::ProbeKubeVersions => {
            spawn_kube_probe(app, tx);
        }
        Command::ProbeRepoStats { fetch_git } => {
            spawn_repo_stats(app, fetch_git, tx);
        }
    }
}

/// Off-thread kubectl inventory of Deployments → deployed versions (phase 2).
fn spawn_kube_probe(app: &mut App, tx: &mpsc::UnboundedSender<AppEvent>) {
    let projects = app.projects.clone();
    let kube = app.config.kube.clone();
    let tx = tx.clone();
    tokio::task::spawn_blocking(move || {
        let batch = kube::probe_deployed_versions(&projects, &kube);
        if let Err(err) = tx.send(AppEvent::KubeProbeFinished { batch }) {
            tracing::warn!("failed to deliver kube probe result: {err}");
        }
    });
}

/// Off-thread git fetch/lag + Nexus maven-metadata probe (**r**).
fn spawn_repo_stats(app: &mut App, fetch_git: bool, tx: &mpsc::UnboundedSender<AppEvent>) {
    let projects = app.projects.clone();
    let graph = app.graph.clone();
    let nexus_cfg = app.config.nexus.clone();
    let tx = tx.clone();
    tokio::task::spawn_blocking(move || {
        use std::collections::HashMap;
        let mut by_root: HashMap<PathBuf, crate::git::GitInfo> = HashMap::new();
        for p in &projects {
            let Some(root) = p.git_root.as_ref() else {
                continue;
            };
            if by_root.contains_key(root) {
                continue;
            }
            by_root.insert(root.clone(), crate::git::git_remote_sync(root, fetch_git));
        }
        let git: Vec<_> = by_root.into_iter().collect();
        let nexus = crate::nexus::probe_nexus_versions(&projects, &graph, &nexus_cfg);
        let error = None;
        if let Err(err) = tx.send(AppEvent::RepoStatsFinished { git, nexus, error }) {
            tracing::warn!("failed to deliver repo stats: {err}");
        }
    });
}

/// Per-job cancel senders (set true to kill the process group).
fn job_cancel_map() -> std::sync::MutexGuard<'static, HashMap<u64, watch::Sender<bool>>> {
    static JOB_CANCEL: OnceLock<Mutex<HashMap<u64, watch::Sender<bool>>>> = OnceLock::new();
    JOB_CANCEL
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn request_job_cancel(id: u64) {
    if let Some(tx) = job_cancel_map().get(&id) {
        let _ = tx.send(true);
    }
}

fn spawn_job(id: u64, planned: exec::PlannedCommand, tx: &mpsc::UnboundedSender<AppEvent>) {
    let (cancel_tx, cancel_rx) = watch::channel(false);
    job_cancel_map().insert(id, cancel_tx);
    let (line_tx, mut line_rx) = mpsc::unbounded_channel::<String>();
    let event_tx = tx.clone();
    let _ = event_tx.send(AppEvent::JobStarted { id });

    // Forward log lines onto the AppEvent channel.
    let log_tx = event_tx.clone();
    tokio::spawn(async move {
        while let Some(line) = line_rx.recv().await {
            if log_tx.send(AppEvent::JobLog { id, line }).is_err() {
                break;
            }
        }
    });

    tokio::spawn(async move {
        let (ok, summary) = spawn_and_stream(SpawnOpts {
            planned,
            job_id: id,
            cancel_rx,
            line_tx,
        })
        .await;
        job_cancel_map().remove(&id);
        let cancelled = summary == "cancelled";
        let _ = event_tx.send(AppEvent::JobFinished {
            id,
            ok,
            cancelled,
            summary,
        });
    });
}

/// Off-thread filesystem walk. Never blocks the render loop.
fn spawn_workspace_scan(app: &mut App, tx: &mpsc::UnboundedSender<AppEvent>) {
    if app.config.scan.roots.is_empty() {
        // Belt-and-suspenders: App::update already sets status for this path.
        app.set_ephemeral_status("no scan roots configured");
        app.scanning = false;
        return;
    }

    let roots: Vec<PathBuf> = app
        .config
        .scan
        .roots
        .iter()
        .map(|r| expand_user_path(r))
        .collect();
    let max_depth = app.config.scan.max_depth;
    let ignore = app.config.scan.ignore.clone();
    let exclude = app.config.scan.exclude.clone();
    let tx = tx.clone();

    let _ = tx.send(AppEvent::ScanStarted);

    tokio::task::spawn_blocking(move || {
        let opts = ScanOptions {
            roots,
            max_depth,
            ignore,
            exclude,
        };
        let event = match scan::scan_workspace(&opts) {
            Ok(projects) => AppEvent::ScanFinished {
                projects,
                error: None,
            },
            Err(err) => AppEvent::ScanFinished {
                projects: vec![],
                error: Some(err),
            },
        };
        if let Err(err) = tx.send(event) {
            tracing::warn!("failed to deliver scan result: {err}");
        }
    });
}

#[tokio::main]
async fn main() -> AppResult<()> {
    let cli = Cli::parse();

    let config_dir: PathBuf = match &cli.config_dir {
        Some(dir) => dir.clone(),
        None => config::config_dir()?,
    };
    let config_path = config_dir.join("config.toml");

    // Held for the process lifetime: dropping it stops the non-blocking writer.
    let _tracing_guard = init_tracing(&config_dir)?;
    tracing::info!("tako starting up, config path: {}", config_path.display());

    let cfg = config::load(&config_path)?;
    let mut app = App::new(cfg, config_path);

    let panic_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        panic_hook(info);
    }));

    let mut terminal = init_terminal()?;
    let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();

    // Hydrate from cache already happened in App::new; kick off a live rescan
    // when roots are configured so the list isn't only as fresh as the cache.
    for command in app.startup_commands() {
        handle_command(command, &mut app, &tx);
    }

    let mut events = EventStream::new();
    let result = run_loop(&mut terminal, &mut app, &mut events, &mut rx, &tx).await;

    restore_terminal();
    result
}

async fn run_loop(
    terminal: &mut Term,
    app: &mut App,
    events: &mut EventStream,
    rx: &mut mpsc::UnboundedReceiver<AppEvent>,
    tx: &mpsc::UnboundedSender<AppEvent>,
) -> AppResult<()> {
    let mut last_row_click: Option<(Instant, Action)> = None;

    // Braille banner animation (~5 fps when enabled).
    let mut banner_tick = tokio::time::interval(Duration::from_millis(200));
    banner_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    banner_tick.tick().await;

    // Wakes the loop so ephemeral status-bar messages can auto-clear without waiting for input.
    let mut status_dismiss_tick = tokio::time::interval(Duration::from_millis(100));
    status_dismiss_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    status_dismiss_tick.tick().await;

    loop {
        let _ = app.expire_status_if_due();

        // Timed around the draw call itself, not the gap between draws.
        let t0 = Instant::now();
        terminal.draw(|f| ui::draw(f, app))?;
        let draw_ms = t0.elapsed().as_secs_f64() * 1000.0;
        app.push_frame_ms_sample(draw_ms);

        if app.should_quit {
            return Ok(());
        }

        tokio::select! {
            maybe_event = events.next() => {
                match maybe_event {
                    Some(Ok(Event::Key(key))) => {
                        if let Some(action) = key_to_action(key, app) {
                            dispatch_action(action, app, tx);
                        }
                    }
                    Some(Ok(Event::Mouse(mouse))) => {
                        app.set_mouse_pos(mouse.column, mouse.row);
                        if let Some(action) = mouse_to_action(mouse, app) {
                            let double_click = check_double_click(&action, &mut last_row_click);
                            dispatch_action(action, app, tx);
                            if double_click {
                                dispatch_action(Action::Confirm, app, tx);
                            }
                        }
                    }
                    Some(Err(err)) => {
                        tracing::warn!("terminal event stream error: {err}");
                    }
                    _ => {}
                }
            }
            Some(event) = rx.recv() => {
                for command in app.apply_event(event) {
                    handle_command(command, app, tx);
                }
            }
            _ = banner_tick.tick(), if app.banner_mode != BannerMode::Off => {
                let _ = app.update(Action::BannerTick);
            }
            _ = status_dismiss_tick.tick(), if app.status_clear_at.is_some() => {
                // Wake only; clear runs at the top of the next loop iteration.
            }
        }
    }
}
