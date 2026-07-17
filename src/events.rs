//! User actions, background events, and side-effect commands for the Elm loop.

use std::path::PathBuf;

use crate::scan::DiscoveredProject;

/// Events reported from background tasks back to the render loop.
/// Never constructed on the render loop thread itself.
#[derive(Debug)]
pub enum AppEvent {
    /// Scan worker has begun (optional; UI often sets sticky status on Command).
    ScanStarted,
    /// Workspace walk finished (success or soft failure via `error`).
    ScanFinished {
        projects: Vec<DiscoveredProject>,
        error: Option<String>,
    },
    /// Optional progress tick for long walks (message is free-form).
    #[allow(dead_code)]
    ScanProgress { message: String },

    /// Child process has started; mark job Running.
    JobStarted { id: u64 },
    /// One line of stdout or stderr from a running job.
    JobLog { id: u64, line: String },
    /// Child exited (or was cancelled / failed to spawn).
    JobFinished {
        id: u64,
        ok: bool,
        /// True when the user cancelled via Esc / CancelJob.
        cancelled: bool,
        summary: String,
    },

    /// Phase 2 kube probe finished (read-only deployed versions).
    KubeProbeFinished {
        batch: crate::kube::ProbeBatch,
    },
}

/// User- or timer-driven state transitions, dispatched by the render loop into
/// `App::update`. `PartialEq` is used to detect a double-click (two `SelectRow`
/// clicks on the same row within a short window) in main.rs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Request quit: opens a confirmation dialog (`q`). Confirm with y/Enter.
    Quit,
    /// Leave immediately without a dialog (Ctrl-c).
    ForceQuit,
    /// User confirmed the quit dialog.
    ConfirmQuit,
    /// User dismissed the quit dialog.
    CancelQuit,
    MoveSelectionUp,
    MoveSelectionDown,
    /// Mouse click on a rendered list row: selects it by absolute index.
    SelectRow(usize),
    Confirm,
    Back,
    /// Manual refresh of the current screen (M1: rescan workspace).
    Refresh,
    /// Advance braille banner animation frame (timer-driven).
    BannerTick,
    /// Cycles the top banner: wave → `ms/frame` → paint-capacity `fps` → off (`A`).
    CycleBannerMode,
    /// Cycles the color theme (`T`); persisted to `[ui].theme`.
    CycleTheme,
    /// Toggle the global keybind help overlay (`?`).
    ToggleHelp,
    /// Dismiss the startup splash.
    DismissSplash,
    /// Top-level view switcher (digits 1/2/3).
    SwitchToProjects,
    SwitchToJobs,
    SwitchToWorkspace,
    /// Settings tab (`4`).
    SwitchToSettings,

    // ── M1.5 workspace roots ────────────────────────────────────────────────
    /// Open the add-root path editor (`n` on Workspace when roots focused).
    StartAddRoot,
    /// Open the edit-root path editor (`e` on Workspace when roots focused).
    StartEditRoot,
    /// Open the delete-root confirm dialog (`z` on Workspace when roots focused).
    StartDeleteRoot,
    /// Confirm delete-root dialog.
    ConfirmDeleteRoot,
    /// Dismiss delete-root dialog.
    CancelDeleteRoot,
    /// Open add-exclude pattern editor (`n` when excludes focused).
    StartAddExclude,
    /// Open edit-exclude pattern editor (`e` when excludes focused).
    StartEditExclude,
    /// Open delete-exclude confirm (`z` when excludes focused).
    StartDeleteExclude,
    /// Confirm delete-exclude dialog.
    ConfirmDeleteExclude,
    /// Dismiss delete-exclude dialog.
    CancelDeleteExclude,
    /// Toggle Workspace focus between scan roots and excludes (`Tab`).
    ToggleWorkspacePanel,
    /// Click an exclude row (absolute index into `config.scan.exclude`).
    SelectExcludeRow(usize),
    /// Dismiss the root path editor without saving.
    CancelRootEditor,
    /// Insert a character into the root path field.
    RootEditorChar(char),
    RootEditorBackspace,
    RootEditorDelete,
    RootEditorCursorLeft,
    RootEditorCursorRight,
    RootEditorCursorHome,
    RootEditorCursorEnd,
    /// Save the root path editor (Enter).
    SaveRoot,

    // ── M3 single-project exec (project browser focused) ───────────────────
    /// Gradle `build` on the selected project (`b`); bulk when multi-select non-empty.
    Build,
    /// Open `publish_and_rebuild_consumers` plan: publish cursor, then rebuild
    /// graph dependents (`B` / detail page). Sequential; no skaffold.
    BuildWithDeps,
    /// Gradle `clean` (`c`); bulk when multi-select non-empty.
    Clean,
    /// Gradle `publish` (`p`).
    Publish,
    /// `skaffold dev` (`d`).
    SkaffoldDev,
    /// `skaffold debug` (`D`).
    SkaffoldDebug,
    /// `skaffold delete` (`x`).
    SkaffoldDelete,
    /// `skaffold run` (`u`).
    SkaffoldRun,
    /// `git pull` (ff-only default) for the selected project's git root (`G`);
    /// bulk: unique `git_root`s among multi-selected projects.
    GitPull,
    /// Toggle multi-select on the cursor project (`Space` on project browser).
    ToggleMultiSelect,
    /// Cancel the focused running job on the Jobs screen (also via Esc → Back).
    #[allow(dead_code)]
    CancelJob,
    /// Toggle Jobs list vs log pane focus (`Tab` on Jobs screen).
    ToggleJobsFocus,

    // ── M4 cascade recipes ─────────────────────────────────────────────────
    /// Open `publish_and_redeploy_consumers` plan for the selected producer (`P`).
    CascadePublish,
    /// Confirm and start the open cascade plan (y/Enter).
    ConfirmCascadePlan,
    /// Dismiss the cascade plan without running (n/Esc).
    CancelCascadePlan,

    // ── Phase 2 kube ───────────────────────────────────────────────────────
    /// Refresh deployed versions from the cluster (`K`).
    RefreshDeployedVersions,
    /// Toggle "show only drift" filter on the project browser (`f`).
    ToggleDriftFilter,
    /// Add cursor (or multi-selected) project(s) to `[scan].exclude` (`-`).
    ExcludeSelectedProjects,

    // ── Settings screen ────────────────────────────────────────────────────
    /// Activate / toggle / open editor for the focused setting.
    ActivateSetting,
    /// Nudge int setting left/right.
    NudgeSettingLeft,
    NudgeSettingRight,
    /// Click a settings row.
    SelectSettingRow(usize),
    /// Save settings text editor.
    SaveSettingsEditor,
    /// Cancel settings text editor.
    CancelSettingsEditor,
    SettingsEditorChar(char),
    SettingsEditorBackspace,
    SettingsEditorDelete,
    SettingsEditorCursorLeft,
    SettingsEditorCursorRight,
    SettingsEditorCursorHome,
    SettingsEditorCursorEnd,
}

/// Side effects the reducer wants performed outside of itself.
/// `App::update` stays synchronous and returns `Command`s for main to run.
#[derive(Debug, Clone)]
pub enum Command {
    /// Walk `[scan].roots` off the render loop (`spawn_blocking`).
    ScanWorkspace,
    /// Run `gradle -p <path> <tasks…>` (PATH binary only — never `./gradlew`).
    RunGradle {
        id: u64,
        path: PathBuf,
        tasks: Vec<String>,
        #[allow(dead_code)] // retained for logging / future job labels in main
        project_name: String,
        /// Inject tako's SNAPSHOT-refresh init script (`--init-script`).
        force_latest_snapshots: bool,
    },
    /// Run `skaffold <args…> -f <file>` with cwd = file parent.
    RunSkaffold {
        id: u64,
        /// Project directory (for display / status chip).
        #[allow(dead_code)]
        path: PathBuf,
        /// Subcommand first, then extras (profile etc.). `-f` is added by the runner.
        args: Vec<String>,
        skaffold_file: PathBuf,
        #[allow(dead_code)]
        project_name: String,
    },
    /// `git -C <git_root> pull [--ff-only]`.
    GitPull {
        id: u64,
        git_root: PathBuf,
        #[allow(dead_code)]
        project_name: String,
        /// Project path used for status-chip updates (may differ from git_root
        /// in a monorepo).
        #[allow(dead_code)]
        project_path: PathBuf,
        ff_only: bool,
    },
    /// Kill the process group for a running job.
    CancelJob { id: u64 },
    /// Probe cluster Deployments for deployed versions (phase 2).
    ProbeKubeVersions,
}
