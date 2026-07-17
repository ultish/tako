//! In-TUI settings editor: every config.toml field (except roots/excludes —
//! those stay on Workspace for list UX).

use super::App;
use crate::config::{self, BannerMode};
use crate::events::Command;
use crate::text_field;
use crate::ui::theme::ThemeName;

/// One editable settings row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingId {
    // scan
    ScanMaxDepth,
    ScanIgnore,
    // gradle
    GradleCommand,
    GradleTasksBuild,
    GradleTasksClean,
    GradleTasksPublish,
    GradleForceSnapshots,
    // skaffold
    SkaffoldCommand,
    SkaffoldProfile,
    SkaffoldDevArgs,
    SkaffoldDebugArgs,
    // git
    GitPullFfOnly,
    GitPullArgs,
    // cascade
    CascadeConsumerGradle,
    CascadeConsumerSkaffold,
    CascadeMaxParallel,
    // kube
    KubeEnabled,
    KubeContext,
    KubeNamespaces,
    KubeVersionSource,
    KubeCommand,
    // ui
    UiTheme,
    UiBannerMode,
}

impl SettingId {
    pub const ALL: &'static [SettingId] = &[
        SettingId::ScanMaxDepth,
        SettingId::ScanIgnore,
        SettingId::GradleCommand,
        SettingId::GradleTasksBuild,
        SettingId::GradleTasksClean,
        SettingId::GradleTasksPublish,
        SettingId::GradleForceSnapshots,
        SettingId::SkaffoldCommand,
        SettingId::SkaffoldProfile,
        SettingId::SkaffoldDevArgs,
        SettingId::SkaffoldDebugArgs,
        SettingId::GitPullFfOnly,
        SettingId::GitPullArgs,
        SettingId::CascadeConsumerGradle,
        SettingId::CascadeConsumerSkaffold,
        SettingId::CascadeMaxParallel,
        SettingId::KubeEnabled,
        SettingId::KubeContext,
        SettingId::KubeNamespaces,
        SettingId::KubeVersionSource,
        SettingId::KubeCommand,
        SettingId::UiTheme,
        SettingId::UiBannerMode,
    ];

    pub fn section(self) -> &'static str {
        match self {
            SettingId::ScanMaxDepth | SettingId::ScanIgnore => "scan",
            SettingId::GradleCommand
            | SettingId::GradleTasksBuild
            | SettingId::GradleTasksClean
            | SettingId::GradleTasksPublish
            | SettingId::GradleForceSnapshots => "gradle",
            SettingId::SkaffoldCommand
            | SettingId::SkaffoldProfile
            | SettingId::SkaffoldDevArgs
            | SettingId::SkaffoldDebugArgs => "skaffold",
            SettingId::GitPullFfOnly | SettingId::GitPullArgs => "git",
            SettingId::CascadeConsumerGradle
            | SettingId::CascadeConsumerSkaffold
            | SettingId::CascadeMaxParallel => "cascade",
            SettingId::KubeEnabled
            | SettingId::KubeContext
            | SettingId::KubeNamespaces
            | SettingId::KubeVersionSource
            | SettingId::KubeCommand => "kube",
            SettingId::UiTheme | SettingId::UiBannerMode => "ui",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            SettingId::ScanMaxDepth => "max_depth",
            SettingId::ScanIgnore => "ignore",
            SettingId::GradleCommand => "command",
            SettingId::GradleTasksBuild => "default_tasks_build",
            SettingId::GradleTasksClean => "default_tasks_clean",
            SettingId::GradleTasksPublish => "default_tasks_publish",
            SettingId::GradleForceSnapshots => "force_latest_snapshots",
            SettingId::SkaffoldCommand => "command",
            SettingId::SkaffoldProfile => "default_profile",
            SettingId::SkaffoldDevArgs => "dev_args",
            SettingId::SkaffoldDebugArgs => "debug_args",
            SettingId::GitPullFfOnly => "pull_ff_only",
            SettingId::GitPullArgs => "pull_args",
            SettingId::CascadeConsumerGradle => "default_consumer_gradle",
            SettingId::CascadeConsumerSkaffold => "default_consumer_skaffold",
            SettingId::CascadeMaxParallel => "max_parallel",
            SettingId::KubeEnabled => "enabled",
            SettingId::KubeContext => "context",
            SettingId::KubeNamespaces => "namespaces",
            SettingId::KubeVersionSource => "version_source",
            SettingId::KubeCommand => "command",
            SettingId::UiTheme => "theme",
            SettingId::UiBannerMode => "banner_mode",
        }
    }

    pub fn kind(self) -> SettingKind {
        match self {
            SettingId::GradleForceSnapshots
            | SettingId::GitPullFfOnly
            | SettingId::KubeEnabled => SettingKind::Bool,
            SettingId::ScanMaxDepth | SettingId::CascadeMaxParallel => SettingKind::Int,
            SettingId::UiTheme | SettingId::UiBannerMode => SettingKind::Cycle,
            _ => SettingKind::Text,
        }
    }

    pub fn help(self) -> &'static str {
        match self {
            SettingId::ScanMaxDepth => "Directory depth under each scan root",
            SettingId::ScanIgnore => "Comma-separated dir globs to skip while walking",
            SettingId::GradleCommand => "gradle binary on PATH or absolute path",
            SettingId::GradleTasksBuild => "Tasks for b / bulk build (comma-separated)",
            SettingId::GradleTasksClean => "Tasks for c / bulk clean",
            SettingId::GradleTasksPublish => "Tasks for p publish",
            SettingId::GradleForceSnapshots => "Init script on B/P consumer builds (SNAPSHOT refresh)",
            SettingId::SkaffoldCommand => "skaffold binary",
            SettingId::SkaffoldProfile => "Default -p profile (empty = none)",
            SettingId::SkaffoldDevArgs => "Extra args for skaffold dev",
            SettingId::SkaffoldDebugArgs => "Extra args for skaffold debug",
            SettingId::GitPullFfOnly => "git pull --ff-only (recommended)",
            SettingId::GitPullArgs => "Extra args after pull",
            SettingId::CascadeConsumerGradle => "Gradle tasks on cascade consumers",
            SettingId::CascadeConsumerSkaffold => "Skaffold subcommands on consumers (P)",
            SettingId::CascadeMaxParallel => "Max parallel consumer jobs after publish (1=serial)",
            SettingId::KubeEnabled => "Probe cluster for deployed versions (K key)",
            SettingId::KubeContext => "kubectl --context (empty = current)",
            SettingId::KubeNamespaces => "Namespaces to search (comma-separated)",
            SettingId::KubeVersionSource => "auto | label:… | image_tag | annotation:… | env:…",
            SettingId::KubeCommand => "kubectl binary",
            SettingId::UiTheme => "Color theme (also T key)",
            SettingId::UiBannerMode => "Top banner mode (also A key)",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingKind {
    Bool,
    Int,
    Text,
    Cycle,
}

/// Text overlay while editing a string/list setting.
#[derive(Debug, Clone)]
pub struct SettingsEditorState {
    pub id: SettingId,
    pub text: String,
    pub cursor: usize,
    pub error: Option<String>,
}

impl SettingsEditorState {
    pub fn new(id: SettingId, initial: String) -> Self {
        let cursor = initial.chars().count();
        Self {
            id,
            text: initial,
            cursor,
            error: None,
        }
    }

    pub fn insert_char(&mut self, c: char) {
        text_field::insert_char(&mut self.text, &mut self.cursor, c);
        self.error = None;
    }
    pub fn backspace(&mut self) {
        text_field::backspace(&mut self.text, &mut self.cursor);
        self.error = None;
    }
    pub fn delete_forward(&mut self) {
        text_field::delete_forward(&mut self.text, &mut self.cursor);
        self.error = None;
    }
    pub fn cursor_left(&mut self) {
        text_field::cursor_left(&mut self.cursor);
    }
    pub fn cursor_right(&mut self) {
        text_field::cursor_right(&self.text, &mut self.cursor);
    }
    pub fn cursor_home(&mut self) {
        text_field::cursor_home(&mut self.cursor);
    }
    pub fn cursor_end(&mut self) {
        text_field::cursor_end(&self.text, &mut self.cursor);
    }
    pub fn display_with_cursor(&self) -> String {
        text_field::display_with_cursor(&self.text, self.cursor)
    }
}

fn join_list(v: &[String]) -> String {
    v.join(", ")
}

fn split_list(s: &str) -> Vec<String> {
    s.split(',')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

impl App {
    pub fn setting_display_value(&self, id: SettingId) -> String {
        let c = &self.config;
        match id {
            SettingId::ScanMaxDepth => c.scan.max_depth.to_string(),
            SettingId::ScanIgnore => join_list(&c.scan.ignore),
            SettingId::GradleCommand => c.gradle.command.clone(),
            SettingId::GradleTasksBuild => join_list(&c.gradle.default_tasks_build),
            SettingId::GradleTasksClean => join_list(&c.gradle.default_tasks_clean),
            SettingId::GradleTasksPublish => join_list(&c.gradle.default_tasks_publish),
            SettingId::GradleForceSnapshots => bool_label(c.gradle.force_latest_snapshots),
            SettingId::SkaffoldCommand => c.skaffold.command.clone(),
            SettingId::SkaffoldProfile => {
                if c.skaffold.default_profile.is_empty() {
                    "(none)".into()
                } else {
                    c.skaffold.default_profile.clone()
                }
            }
            SettingId::SkaffoldDevArgs => join_list(&c.skaffold.dev_args),
            SettingId::SkaffoldDebugArgs => join_list(&c.skaffold.debug_args),
            SettingId::GitPullFfOnly => bool_label(c.git.pull_ff_only),
            SettingId::GitPullArgs => join_list(&c.git.pull_args),
            SettingId::CascadeConsumerGradle => join_list(&c.cascade.default_consumer_gradle),
            SettingId::CascadeConsumerSkaffold => join_list(&c.cascade.default_consumer_skaffold),
            SettingId::CascadeMaxParallel => c.cascade.max_parallel.to_string(),
            SettingId::KubeEnabled => bool_label(c.kube.enabled),
            SettingId::KubeContext => {
                if c.kube.context.is_empty() {
                    "(current)".into()
                } else {
                    c.kube.context.clone()
                }
            }
            SettingId::KubeNamespaces => {
                if c.kube.namespaces.is_empty() {
                    "(none)".into()
                } else {
                    join_list(&c.kube.namespaces)
                }
            }
            SettingId::KubeVersionSource => c.kube.version_source.clone(),
            SettingId::KubeCommand => c.kube.command.clone(),
            SettingId::UiTheme => match c.ui.theme {
                ThemeName::Dark => "dark".into(),
                ThemeName::Light => "light".into(),
            },
            SettingId::UiBannerMode => match c.ui.banner_mode {
                BannerMode::Wave => "wave".into(),
                BannerMode::Ms => "ms".into(),
                BannerMode::Fps => "fps".into(),
                BannerMode::Off => "off".into(),
            },
        }
    }

    pub(super) fn move_settings_selection(&mut self, delta: i32) {
        let len = SettingId::ALL.len() as i32;
        if len == 0 {
            return;
        }
        let cur = self.selected_setting as i32;
        self.selected_setting = (cur + delta).clamp(0, len - 1) as usize;
    }

    pub(super) fn clamp_settings_selection(&mut self) {
        if self.selected_setting >= SettingId::ALL.len() {
            self.selected_setting = SettingId::ALL.len().saturating_sub(1);
        }
    }

    /// Activate the focused setting: toggle bool, cycle enum, or open text editor.
    pub(super) fn activate_setting(&mut self) -> Vec<Command> {
        let Some(&id) = SettingId::ALL.get(self.selected_setting) else {
            return vec![];
        };
        match id.kind() {
            SettingKind::Bool => self.toggle_setting_bool(id),
            SettingKind::Cycle => self.cycle_setting(id),
            SettingKind::Int => {
                // Open as text for free edit; also support +/- via other keys.
                self.open_settings_editor(id);
                vec![]
            }
            SettingKind::Text => {
                self.open_settings_editor(id);
                vec![]
            }
        }
    }

    pub(super) fn nudge_setting_int(&mut self, delta: i32) -> Vec<Command> {
        let Some(&id) = SettingId::ALL.get(self.selected_setting) else {
            return vec![];
        };
        if id.kind() != SettingKind::Int {
            return vec![];
        }
        match id {
            SettingId::ScanMaxDepth => {
                let v = (self.config.scan.max_depth as i32 + delta).clamp(0, 32) as usize;
                self.config.scan.max_depth = v;
            }
            SettingId::CascadeMaxParallel => {
                let v = (self.config.cascade.max_parallel as i32 + delta).clamp(1, 64) as usize;
                self.config.cascade.max_parallel = v;
            }
            _ => return vec![],
        }
        self.persist_settings(&format!("{} = {}", id.key(), self.setting_display_value(id)))
    }

    fn toggle_setting_bool(&mut self, id: SettingId) -> Vec<Command> {
        match id {
            SettingId::GradleForceSnapshots => {
                self.config.gradle.force_latest_snapshots = !self.config.gradle.force_latest_snapshots;
            }
            SettingId::GitPullFfOnly => {
                self.config.git.pull_ff_only = !self.config.git.pull_ff_only;
            }
            SettingId::KubeEnabled => {
                self.config.kube.enabled = !self.config.kube.enabled;
                // Enabling with empty namespaces → seed "default" so probe can run.
                if self.config.kube.enabled && self.config.kube.namespaces.is_empty() {
                    self.config.kube.namespaces = vec!["default".into()];
                }
            }
            _ => return vec![],
        }
        let label = match id {
            SettingId::KubeEnabled => {
                if self.config.kube.enabled {
                    format!(
                        "kube enabled · ns={} · press K to probe",
                        self.config.kube.namespaces.join(",")
                    )
                } else {
                    "kube disabled".into()
                }
            }
            _ => format!(
                "{}.{} = {}",
                id.section(),
                id.key(),
                self.setting_display_value(id)
            ),
        };
        self.persist_settings(&label)
    }

    fn cycle_setting(&mut self, id: SettingId) -> Vec<Command> {
        match id {
            SettingId::UiTheme => {
                self.config.ui.theme = match self.config.ui.theme {
                    ThemeName::Dark => ThemeName::Light,
                    ThemeName::Light => ThemeName::Dark,
                };
                self.theme = crate::ui::theme::Theme::from_name(self.config.ui.theme);
            }
            SettingId::UiBannerMode => {
                self.config.ui.banner_mode = self.config.ui.banner_mode.next();
                self.banner_mode = self.config.ui.banner_mode;
            }
            _ => return vec![],
        }
        self.persist_settings(&format!(
            "{}.{} = {}",
            id.section(),
            id.key(),
            self.setting_display_value(id)
        ))
    }

    fn open_settings_editor(&mut self, id: SettingId) {
        let initial = match id {
            SettingId::SkaffoldProfile if self.config.skaffold.default_profile.is_empty() => {
                String::new()
            }
            SettingId::KubeContext if self.config.kube.context.is_empty() => String::new(),
            SettingId::KubeNamespaces if self.config.kube.namespaces.is_empty() => String::new(),
            _ => {
                let d = self.setting_display_value(id);
                if d == "(none)" || d == "(current)" {
                    String::new()
                } else {
                    d
                }
            }
        };
        self.settings_editor = Some(SettingsEditorState::new(id, initial));
    }

    pub(super) fn cancel_settings_editor(&mut self) {
        self.settings_editor = None;
    }

    pub(super) fn save_settings_editor(&mut self) -> Vec<Command> {
        let Some(ed) = self.settings_editor.as_ref() else {
            return vec![];
        };
        let id = ed.id;
        let raw = ed.text.trim().to_string();
        if let Err(err) = self.apply_setting_text(id, &raw) {
            if let Some(ed) = self.settings_editor.as_mut() {
                ed.error = Some(err);
            }
            return vec![];
        }
        self.settings_editor = None;
        self.persist_settings(&format!(
            "{}.{} = {}",
            id.section(),
            id.key(),
            self.setting_display_value(id)
        ))
    }

    fn apply_setting_text(&mut self, id: SettingId, raw: &str) -> Result<(), String> {
        let raw = raw.trim();
        match id {
            SettingId::ScanMaxDepth => {
                let v: usize = raw
                    .parse()
                    .map_err(|_| "max_depth must be a number 0–32".to_string())?;
                if v > 32 {
                    return Err("max_depth max is 32".into());
                }
                self.config.scan.max_depth = v;
            }
            SettingId::ScanIgnore => {
                self.config.scan.ignore = split_list(raw);
            }
            SettingId::GradleCommand => {
                if raw.is_empty() {
                    return Err("command required".into());
                }
                self.config.gradle.command = raw.to_string();
            }
            SettingId::GradleTasksBuild => {
                let v = split_list(raw);
                if v.is_empty() {
                    return Err("at least one task".into());
                }
                self.config.gradle.default_tasks_build = v;
            }
            SettingId::GradleTasksClean => {
                let v = split_list(raw);
                if v.is_empty() {
                    return Err("at least one task".into());
                }
                self.config.gradle.default_tasks_clean = v;
            }
            SettingId::GradleTasksPublish => {
                let v = split_list(raw);
                if v.is_empty() {
                    return Err("at least one task".into());
                }
                self.config.gradle.default_tasks_publish = v;
            }
            SettingId::SkaffoldCommand => {
                if raw.is_empty() {
                    return Err("command required".into());
                }
                self.config.skaffold.command = raw.to_string();
            }
            SettingId::SkaffoldProfile => {
                self.config.skaffold.default_profile = raw.to_string();
            }
            SettingId::SkaffoldDevArgs => {
                self.config.skaffold.dev_args = split_list(raw);
            }
            SettingId::SkaffoldDebugArgs => {
                self.config.skaffold.debug_args = split_list(raw);
            }
            SettingId::GitPullArgs => {
                self.config.git.pull_args = split_list(raw);
            }
            SettingId::CascadeConsumerGradle => {
                let v = split_list(raw);
                if v.is_empty() {
                    return Err("at least one task".into());
                }
                self.config.cascade.default_consumer_gradle = v;
            }
            SettingId::CascadeConsumerSkaffold => {
                self.config.cascade.default_consumer_skaffold = split_list(raw);
            }
            SettingId::CascadeMaxParallel => {
                let v: usize = raw
                    .parse()
                    .map_err(|_| "max_parallel must be a number ≥ 1".to_string())?;
                if v == 0 {
                    return Err("max_parallel min is 1".into());
                }
                self.config.cascade.max_parallel = v.min(64);
            }
            SettingId::KubeContext => {
                self.config.kube.context = raw.to_string();
            }
            SettingId::KubeNamespaces => {
                self.config.kube.namespaces = split_list(raw);
            }
            SettingId::KubeVersionSource => {
                if raw.is_empty() {
                    return Err("version_source required".into());
                }
                self.config.kube.version_source = raw.to_string();
            }
            SettingId::KubeCommand => {
                if raw.is_empty() {
                    return Err("command required".into());
                }
                self.config.kube.command = raw.to_string();
            }
            _ => return Err("not a text setting".into()),
        }
        Ok(())
    }

    fn persist_settings(&mut self, status: &str) -> Vec<Command> {
        if let Err(err) = config::save(&self.config_path, &self.config) {
            self.set_ephemeral_status(format!("failed to save config: {err}"));
            return vec![];
        }
        self.set_ephemeral_status(status);
        // Changing scan depth/ignore should rescan when roots exist.
        if !self.config.scan.roots.is_empty()
            && matches!(
                SettingId::ALL.get(self.selected_setting),
                Some(SettingId::ScanMaxDepth | SettingId::ScanIgnore)
            )
        {
            return vec![Command::ScanWorkspace];
        }
        vec![]
    }
}

fn bool_label(v: bool) -> String {
    if v {
        "true".into()
    } else {
        "false".into()
    }
}
