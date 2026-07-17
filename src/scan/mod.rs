//! Filesystem discovery: skaffold entrypoints + publishable Gradle modules.
//!
//! Walks configured scan roots (depth-limited, with ignore rules) and returns
//! one [`DiscoveredProject`] per inventory unit:
//! - **one row per `skaffold.yaml` / `skaffold.yml` directory** (monorepo multi-skaffold)
//! - **library / avro** dirs with `build.gradle(.kts)` and no skaffold
//!
//! Also provides workspace-cache load/save for the TUI inventory hydrate path.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::app::{ProjectKind, ProjectRow};
use crate::error::{AppError, AppResult};
use crate::git::{self, GitStatusCache};
use crate::gradle::model::{DepReq, Produces};
use crate::gradle::catalog::CatalogRegistry;
use crate::gradle::parse::extract_for_project;

/// Placeholder version when Gradle does not declare one.
pub const UNKNOWN_VERSION: &str = "—";

/// Placeholder branch when not a git checkout / git fails.
pub const UNKNOWN_BRANCH: &str = "—";

/// One discovered inventory unit (service / library / avro).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredProject {
    /// Stable-ish id: path relative to the scan root that found this project.
    #[serde(default)]
    pub id: String,
    pub path: PathBuf,
    pub git_root: Option<PathBuf>,
    /// Relative display name (same as `id` for v1).
    pub name: String,
    pub kind: ProjectKind,
    /// Local Gradle version, or [`UNKNOWN_VERSION`].
    pub version: String,
    /// Git branch, or [`UNKNOWN_BRANCH`].
    pub branch: String,
    pub git_dirty: bool,
    pub has_skaffold: bool,
    /// Absolute paths to skaffold.yaml / skaffold.yml in the project dir.
    #[serde(default)]
    pub skaffold_files: Vec<PathBuf>,
    /// Nearest Gradle settings root, or the project dir when it has a build file.
    #[serde(default)]
    pub gradle_root: Option<PathBuf>,
    /// First path segment under the scan root (UI group key).
    #[serde(default)]
    pub folder_group: String,
    /// Last job / scan status chip text (persisted for cache hydrate).
    #[serde(default = "default_status")]
    pub status: String,
    /// Static dependency requirements (from build scripts + catalog).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends: Vec<DepReq>,
    /// Published Maven coordinates (group:name + version).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub produces: Vec<Produces>,
}

fn default_status() -> String {
    "idle".into()
}

impl DiscoveredProject {
    /// Convert into a browser row.
    pub fn to_row(&self) -> ProjectRow {
        use crate::app::Drift;
        let drift = if self.has_skaffold || matches!(self.kind, ProjectKind::Service) {
            Drift::NotProbed
        } else {
            Drift::NotApplicable
        };
        ProjectRow {
            name: self.name.clone(),
            path: self.path.clone(),
            kind: self.kind,
            version: self.version.clone(),
            branch: self.branch.clone(),
            has_skaffold: self.has_skaffold,
            git_dirty: self.git_dirty,
            status: self.status.clone(),
            folder_group: if self.folder_group.is_empty() {
                None
            } else {
                Some(self.folder_group.clone())
            },
            git_root: self.git_root.clone(),
            skaffold_path: self.skaffold_files.first().cloned(),
            gradle_root: self.gradle_root.clone(),
            deployed_version: None,
            drift,
            deploy_owner: None,
        }
    }
}

impl From<&DiscoveredProject> for ProjectRow {
    fn from(p: &DiscoveredProject) -> Self {
        p.to_row()
    }
}

impl From<DiscoveredProject> for ProjectRow {
    fn from(p: DiscoveredProject) -> Self {
        p.to_row()
    }
}

/// Parameters for a workspace walk (used by the background `Command::ScanWorkspace`).
#[derive(Debug, Clone)]
pub struct ScanOptions {
    pub roots: Vec<PathBuf>,
    pub max_depth: usize,
    pub ignore: Vec<String>,
    /// Drop matching projects from the inventory (see [`project_is_excluded`]).
    pub exclude: Vec<String>,
}

/// On-disk cache envelope (`~/.config/tako/cache/workspace.json`).
///
/// Stores browser rows so hydrate skips a second conversion on startup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceCache {
    /// Unix epoch seconds when the scan finished.
    pub scanned_at: u64,
    pub projects: Vec<ProjectRow>,
}

/// Convenience wrapper around [`scan_roots`] that maps errors to `String`
/// (matches the background-scan `AppEvent` error channel).
pub fn scan_workspace(opts: &ScanOptions) -> Result<Vec<DiscoveredProject>, String> {
    if opts.roots.is_empty() {
        return Err("no scan roots configured".into());
    }
    let mut projects =
        scan_roots(&opts.roots, opts.max_depth, &opts.ignore).map_err(|e| e.to_string())?;
    if !opts.exclude.is_empty() {
        let before = projects.len();
        projects.retain(|p| !project_is_excluded(p, &opts.exclude));
        let dropped = before.saturating_sub(projects.len());
        if dropped > 0 {
            tracing::info!(dropped, remaining = projects.len(), "scan exclude applied");
        }
    }
    Ok(projects)
}

/// True when `project` matches any entry in `[scan].exclude`.
///
/// Patterns (after `~/` expand):
/// - exact project **name** or **id** (e.g. `payments-api`, `group/leaf`)
/// - **leaf** name only (`common-lib` matches `libs/common-lib`)
/// - absolute **path** prefix/exact
/// - path **suffix** / substring (e.g. `services/legacy`)
/// - simple globs with `*` (one segment) and `**` (any path)
pub fn project_is_excluded(project: &DiscoveredProject, exclude: &[String]) -> bool {
    if exclude.is_empty() {
        return false;
    }
    let path_str = project.path.to_string_lossy();
    let name = project.name.as_str();
    let id = project.id.as_str();
    let leaf = name.rsplit('/').next().unwrap_or(name);

    for raw in exclude {
        let pat = raw.trim();
        if pat.is_empty() {
            continue;
        }
        let expanded = crate::config::expand_user_path(pat);
        let pat_s = expanded.to_string_lossy();
        let pat_s = pat_s.as_ref();

        if pat_s.contains('*') {
            if glob_match(pat_s, name)
                || glob_match(pat_s, id)
                || glob_match(pat_s, path_str.as_ref())
                || glob_match(pat_s, leaf)
            {
                return true;
            }
            continue;
        }

        if name == pat_s || id == pat_s || leaf == pat_s {
            return true;
        }
        // Absolute path: prefix or exact
        if expanded.is_absolute() {
            if project.path == expanded || project.path.starts_with(&expanded) {
                return true;
            }
        }
        // Relative path fragment
        if path_str.ends_with(pat_s)
            || path_str.contains(&format!("/{pat_s}/"))
            || path_str.contains(&format!("/{pat_s}"))
            || name.ends_with(pat_s)
            || name.contains(&format!("/{pat_s}"))
        {
            return true;
        }
    }
    false
}

/// Minimal glob: `*` = non-slash run, `**` = anything (including `/`).
fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.trim_start_matches("./");
    let text = text.trim_start_matches("./");
    glob_match_rec(pattern.as_bytes(), text.as_bytes())
}

fn glob_match_rec(pat: &[u8], text: &[u8]) -> bool {
    let mut pi = 0usize;
    let mut ti = 0usize;
    while pi < pat.len() {
        if pi + 1 < pat.len() && pat[pi] == b'*' && pat[pi + 1] == b'*' {
            // ** — match any suffix; optional following /
            pi += 2;
            if pi < pat.len() && pat[pi] == b'/' {
                pi += 1;
            }
            if pi >= pat.len() {
                return true;
            }
            while ti <= text.len() {
                if glob_match_rec(&pat[pi..], &text[ti..]) {
                    return true;
                }
                if ti == text.len() {
                    break;
                }
                ti += 1;
            }
            return false;
        }
        if pat[pi] == b'*' {
            pi += 1;
            if pi >= pat.len() {
                // trailing * — rest of segment until / or end
                return !text[ti..].contains(&b'/');
            }
            while ti <= text.len() {
                if text.get(ti) == Some(&b'/') && pat[pi] != b'/' {
                    break;
                }
                if glob_match_rec(&pat[pi..], &text[ti..]) {
                    return true;
                }
                if ti == text.len() {
                    break;
                }
                if text[ti] == b'/' {
                    break;
                }
                ti += 1;
            }
            return false;
        }
        if ti >= text.len() || pat[pi] != text[ti] {
            return false;
        }
        pi += 1;
        ti += 1;
    }
    ti == text.len()
}

pub fn load_workspace_cache(path: &Path) -> Option<WorkspaceCache> {
    let contents = fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

pub fn save_workspace_cache(path: &Path, projects: &[ProjectRow]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create cache dir: {e}"))?;
    }
    let cache = WorkspaceCache {
        scanned_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        projects: projects.to_vec(),
    };
    let body =
        serde_json::to_string_pretty(&cache).map_err(|e| format!("serialize cache: {e}"))?;
    fs::write(path, body).map_err(|e| format!("write cache: {e}"))?;
    Ok(())
}

/// Walk `roots` and discover projects.
///
/// * `max_depth` — how many directory levels below each root to descend
///   (0 = only the root directory itself).
/// * `ignore` — extra directory names / glob-ish patterns to skip
///   (e.g. `**/build/**`). `.git`, `build`, and `node_modules` are always skipped.
pub fn scan_roots(
    roots: &[PathBuf],
    max_depth: usize,
    ignore: &[String],
) -> AppResult<Vec<DiscoveredProject>> {
    let skip_names = build_skip_names(ignore);
    let mut git_cache = GitStatusCache::new();
    let mut projects: Vec<DiscoveredProject> = Vec::new();
    // Dedup by canonical project path so the same dir isn't listed twice.
    let mut seen: HashSet<PathBuf> = HashSet::new();

    // Pass 0: discover every local version catalog under the scan roots so
    // projects that only reference a published catalog (settings from("g:n:v"))
    // can still resolve libs.* against the TOML that lives in e.g. gradle-catalog/.
    let mut catalog_registry = CatalogRegistry::new();
    for root in roots {
        if root.is_dir() {
            catalog_registry.discover_under(root, max_depth.saturating_add(2));
        }
    }

    // Pass 1: collect dir hits + identity (no edges yet) so we can register
    // version-catalog publishers (group/name/version) before extracting depends.
    struct Pending {
        hit: DirHit,
        kind: crate::app::ProjectKind,
        version: String,
        gradle_root: Option<PathBuf>,
        git_root: Option<PathBuf>,
        branch: String,
        git_dirty: bool,
        name: String,
        folder_group: String,
        id: String,
    }
    let mut pending: Vec<Pending> = Vec::new();

    for root in roots {
        if !root.exists() {
            continue;
        }
        if !root.is_dir() {
            return Err(AppError::Other(format!(
                "scan root is not a directory: {}",
                root.display()
            )));
        }
        let root_canon = root.canonicalize().unwrap_or_else(|_| root.clone());
        let mut hits: Vec<DirHit> = Vec::new();
        walk_collect(&root_canon, 0, max_depth, &skip_names, &mut hits)?;

        for hit in hits {
            if !hit.has_skaffold && !hit.has_gradle {
                continue;
            }

            let project_path = hit.path.clone();
            let key = project_path
                .canonicalize()
                .unwrap_or_else(|_| project_path.clone());
            if !seen.insert(key) {
                continue;
            }

            let kind = infer_kind(&project_path, hit.has_skaffold, hit.has_gradle);
            let version = resolve_project_version(&project_path);
            let gradle_root = find_gradle_root(&project_path);
            let git_root = git::find_git_root(&project_path);
            let (branch, git_dirty) = match &git_root {
                Some(gr) => {
                    let info = git_cache.info(gr);
                    (info.branch.clone(), info.dirty)
                }
                None => (UNKNOWN_BRANCH.to_string(), false),
            };

            let name = relative_display_name(&root_canon, &project_path);
            let folder_group = folder_group_for(&root_canon, &project_path);
            let id = name.clone();

            // If this project publishes a version catalog, index it under its GAV.
            if hit.has_gradle {
                register_catalog_publisher(
                    &mut catalog_registry,
                    &project_path,
                    gradle_root.as_deref(),
                    &version,
                );
            }

            pending.push(Pending {
                hit,
                kind,
                version,
                gradle_root,
                git_root,
                branch,
                git_dirty,
                name,
                folder_group,
                id,
            });
        }
    }

    // Pass 2: extract depends/produces with full catalog registry.
    for p in pending {
        let (depends, produces) = if p.hit.has_gradle {
            let model = extract_for_project(
                &p.hit.path,
                &p.version,
                p.gradle_root.as_deref(),
                Some(&catalog_registry),
            );
            (model.depends, model.produces)
        } else {
            (Vec::new(), Vec::new())
        };

        projects.push(DiscoveredProject {
            id: p.id,
            path: p.hit.path,
            git_root: p.git_root,
            name: p.name,
            kind: p.kind,
            version: p.version,
            branch: p.branch,
            git_dirty: p.git_dirty,
            has_skaffold: p.hit.has_skaffold,
            skaffold_files: p.hit.skaffold_files,
            gradle_root: p.gradle_root,
            folder_group: p.folder_group,
            status: "idle".into(),
            depends,
            produces,
        });
    }

    projects.sort_by(|a, b| {
        a.folder_group
            .cmp(&b.folder_group)
            .then_with(|| a.name.cmp(&b.name))
    });

    Ok(projects)
}

/// Register a project that uses the `version-catalog` plugin + local TOML as a
/// published catalog coordinate (group from build script, artifact from
/// publication name or directory name).
fn register_catalog_publisher(
    registry: &mut CatalogRegistry,
    project_path: &Path,
    gradle_root: Option<&Path>,
    version: &str,
) {
    let mut is_catalog = false;
    let mut group = String::new();
    let mut artifact = project_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "catalog".into());

    for name in ["build.gradle.kts", "build.gradle"] {
        let path = project_path.join(name);
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        if content.contains("`version-catalog`")
            || content.contains("\"version-catalog\"")
            || content.contains("version-catalog")
            || content.contains("versionCatalog")
        {
            is_catalog = true;
        }
        // create<MavenPublication>("catalog") → artifact id often "catalog"
        if let Some(cap) = content
            .lines()
            .find_map(|l| {
                let t = l.trim();
                // create<MavenPublication>("catalog") or create("catalog", MavenPublication)
                if t.contains("MavenPublication") && t.contains("create") {
                    // pull first quoted string on the line
                    let bytes = t.as_bytes();
                    for (i, &b) in bytes.iter().enumerate() {
                        if b == b'"' || b == b'\'' {
                            if let Some(end) = t[i + 1..].find(b as char) {
                                return Some(t[i + 1..i + 1 + end].to_string());
                            }
                        }
                    }
                }
                None
            })
        {
            if !cap.is_empty() && cap != "maven" {
                artifact = cap;
            }
        }
        for line in content.lines() {
            let t = line.trim();
            if t.starts_with("group ") || t.starts_with("group=") || t.starts_with("group =") {
                if let Some(rest) = t.strip_prefix("group") {
                    let rest = rest.trim().trim_start_matches('=').trim();
                    if (rest.starts_with('"') || rest.starts_with('\'')) && rest.len() >= 2 {
                        let q = rest.as_bytes()[0];
                        if let Some(end) = rest[1..].find(q as char) {
                            group = rest[1..1 + end].to_string();
                        }
                    }
                }
            }
        }
    }

    if !is_catalog {
        // Still index local TOML under this project if present.
        if let Some(root) = gradle_root.or(Some(project_path)) {
            let c = root.join("gradle").join("libs.versions.toml");
            if c.is_file() {
                if let Ok(cat) = crate::gradle::catalog::VersionCatalog::load_file(&c) {
                    registry.insert_file(c, cat);
                }
            }
        }
        return;
    }

    let ver = if version == "—" { "1.0.0" } else { version };
    let g = if group.is_empty() {
        "unknown"
    } else {
        group.as_str()
    };
    registry.register_publisher(project_path, g, &artifact, ver);
    // Common default artifactId for version-catalog projects is the root project name
    // from settings — also register directory name variants.
    if let Some(name) = project_path.file_name().and_then(|n| n.to_str()) {
        if name != artifact {
            registry.register_publisher(project_path, g, name, ver);
        }
    }
}

#[derive(Debug)]
struct DirHit {
    path: PathBuf,
    has_skaffold: bool,
    skaffold_files: Vec<PathBuf>,
    has_gradle: bool,
}

fn walk_collect(
    dir: &Path,
    depth: usize,
    max_depth: usize,
    skip_names: &HashSet<String>,
    out: &mut Vec<DirHit>,
) -> AppResult<()> {
    let skaffold_files = list_skaffold_files(dir);
    let has_skaffold = !skaffold_files.is_empty();
    let has_gradle = has_gradle_build(dir);

    if has_skaffold || has_gradle {
        out.push(DirHit {
            path: dir.to_path_buf(),
            has_skaffold,
            skaffold_files,
            has_gradle,
        });
    }

    if depth >= max_depth {
        return Ok(());
    }

    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(err) => {
            tracing::debug!("skip unreadable dir {}: {err}", dir.display());
            return Ok(());
        }
    };

    for entry in entries {
        let entry = entry?;
        let ft = entry.file_type()?;
        if !ft.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if should_skip_dir_name(&name, skip_names) {
            continue;
        }
        walk_collect(&entry.path(), depth + 1, max_depth, skip_names, out)?;
    }
    Ok(())
}

fn list_skaffold_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for name in ["skaffold.yaml", "skaffold.yml"] {
        let p = dir.join(name);
        if p.is_file() {
            files.push(p);
        }
    }
    files
}

fn has_gradle_build(dir: &Path) -> bool {
    dir.join("build.gradle.kts").is_file() || dir.join("build.gradle").is_file()
}

/// Kind heuristics (overridable later via `tako.project.toml` / UI).
fn infer_kind(path: &Path, has_skaffold: bool, has_gradle: bool) -> ProjectKind {
    let name_l = path
        .file_name()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let path_l = path.to_string_lossy().to_ascii_lowercase();

    if name_l.contains("avro")
        || name_l.contains("schema")
        || path_l.contains("/avro")
        || path_l.contains("-avro")
        || path_l.contains("avro-")
    {
        return ProjectKind::Avro;
    }

    if has_skaffold {
        return ProjectKind::Service;
    }

    if has_gradle {
        return ProjectKind::Library;
    }

    ProjectKind::Unknown
}

/// Relative display name under the scan root (`orders` or `platform/orders`).
pub fn relative_display_name(scan_root: &Path, project: &Path) -> String {
    match project.strip_prefix(scan_root) {
        Ok(rel) if rel.as_os_str().is_empty() => scan_root
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| project.display().to_string()),
        Ok(rel) => rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
        Err(_) => project
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| project.display().to_string()),
    }
}

/// First path segment under the scan root, or the root folder name when the
/// project *is* the root.
pub fn folder_group_for(scan_root: &Path, project: &Path) -> String {
    match project.strip_prefix(scan_root) {
        Ok(rel) if rel.as_os_str().is_empty() => scan_root
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "root".into()),
        Ok(rel) => rel
            .components()
            .next()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .unwrap_or_else(|| "root".into()),
        Err(_) => scan_root
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "root".into()),
    }
}

/// Nearest ancestor (inclusive) with `settings.gradle(.kts)`, else the project
/// dir if it has a build file, else `None`.
pub fn find_gradle_root(project: &Path) -> Option<PathBuf> {
    let mut cur = project.to_path_buf();
    loop {
        if cur.join("settings.gradle.kts").is_file() || cur.join("settings.gradle").is_file() {
            return Some(cur);
        }
        if !cur.pop() {
            break;
        }
    }
    if has_gradle_build(project) {
        return Some(project.to_path_buf());
    }
    None
}

// ── Version parsing ──────────────────────────────────────────────────────────

/// Resolve local Gradle version for a project directory.
///
/// Order: module `build.gradle.kts` / `build.gradle` → module `gradle.properties`
/// → walk up for `gradle.properties` → [`UNKNOWN_VERSION`].
pub fn resolve_project_version(project_dir: &Path) -> String {
    for name in ["build.gradle.kts", "build.gradle"] {
        let p = project_dir.join(name);
        if let Ok(content) = fs::read_to_string(&p) {
            if let Some(v) = parse_version_from_build_script(&content) {
                return v;
            }
        }
    }

    if let Ok(content) = fs::read_to_string(project_dir.join("gradle.properties")) {
        if let Some(v) = parse_version_from_gradle_properties(&content) {
            return v;
        }
    }

    let mut cur = project_dir.to_path_buf();
    while cur.pop() {
        if let Ok(content) = fs::read_to_string(cur.join("gradle.properties")) {
            if let Some(v) = parse_version_from_gradle_properties(&content) {
                return v;
            }
        }
        if cur.parent().is_none() {
            break;
        }
    }

    UNKNOWN_VERSION.to_string()
}

/// Extract `version = "…"` / `version = '…'` from a Gradle build script.
/// Only matches a bare `version` assignment (not `kotlinVersion`, etc.).
pub fn parse_version_from_build_script(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = strip_gradle_comment(line).trim();
        if let Some(v) = parse_version_assignment_line(trimmed) {
            return Some(v);
        }
    }
    None
}

/// Extract `version=…` from `gradle.properties` (first non-comment match).
pub fn parse_version_from_gradle_properties(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('!') {
            continue;
        }
        // Must not use `?` here — a non-matching line must continue, not exit.
        let Some(rest) = trimmed.strip_prefix("version") else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let v = rest.trim().trim_matches('"').trim_matches('\'').trim();
        if !v.is_empty() {
            return Some(v.to_string());
        }
    }
    None
}

fn parse_version_assignment_line(trimmed: &str) -> Option<String> {
    let rest = trimmed.strip_prefix("version")?;
    // Reject identifiers that continue past "version" (e.g. versionCatalog).
    let rest = rest.trim_start();
    if rest.starts_with('=') {
        let rest = rest[1..].trim_start();
        return parse_quoted_or_bare(rest);
    }
    // version("1.0.0") / version('1.0.0')
    if rest.starts_with('(') {
        let rest = rest[1..].trim_start();
        return parse_quoted_or_bare(rest.trim_end_matches(')'));
    }
    None
}

fn parse_quoted_or_bare(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(inner) = strip_quotes(s) {
        let inner = inner.trim();
        if inner.is_empty() || inner.starts_with("libs.") {
            return None;
        }
        return Some(inner.to_string());
    }
    let token: String = s
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != '/' && *c != ';')
        .collect();
    if token.is_empty() || token.starts_with("libs.") {
        None
    } else {
        Some(token)
    }
}

fn strip_quotes(s: &str) -> Option<&str> {
    let s = s.trim();
    if s.len() < 2 {
        return None;
    }
    let bytes = s.as_bytes();
    if (bytes[0] == b'"' || bytes[0] == b'\'') && bytes[0] == bytes[s.len() - 1] {
        return Some(&s[1..s.len() - 1]);
    }
    if bytes[0] == b'"' {
        if let Some(end) = s[1..].find('"') {
            return Some(&s[1..1 + end]);
        }
    }
    if bytes[0] == b'\'' {
        if let Some(end) = s[1..].find('\'') {
            return Some(&s[1..1 + end]);
        }
    }
    None
}

fn strip_gradle_comment(line: &str) -> &str {
    if let Some(idx) = line.find("//") {
        let before = &line[..idx];
        let quote_count = before.matches('"').count() + before.matches('\'').count();
        if quote_count % 2 == 0 {
            return before;
        }
    }
    line
}

// ── Ignore helpers ───────────────────────────────────────────────────────────

fn build_skip_names(ignore: &[String]) -> HashSet<String> {
    let mut set = HashSet::new();
    set.insert(".git".into());
    set.insert("build".into());
    set.insert("node_modules".into());
    for pat in ignore {
        for name in ignore_pattern_dir_names(pat) {
            set.insert(name);
        }
    }
    set
}

/// Pull directory name candidates from patterns like `**/build/**`, `build`, `.git`.
fn ignore_pattern_dir_names(pat: &str) -> Vec<String> {
    let cleaned = pat.replace("**/", "").replace("/**", "").replace("**", "");
    cleaned
        .split('/')
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "*" && *s != "**")
        .map(|s| s.to_string())
        .collect()
}

fn should_skip_dir_name(name: &str, skip_names: &HashSet<String>) -> bool {
    if name == "." || name == ".." {
        return true;
    }
    if name == ".svn" || name == ".hg" {
        return true;
    }
    skip_names.contains(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(label: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "tako-scan-test-{}-{}-{}-{}",
            label,
            std::process::id(),
            nanos,
            n
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("mkdir parent");
        }
        fs::write(path, content).expect("write file");
    }

    // ── Version parse unit tests ─────────────────────────────────────────────

    #[test]
    fn parse_version_double_quotes() {
        let src = r#"
            plugins { id("java") }
            version = "1.4.2"
            group = "com.example"
        "#;
        assert_eq!(
            parse_version_from_build_script(src).as_deref(),
            Some("1.4.2")
        );
    }

    #[test]
    fn parse_version_single_quotes() {
        let src = "version = '0.0.1-SNAPSHOT'\n";
        assert_eq!(
            parse_version_from_build_script(src).as_deref(),
            Some("0.0.1-SNAPSHOT")
        );
    }

    #[test]
    fn parse_version_ignores_kotlin_version_style() {
        let src = r#"
            val kotlinVersion = "1.9.0"
            // no project version
        "#;
        assert_eq!(parse_version_from_build_script(src), None);
    }

    #[test]
    fn parse_version_ignores_catalog_ref_string() {
        let src = r#"version = "libs.versions.app""#;
        assert_eq!(parse_version_from_build_script(src), None);
    }

    #[test]
    fn parse_version_with_trailing_comment() {
        let src = r#"version = "2.1.0" // release"#;
        assert_eq!(
            parse_version_from_build_script(src).as_deref(),
            Some("2.1.0")
        );
    }

    #[test]
    fn parse_gradle_properties_version() {
        let src = "# header\norg.gradle.jvmargs=-Xmx1g\nversion=3.2.0\n";
        assert_eq!(
            parse_version_from_gradle_properties(src).as_deref(),
            Some("3.2.0")
        );
    }

    #[test]
    fn resolve_version_prefers_build_kts_over_properties() {
        let dir = temp_dir("ver-pref");
        write(&dir.join("build.gradle.kts"), "version = \"1.0.0\"\n");
        write(&dir.join("gradle.properties"), "version=9.9.9\n");
        assert_eq!(resolve_project_version(&dir), "1.0.0");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_version_falls_back_to_properties() {
        let dir = temp_dir("ver-props");
        write(&dir.join("build.gradle.kts"), "plugins { java }\n");
        write(&dir.join("gradle.properties"), "version=4.5.6\n");
        assert_eq!(resolve_project_version(&dir), "4.5.6");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_version_unknown() {
        let dir = temp_dir("ver-none");
        write(&dir.join("build.gradle.kts"), "plugins { java }\n");
        assert_eq!(resolve_project_version(&dir), UNKNOWN_VERSION);
        let _ = fs::remove_dir_all(&dir);
    }

    // ── Scan fixtures ────────────────────────────────────────────────────────

    #[test]
    fn scan_single_service_with_skaffold_and_version() {
        let root = temp_dir("single-svc");
        let svc = root.join("payments-api");
        write(&svc.join("skaffold.yaml"), "apiVersion: skaffold/v4beta1\n");
        write(
            &svc.join("build.gradle.kts"),
            r#"
            plugins { id("org.springframework.boot") }
            version = "3.2.0"
            "#,
        );
        write(&svc.join(".git"), "gitdir: fake\n");

        let projects = scan_roots(&[root.clone()], 4, &[]).expect("scan");
        assert_eq!(projects.len(), 1);
        let p = &projects[0];
        assert_eq!(p.name, "payments-api");
        assert_eq!(p.kind, ProjectKind::Service);
        assert_eq!(p.version, "3.2.0");
        assert!(p.has_skaffold);
        assert_eq!(p.skaffold_files.len(), 1);
        assert_eq!(p.folder_group, "payments-api");
        assert!(p.git_root.is_some());
        assert_eq!(p.branch, UNKNOWN_BRANCH);

        let row = p.to_row();
        assert_eq!(row.name, "payments-api");
        assert_eq!(row.version, "3.2.0");
        assert!(row.skaffold_path.is_some());
        assert_eq!(row.folder_group.as_deref(), Some("payments-api"));
        assert_eq!(row.path, p.path);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_multi_skaffold_monorepo_shares_git_root() {
        let root = temp_dir("monorepo");
        let st = Command::new("git")
            .args(["init", "-b", "feature/foo"])
            .current_dir(&root)
            .output()
            .expect("git init");
        assert!(st.status.success());
        let root_s = root.to_str().unwrap();
        let _ = Command::new("git")
            .args(["-C", root_s, "config", "user.email", "tako@test"])
            .status();
        let _ = Command::new("git")
            .args(["-C", root_s, "config", "user.name", "tako"])
            .status();

        write(
            &root.join("orders").join("skaffold.yaml"),
            "apiVersion: skaffold/v4beta1\n",
        );
        write(
            &root.join("orders").join("build.gradle.kts"),
            r#"version = "2.1.0""#,
        );
        write(
            &root.join("billing").join("skaffold.yml"),
            "apiVersion: skaffold/v4beta1\n",
        );
        write(
            &root.join("billing").join("build.gradle.kts"),
            r#"version = "2.0.3""#,
        );
        write(
            &root.join("settings.gradle.kts"),
            r#"rootProject.name = "platform""#,
        );

        let projects = scan_roots(&[root.clone()], 6, &[]).expect("scan");
        assert_eq!(projects.len(), 2, "one row per skaffold: {:?}", projects);

        let names: HashSet<_> = projects.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains("orders"));
        assert!(names.contains("billing"));

        let git_roots: HashSet<_> = projects
            .iter()
            .filter_map(|p| p.git_root.as_ref().map(|g| g.canonicalize().unwrap()))
            .collect();
        assert_eq!(git_roots.len(), 1, "shared git_root");

        let root_canon = root.canonicalize().unwrap();
        for p in &projects {
            assert_eq!(p.kind, ProjectKind::Service);
            assert!(p.has_skaffold);
            assert_eq!(p.branch, "feature/foo");
            assert!(p.git_dirty, "untracked files → dirty");
            assert_eq!(p.folder_group, p.name);
            assert!(
                p.gradle_root
                    .as_ref()
                    .is_some_and(|g| g.canonicalize().unwrap() == root_canon),
                "gradle_root should be monorepo root with settings.gradle.kts"
            );
        }

        let orders = projects.iter().find(|p| p.name == "orders").unwrap();
        assert_eq!(orders.version, "2.1.0");
        let billing = projects.iter().find(|p| p.name == "billing").unwrap();
        assert_eq!(billing.version, "2.0.3");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_lib_without_skaffold() {
        let root = temp_dir("libs-root");
        let lib = root.join("common-lib");
        write(
            &lib.join("build.gradle.kts"),
            r#"
            plugins {
                `java-library`
                `maven-publish`
            }
            version = "1.4.2"
            "#,
        );
        write(&lib.join(".git"), "gitdir: fake\n");

        let projects = scan_roots(&[root.clone()], 4, &[]).expect("scan");
        assert_eq!(projects.len(), 1);
        let p = &projects[0];
        assert_eq!(p.name, "common-lib");
        assert_eq!(p.kind, ProjectKind::Library);
        assert_eq!(p.version, "1.4.2");
        assert!(!p.has_skaffold);
        assert!(p.skaffold_files.is_empty());
        assert!(p.gradle_root.is_some());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_avro_by_name_heuristic() {
        let root = temp_dir("avro-root");
        let avro = root.join("avro-payments");
        write(
            &avro.join("build.gradle.kts"),
            r#"version = "1.3.0"
plugins { `java-library` }
"#,
        );

        let projects = scan_roots(&[root.clone()], 3, &[]).expect("scan");
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].kind, ProjectKind::Avro);
        assert_eq!(projects[0].version, "1.3.0");
        assert!(!projects[0].has_skaffold);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_skips_build_and_node_modules() {
        let root = temp_dir("skip-dirs");
        write(
            &root.join("svc").join("skaffold.yaml"),
            "apiVersion: skaffold/v4beta1\n",
        );
        write(
            &root.join("svc").join("build").join("skaffold.yaml"),
            "should-not-see\n",
        );
        write(
            &root
                .join("svc")
                .join("node_modules")
                .join("x")
                .join("skaffold.yaml"),
            "should-not-see\n",
        );
        write(
            &root.join("svc").join(".git").join("skaffold.yaml"),
            "should-not-see\n",
        );

        let projects = scan_roots(&[root.clone()], 8, &[]).expect("scan");
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "svc");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_respects_max_depth() {
        let root = temp_dir("depth");
        write(
            &root
                .join("deep")
                .join("nested")
                .join("svc")
                .join("skaffold.yaml"),
            "apiVersion: skaffold/v4beta1\n",
        );
        let none = scan_roots(&[root.clone()], 0, &[]).expect("scan");
        assert!(none.is_empty());
        let d1 = scan_roots(&[root.clone()], 1, &[]).expect("scan");
        assert!(d1.is_empty());
        let d3 = scan_roots(&[root.clone()], 3, &[]).expect("scan");
        assert_eq!(d3.len(), 1);
        assert_eq!(d3[0].name, "deep/nested/svc");
        assert_eq!(d3[0].folder_group, "deep");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn project_is_excluded_by_name_path_and_glob() {
        let mut p = DiscoveredProject {
            id: "services/legacy-api".into(),
            path: PathBuf::from("/ws/services/legacy-api"),
            git_root: None,
            name: "services/legacy-api".into(),
            kind: ProjectKind::Service,
            version: "1.0.0".into(),
            branch: "main".into(),
            git_dirty: false,
            has_skaffold: true,
            skaffold_files: vec![],
            gradle_root: None,
            folder_group: "services".into(),
            status: "idle".into(),
            depends: vec![],
            produces: vec![],
        };
        assert!(project_is_excluded(&p, &["legacy-api".into()]));
        assert!(project_is_excluded(&p, &["services/legacy-api".into()]));
        assert!(project_is_excluded(&p, &["/ws/services/legacy-api".into()]));
        assert!(project_is_excluded(&p, &["**/legacy-api".into()]));
        assert!(project_is_excluded(&p, &["services/*".into()]));
        assert!(!project_is_excluded(&p, &["orders-api".into()]));
        assert!(!project_is_excluded(&p, &[]));

        p.name = "payments-api".into();
        p.id = "payments-api".into();
        p.path = PathBuf::from("/ws/payments-api");
        assert!(project_is_excluded(&p, &["payments-api".into()]));
        assert!(!project_is_excluded(&p, &["legacy-api".into()]));
    }

    #[test]
    fn scan_workspace_applies_exclude() {
        let root = temp_dir("exclude");
        write(
            &root.join("keep").join("skaffold.yaml"),
            "apiVersion: skaffold/v4beta1\n",
        );
        write(
            &root.join("keep").join("build.gradle.kts"),
            r#"version = "1.0.0""#,
        );
        write(
            &root.join("drop-me").join("skaffold.yaml"),
            "apiVersion: skaffold/v4beta1\n",
        );
        write(
            &root.join("drop-me").join("build.gradle.kts"),
            r#"version = "0.1.0""#,
        );

        let all = scan_workspace(&ScanOptions {
            roots: vec![root.clone()],
            max_depth: 4,
            ignore: vec![],
            exclude: vec![],
        })
        .expect("scan");
        assert_eq!(all.len(), 2, "names: {:?}", all.iter().map(|p| &p.name).collect::<Vec<_>>());

        let filtered = scan_workspace(&ScanOptions {
            roots: vec![root.clone()],
            max_depth: 4,
            ignore: vec![],
            exclude: vec!["drop-me".into()],
        })
        .expect("scan");
        assert_eq!(filtered.len(), 1);
        assert!(
            filtered.iter().all(|p| p.name.contains("keep")),
            "{:?}",
            filtered.iter().map(|p| &p.name).collect::<Vec<_>>()
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn folder_group_and_relative_name() {
        let root = PathBuf::from("/Users/you/Developer");
        let proj = PathBuf::from("/Users/you/Developer/services/payments-api");
        assert_eq!(relative_display_name(&root, &proj), "services/payments-api");
        assert_eq!(folder_group_for(&root, &proj), "services");
        assert_eq!(relative_display_name(&root, &root), "Developer");
        assert_eq!(folder_group_for(&root, &root), "Developer");
    }

    #[test]
    fn missing_root_is_skipped() {
        let missing = std::env::temp_dir().join(format!(
            "tako-scan-missing-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&missing);
        let projects = scan_roots(&[missing], 3, &[]).expect("scan");
        assert!(projects.is_empty());
    }

    /// End-to-end: fixture catalog range → scan populates depends/produces →
    /// graph selects range consumers for cascade.
    #[test]
    fn scan_fixture_catalog_range_builds_graph() {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/catalog-range");
        if !fixture.is_dir() {
            // Fixture optional in some checkouts.
            return;
        }
        let projects = scan_roots(&[fixture], 4, &[]).expect("scan fixture");
        assert!(
            projects.len() >= 3,
            "expected common-lib + 2 services, got {:?}",
            projects.iter().map(|p| &p.name).collect::<Vec<_>>()
        );

        let lib = projects
            .iter()
            .find(|p| p.name.contains("common-lib"))
            .expect("common-lib");
        assert!(!lib.produces.is_empty(), "lib should produce a coordinate");
        assert_eq!(lib.version, "1.4.2");

        let payments = projects
            .iter()
            .find(|p| p.name.contains("payments-api"))
            .expect("payments-api");
        assert!(
            payments.depends.iter().any(|d| {
                d.coordinate
                    .as_ref()
                    .is_some_and(|c| c.key() == "com.example:common-lib")
            }),
            "payments depends: {:?}",
            payments.depends
        );

        let g = crate::graph::DependencyGraph::from_projects(&projects);
        let consumers = g.dependents_of(&lib.id);
        assert!(
            consumers.iter().any(|c| c.contains("payments")),
            "range consumer payments should depend on lib: {consumers:?}"
        );
        assert!(
            !consumers.iter().any(|c| c.contains("legacy")),
            "legacy exact 1.0.0 must not match 1.4.2: {consumers:?}"
        );
    }

    /// Published version catalog in a sibling repo (settings from("g:n:v")), no
    /// local libs.versions.toml next to the service — mirrors tmp-dev/animals/cat.
    #[test]
    fn scan_resolves_libs_via_workspace_published_catalog() {
        let root = temp_dir("pub-catalog-ws");
        let cat_proj = root.join("gradle-catalog");
        let svc = root.join("animals").join("cat");
        write(
            &cat_proj.join("gradle").join("libs.versions.toml"),
            r#"
[versions]
avro-cats = "1.0.0"
util-core = "[1.0.0,2.0.0)"
[libraries]
avro-cats = { module = "com.tako.fixtures:cats", version.ref = "avro-cats" }
util-core = { module = "com.tako.fixtures:util-core", version.ref = "util-core" }
"#,
        );
        write(
            &cat_proj.join("build.gradle.kts"),
            r#"
plugins {
    `version-catalog`
    `maven-publish`
}
group = "com.tako.fixtures"
version = "1.0.0"
catalog {
    versionCatalog {
        from(files("gradle/libs.versions.toml"))
    }
}
publishing {
    publications {
        create<MavenPublication>("catalog") {
            from(components["versionCatalog"])
        }
    }
}
"#,
        );
        write(
            &svc.join("settings.gradle.kts"),
            r#"
dependencyResolutionManagement {
    versionCatalogs {
        create("libs") {
            from("com.tako.fixtures:catalog:1.0.0")
        }
    }
}
rootProject.name = "cat"
"#,
        );
        write(
            &svc.join("build.gradle.kts"),
            r#"
group = "com.tako.fixtures"
version = "0.1.0"
dependencies {
    implementation(libs.avro.cats)
    implementation(libs.util.core)
    implementation(libs.spring.boot.starter.web)
}
"#,
        );
        write(&svc.join("skaffold.yaml"), "apiVersion: skaffold/v4beta1\n");

        let projects = scan_roots(&[root.clone()], 6, &[]).expect("scan");
        let cat = projects
            .iter()
            .find(|p| p.name.contains("cat") && p.has_skaffold)
            .expect("cat service");
        assert!(
            cat.depends.len() >= 2,
            "expected libs.* resolved, got {:?}",
            cat.depends
        );
        assert!(
            cat.depends.iter().any(|d| {
                d.coordinate
                    .as_ref()
                    .is_some_and(|c| c.key() == "com.tako.fixtures:cats")
            }),
            "avro-cats: {:?}",
            cat.depends
        );
        assert!(
            cat.depends.iter().any(|d| {
                d.coordinate
                    .as_ref()
                    .is_some_and(|c| c.key() == "com.tako.fixtures:util-core")
            }),
            "util-core: {:?}",
            cat.depends
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// Live smoke against `~/Developer/tmp-dev` when present (user fixture).
    #[test]
    fn scan_real_tmp_dev_cat_if_present() {
        let home = std::env::var("HOME").unwrap_or_default();
        let root = PathBuf::from(home).join("Developer/tmp-dev");
        if !root.is_dir() {
            return;
        }
        let projects = scan_roots(&[root], 8, &[]).expect("scan tmp-dev");
        let cat = projects.iter().find(|p| {
            p.path.ends_with("animals/cat")
                || p.name == "cat"
                || p.name.ends_with("/cat")
                || p.name.contains("animals/cat")
        });
        let Some(cat) = cat else {
            // Tree layout may differ — don't fail CI.
            return;
        };
        let resolved = cat
            .depends
            .iter()
            .filter(|d| d.coordinate.is_some())
            .count();
        assert!(
            resolved >= 2,
            "cat should resolve libs.* via workspace catalog, depends={:?}",
            cat.depends
        );
    }
}
