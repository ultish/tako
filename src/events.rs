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

    /// Git remote lag + Nexus version probe finished (**r** stats).
    RepoStatsFinished {
        git: Vec<(std::path::PathBuf, crate::git::GitInfo)>,
        nexus: crate::nexus::NexusBatch,
        error: Option<String>,
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
    /// Cycle help page: workflows ↔ keys (`Tab` while help is open).
    CycleHelpPage,
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
    /// Mouse: focus the scan-roots list (click anywhere on that panel).
    FocusWorkspaceRoots,
    /// Mouse: focus the project-excludes list (click anywhere on that panel).
    FocusWorkspaceExcludes,
    /// Click an exclude row (absolute index into `config.scan.exclude`).
    SelectExcludeRow(usize),
    /// Mouse: focus the jobs list panel.
    FocusJobsList,
    /// Mouse: focus the job log panel.
    FocusJobsLog,
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

    // ── Project exec (shared) ──────────────────────────────────────────────
    /// Gradle `build` (`b`); bulk when multi-select non-empty.
    Build,
    /// Gradle build forcing latest SNAPSHOT re-resolve (`B`).
    BuildForce,
    /// Gradle `clean` (`c`); bulk when multi-select non-empty.
    Clean,
    /// Gradle `publish` to Nexus (`p`).
    Publish,
    /// Skaffold **delete → run** for this project (`u`).
    SkaffoldRedeploy,
    /// Skaffold `delete` only (`x`).
    SkaffoldDelete,
    /// `git pull` (`G`); bulk: unique git roots among multi-selected.
    GitPull,
    /// Toggle multi-select (`Space`).
    ToggleMultiSelect,
    /// Cancel the focused running job on the Jobs screen.
    #[allow(dead_code)]
    CancelJob,
    /// Toggle Jobs list vs log pane focus (`Tab` on Jobs).
    ToggleJobsFocus,

    // ── Multi-step recipes ─────────────────────────────────────────────────
    /// **U**: update full dependent tree (build + skaffold delete→run unless Argo).
    UpdateDependents,
    /// Confirm and start the open plan (y/Enter).
    ConfirmCascadePlan,
    /// Dismiss the plan without running (n/Esc).
    CancelCascadePlan,

    // ── Stats / scan ───────────────────────────────────────────────────────
    /// **r**: refresh stats (cluster probe when kube enabled; kind-aware status).
    RefreshStats,
    /// **K**: probe deployed versions from the cluster (same as part of stats).
    RefreshDeployedVersions,
    /// **w**: rescan workspace roots.
    // (wired as Action::Refresh historically)
    /// Toggle "show only drift" filter (`f`).
    ToggleDriftFilter,
    /// Hide projects from inventory (`-`).
    ExcludeSelectedProjects,

    // ── Version bump ───────────────────────────────────────────────────────
    /// **v**: bump **this** project's version.
    OpenBumpVersion,
    /// **V**: bump versions of projects that depend on this (manual).
    OpenBumpDependents,
    CycleBumpKind,
    SetBumpKind(crate::version_bump::BumpKind),
    ConfirmBumpDependents,
    CancelBumpDependents,

    // ── Browser helpers ────────────────────────────────────────────────────
    /// Who needs this? (`i`).
    ToggleImpact,
    /// Toggle favorite pin for cursor project (`F`).
    ToggleFavorite,
    /// Start / focus project name filter (`/`).
    StartProjectFilter,
    /// Type into project filter.
    ProjectFilterChar(char),
    ProjectFilterBackspace,
    /// Leave filter mode but keep query (Esc).
    ClearProjectFilter,
    /// Confirm Argo-guarded skaffold action (y).
    ConfirmArgoSkaffold,
    /// Cancel Argo-guarded skaffold (n).
    CancelArgoSkaffold,

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
    /// Probe git remotes (fetch + lag) and Nexus maven-metadata (**r** stats).
    ProbeRepoStats { fetch_git: bool },
}
