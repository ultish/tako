//! Config load/save under `~/.config/tako/` (Linux + macOS — not Application Support).

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::ui::theme::ThemeName;

/// Top banner animation mode — stored under `[ui].banner_mode` and cycled with `A`.
///
/// Diagnostic modes share the same paint samples: **`Ms`** shows wall time per
/// draw; **`Fps`** shows the inverse capacity (`1000 / ms`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BannerMode {
    #[default]
    Wave,
    /// Paint cost in milliseconds per `terminal.draw`.
    Ms,
    /// Instantaneous paint capacity (`1000 / ms`), not screen refresh rate.
    Fps,
    Off,
}

impl BannerMode {
    pub fn next(self) -> Self {
        match self {
            BannerMode::Wave => BannerMode::Ms,
            BannerMode::Ms => BannerMode::Fps,
            BannerMode::Fps => BannerMode::Off,
            BannerMode::Off => BannerMode::Wave,
        }
    }
}

/// Appearance / TUI preferences under `[ui]`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UiConfig {
    /// Color theme (`dark` | `light`). Default `dark`.
    #[serde(default)]
    pub theme: ThemeName,
    /// Top banner animation (`wave` | `ms` | `fps` | `off`). Default `wave`.
    #[serde(default)]
    pub banner_mode: BannerMode,
}

fn default_max_depth() -> usize {
    6
}

fn default_ignore() -> Vec<String> {
    vec![
        "**/.git/**".into(),
        "**/build/**".into(),
        "**/node_modules/**".into(),
    ]
}

/// Filesystem scan settings under `[scan]`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScanConfig {
    /// Root folders to walk for project discovery (M1).
    #[serde(default)]
    pub roots: Vec<String>,
    /// Maximum directory depth relative to each root (root = depth 0).
    #[serde(default = "default_max_depth")]
    pub max_depth: usize,
    /// Glob-style ignore patterns (v1 matches path components for common dirs).
    #[serde(default = "default_ignore")]
    pub ignore: Vec<String>,
    /// Projects to drop from inventory after discovery (name, path fragment,
    /// absolute path, or simple `*` / `**` glob). Does not skip walking parents.
    #[serde(default)]
    pub exclude: Vec<String>,
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            roots: Vec::new(),
            max_depth: default_max_depth(),
            ignore: default_ignore(),
            exclude: Vec::new(),
        }
    }
}

fn default_gradle_command() -> String {
    "gradle".into()
}

fn default_true() -> bool {
    true
}

fn default_tasks_build() -> Vec<String> {
    vec!["build".into()]
}

fn default_tasks_publish() -> Vec<String> {
    vec!["publish".into()]
}

fn default_tasks_clean() -> Vec<String> {
    vec!["clean".into()]
}

fn default_skaffold_command() -> String {
    "skaffold".into()
}

fn default_consumer_gradle() -> Vec<String> {
    vec!["build".into()]
}

fn default_consumer_skaffold() -> Vec<String> {
    vec!["delete".into(), "run".into()]
}

fn default_max_parallel() -> usize {
    5
}

/// Cascade recipe defaults under `[cascade]` (M4).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CascadeConfig {
    /// Gradle tasks run on each consumer after the source publish.
    #[serde(default = "default_consumer_gradle")]
    pub default_consumer_gradle: Vec<String>,
    /// Skaffold subcommands run in order when the consumer has a skaffold file
    /// (default: `delete` then `run` — non-interactive redeploy).
    #[serde(default = "default_consumer_skaffold")]
    pub default_consumer_skaffold: Vec<String>,
    /// Max concurrent cascade jobs after the source publish finishes.
    /// Source publish always runs alone first. Per-project steps (build then
    /// skaffold) stay ordered; different consumers may run in parallel.
    /// `1` = fully sequential. Default `5`.
    #[serde(default = "default_max_parallel")]
    pub max_parallel: usize,
}

impl Default for CascadeConfig {
    fn default() -> Self {
        Self {
            default_consumer_gradle: default_consumer_gradle(),
            default_consumer_skaffold: default_consumer_skaffold(),
            max_parallel: default_max_parallel(),
        }
    }
}

/// Gradle CLI settings under `[gradle]`.
///
/// **Always `gradle` on PATH** (or an absolute override) — never `./gradlew`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GradleConfig {
    /// Binary name or absolute path. Default `"gradle"`.
    #[serde(default = "default_gradle_command")]
    pub command: String,
    #[serde(default = "default_tasks_build")]
    pub default_tasks_build: Vec<String>,
    #[serde(default = "default_tasks_clean")]
    pub default_tasks_clean: Vec<String>,
    #[serde(default = "default_tasks_publish")]
    pub default_tasks_publish: Vec<String>,
    /// When true (default), cascade **B**/**P** consumer gradle steps inject an
    /// init script that sets `cacheChangingModulesFor 0` so just-published
    /// Nexus SNAPSHOTs re-resolve without `--refresh-dependencies`.
    #[serde(default = "default_true")]
    pub force_latest_snapshots: bool,
}

impl Default for GradleConfig {
    fn default() -> Self {
        Self {
            command: default_gradle_command(),
            default_tasks_build: default_tasks_build(),
            default_tasks_clean: default_tasks_clean(),
            default_tasks_publish: default_tasks_publish(),
            force_latest_snapshots: true,
        }
    }
}

/// Skaffold CLI settings under `[skaffold]`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkaffoldConfig {
    #[serde(default = "default_skaffold_command")]
    pub command: String,
    #[serde(default)]
    pub default_profile: String,
    #[serde(default)]
    pub dev_args: Vec<String>,
    #[serde(default)]
    pub debug_args: Vec<String>,
}

impl Default for SkaffoldConfig {
    fn default() -> Self {
        Self {
            command: default_skaffold_command(),
            default_profile: String::new(),
            dev_args: Vec::new(),
            debug_args: Vec::new(),
        }
    }
}

/// Git CLI settings under `[git]`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitConfig {
    /// Prefer `git pull --ff-only` so tako never invents merge commits.
    #[serde(default = "default_true")]
    pub pull_ff_only: bool,
    #[serde(default)]
    pub pull_args: Vec<String>,
}

impl Default for GitConfig {
    fn default() -> Self {
        Self {
            pull_ff_only: true,
            pull_args: Vec::new(),
        }
    }
}

fn default_kubectl() -> String {
    "kubectl".into()
}

fn default_version_source() -> String {
    // Try version labels, then semver-ish image tags; Skaffold hash tags → "live".
    "auto".into()
}

/// Kubernetes read-only settings under `[kube]` (phase 2 — deployed versions).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KubeConfig {
    /// When true and `namespaces` non-empty, **K** probes the cluster.
    #[serde(default)]
    pub enabled: bool,
    /// `kubectl --context` (empty = current context).
    #[serde(default)]
    pub context: String,
    /// Namespaces to search for Deployments (matched by project leaf name).
    #[serde(default)]
    pub namespaces: Vec<String>,
    /// How to read the version: `label:…`, `annotation:…`, `image_tag`, `env:…`.
    #[serde(default = "default_version_source")]
    pub version_source: String,
    /// kubectl binary. Default `"kubectl"`.
    #[serde(default = "default_kubectl")]
    pub command: String,
}

impl Default for KubeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            context: String::new(),
            namespaces: Vec::new(),
            version_source: default_version_source(),
            command: default_kubectl(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Config {
    #[serde(default)]
    pub scan: ScanConfig,
    #[serde(default)]
    pub gradle: GradleConfig,
    #[serde(default)]
    pub skaffold: SkaffoldConfig,
    #[serde(default)]
    pub git: GitConfig,
    /// Cascade recipe defaults (`default_consumer_gradle` / skaffold).
    #[serde(default)]
    pub cascade: CascadeConfig,
    /// Phase 2 cluster version compare (`kubectl` read-only).
    #[serde(default)]
    pub kube: KubeConfig,
    /// TUI appearance (`theme`, `banner_mode`). Absent section → defaults.
    #[serde(default)]
    pub ui: UiConfig,
}

/// `~/.config/tako/`, constructed manually on both macOS and Linux rather than via a
/// crate's "native" platform config dir (which returns `~/Library/Application Support`
/// on macOS as of recent `dirs`/`directories` releases).
pub fn config_dir() -> AppResult<PathBuf> {
    let home = dirs::home_dir()
        .ok_or_else(|| AppError::Config("could not determine home directory".into()))?;
    Ok(home.join(".config").join("tako"))
}

/// `~/.config/tako/cache/` (created on demand by callers).
#[allow(dead_code)] // public config-dir helper for future tools / tests
pub fn cache_dir() -> AppResult<PathBuf> {
    Ok(config_dir()?.join("cache"))
}

/// Default workspace inventory cache path: `~/.config/tako/cache/workspace.json`.
#[allow(dead_code)] // public helper; TUI uses `workspace_cache_path_in` for --config-dir
pub fn workspace_cache_path() -> AppResult<PathBuf> {
    Ok(cache_dir()?.join("workspace.json"))
}

/// Workspace cache path for a given config directory (supports `--config-dir`).
pub fn workspace_cache_path_in(config_dir: &std::path::Path) -> PathBuf {
    config_dir.join("cache").join("workspace.json")
}

/// Expand a leading `~/` (or bare `~`) using the home directory.
/// Non-tilde paths are returned as-is (`PathBuf::from`).
pub fn expand_user_path(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    if path == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }
    PathBuf::from(path)
}

pub fn load(path: &PathBuf) -> AppResult<Config> {
    if !path.exists() {
        return Ok(Config::default());
    }
    let contents = fs::read_to_string(path)?;
    let config: Config = toml::from_str(&contents)?;
    Ok(config)
}

pub fn save(path: &PathBuf, config: &Config) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let contents = toml::to_string_pretty(config)?;
    fs::write(path, contents)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_toml() {
        let config = Config {
            scan: ScanConfig {
                roots: vec!["/Users/you/Developer".into()],
                max_depth: 4,
                ignore: vec!["**/build/**".into()],
                exclude: vec!["legacy-api".into()],
            },
            gradle: GradleConfig {
                command: "gradle".into(),
                default_tasks_build: vec!["build".into()],
                default_tasks_clean: vec!["clean".into()],
                default_tasks_publish: vec!["publish".into()],
                force_latest_snapshots: true,
            },
            skaffold: SkaffoldConfig {
                command: "skaffold".into(),
                default_profile: String::new(),
                dev_args: vec![],
                debug_args: vec!["--port-forward".into()],
            },
            git: GitConfig {
                pull_ff_only: true,
                pull_args: vec![],
            },
            cascade: CascadeConfig {
                default_consumer_gradle: vec!["build".into()],
                default_consumer_skaffold: vec!["delete".into(), "run".into()],
                max_parallel: 5,
            },
            kube: KubeConfig {
                enabled: true,
                context: "docker-desktop".into(),
                namespaces: vec!["dev".into()],
                version_source: "label:app.kubernetes.io/version".into(),
                command: "kubectl".into(),
            },
            ui: UiConfig {
                theme: ThemeName::Light,
                banner_mode: BannerMode::Fps,
            },
        };
        let serialized = toml::to_string_pretty(&config).expect("serialize");
        let deserialized: Config = toml::from_str(&serialized).expect("deserialize");
        assert_eq!(config, deserialized);
    }

    #[test]
    fn gradle_git_sections_default() {
        let config: Config = toml::from_str("").expect("empty");
        assert_eq!(config.gradle.command, "gradle");
        assert_eq!(config.gradle.default_tasks_build, vec!["build"]);
        assert!(config.gradle.force_latest_snapshots);
        assert!(config.git.pull_ff_only);
        assert_eq!(config.skaffold.command, "skaffold");
        assert_eq!(config.cascade.default_consumer_gradle, vec!["build"]);
        assert_eq!(
            config.cascade.default_consumer_skaffold,
            vec!["delete", "run"]
        );
        assert_eq!(config.cascade.max_parallel, 5);
    }

    #[test]
    fn cascade_section_round_trips() {
        let config: Config = toml::from_str(
            r#"
            [cascade]
            default_consumer_gradle = ["assemble"]
            default_consumer_skaffold = ["dev"]
            max_parallel = 3
            "#,
        )
        .expect("deserialize");
        assert_eq!(config.cascade.default_consumer_gradle, vec!["assemble"]);
        assert_eq!(config.cascade.default_consumer_skaffold, vec!["dev"]);
        assert_eq!(config.cascade.max_parallel, 3);
    }

    #[test]
    fn ui_section_round_trips() {
        let config: Config = toml::from_str(
            r#"
            [ui]
            theme = "light"
            banner_mode = "off"

            [scan]
            roots = ["/tmp/services"]
            "#,
        )
        .expect("deserialize");
        assert_eq!(config.ui.theme, ThemeName::Light);
        assert_eq!(config.ui.banner_mode, BannerMode::Off);
        assert_eq!(config.scan.roots, vec!["/tmp/services"]);
        assert_eq!(config.scan.max_depth, 6, "absent max_depth defaults to 6");
        assert!(
            config.scan.ignore.iter().any(|p| p.contains(".git")),
            "default ignore should include .git"
        );
        let serialized = toml::to_string_pretty(&config).expect("serialize");
        assert!(serialized.contains("theme = \"light\""));
        assert!(serialized.contains("banner_mode = \"off\""));
    }

    #[test]
    fn absent_sections_use_defaults() {
        let config: Config = toml::from_str("").expect("deserialize empty");
        assert_eq!(config, Config::default());
        assert!(config.scan.roots.is_empty());
        assert_eq!(config.scan.max_depth, 6);
        assert!(!config.scan.ignore.is_empty());
        assert_eq!(config.ui.theme, ThemeName::Dark);
        assert_eq!(config.ui.banner_mode, BannerMode::Wave);
    }

    #[test]
    fn scan_max_depth_and_ignore_round_trip() {
        let config: Config = toml::from_str(
            r#"
            [scan]
            roots = ["/a"]
            max_depth = 3
            ignore = ["**/target/**"]
            exclude = ["legacy-api", "**/sandbox/**"]
            "#,
        )
        .expect("deserialize");
        assert_eq!(config.scan.max_depth, 3);
        assert_eq!(config.scan.ignore, vec!["**/target/**"]);
        assert_eq!(
            config.scan.exclude,
            vec!["legacy-api".to_string(), "**/sandbox/**".into()]
        );
    }

    #[test]
    fn load_missing_file_returns_default() {
        let path = std::env::temp_dir().join("tako-test-nonexistent-config.toml");
        let _ = fs::remove_file(&path);
        let config = load(&path).expect("load should not fail on missing file");
        assert_eq!(config, Config::default());
    }

    #[test]
    fn save_then_load_round_trips() {
        let path = std::env::temp_dir().join(format!(
            "tako-test-config-{}.toml",
            std::process::id()
        ));
        let config = Config {
            scan: ScanConfig {
                roots: vec!["/a".into(), "/b".into()],
                max_depth: 6,
                ignore: default_ignore(),
                exclude: vec![],
            },
            gradle: GradleConfig::default(),
            skaffold: SkaffoldConfig::default(),
            git: GitConfig::default(),
            cascade: CascadeConfig::default(),
            kube: KubeConfig::default(),
            ui: UiConfig {
                theme: ThemeName::Light,
                banner_mode: BannerMode::Ms,
            },
        };
        save(&path, &config).expect("save");
        let loaded = load(&path).expect("load");
        assert_eq!(config, loaded);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn config_dir_uses_dot_config_on_this_platform() {
        let dir = config_dir().expect("home dir resolvable in test env");
        assert!(dir.ends_with(".config/tako"));
    }

    #[test]
    fn workspace_cache_path_in_config_dir() {
        let p = workspace_cache_path_in(std::path::Path::new("/tmp/tako-cfg"));
        assert_eq!(
            p,
            PathBuf::from("/tmp/tako-cfg/cache/workspace.json")
        );
    }

    #[test]
    fn banner_mode_cycles_wave_ms_fps_off() {
        assert_eq!(BannerMode::Wave.next(), BannerMode::Ms);
        assert_eq!(BannerMode::Ms.next(), BannerMode::Fps);
        assert_eq!(BannerMode::Fps.next(), BannerMode::Off);
        assert_eq!(BannerMode::Off.next(), BannerMode::Wave);
    }

    #[test]
    fn expand_user_path_resolves_tilde() {
        let home = dirs::home_dir().expect("home");
        assert_eq!(expand_user_path("~/foo"), home.join("foo"));
        assert_eq!(expand_user_path("~"), home);
        assert_eq!(expand_user_path("/abs"), PathBuf::from("/abs"));
    }
}
