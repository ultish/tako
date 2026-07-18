//! Multi-step plans: confirmable recipe → sequential/parallel steps.
//!
//! - [`RECIPE_UPDATE_DEPENDENTS`] (**U**) — dependent **libs** = Nexus status only;
//!   **services** = git pull → clean → build (latest SNAPSHOT) → skaffold delete→run.
//! - [`RECIPE_SKAFFOLD_REDEPLOY`] (**u**) — this project: delete then run.

use std::path::PathBuf;

use crate::app::{ProjectKind, ProjectRow};
use crate::config::Config;
use crate::graph::DependencyGraph;

/// **U**: Nexus-check dependent libs; run services only.
pub const RECIPE_UPDATE_DEPENDENTS: &str = "update_dependents";

/// **u**: skaffold delete then run for the cursor project.
pub const RECIPE_SKAFFOLD_REDEPLOY: &str = "skaffold_redeploy";

/// Short human title for plan overlays / status (not the stable recipe id).
pub fn recipe_title(recipe: &str) -> &'static str {
    match recipe {
        RECIPE_UPDATE_DEPENDENTS => "Update dependents",
        RECIPE_SKAFFOLD_REDEPLOY => "Skaffold redeploy",
        _ => "Plan",
    }
}

/// One-line plain-English summary of what the recipe does.
pub fn recipe_summary(recipe: &str) -> &'static str {
    match recipe {
        RECIPE_UPDATE_DEPENDENTS => {
            "Libs: Nexus status (no local build). Services: git pull → clean → B → delete→run"
        }
        RECIPE_SKAFFOLD_REDEPLOY => "skaffold delete, then skaffold run (updates the image cleanly)",
        _ => "Run the planned steps in order",
    }
}

/// Always delete then run (user requirement for correct image updates).
pub fn skaffold_redeploy_steps() -> [&'static str; 2] {
    ["delete", "run"]
}

/// Dependent lib/avro row — shown on the U plan, **not executed**.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibNexusRow {
    pub project_id: String,
    pub project_name: String,
    pub local_version: String,
    pub produces: String,
    /// `ok` | `warn` | `unknown`
    pub status: String,
    pub detail: String,
}

impl LibNexusRow {
    pub fn status_glyph(&self) -> &'static str {
        match self.status.as_str() {
            "ok" => "✓",
            "warn" => "⚠",
            _ => "?",
        }
    }
}

/// Kind of work a cascade step performs (executable rows only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CascadeStepKind {
    GitPull,
    /// Gradle tasks; `force_latest_snapshots` adds tako's SNAPSHOT init script.
    Gradle {
        tasks: Vec<String>,
        force_latest_snapshots: bool,
    },
    Skaffold { subcommand: String },
}

impl CascadeStepKind {
    /// Column label for the plan table.
    pub fn step_label(&self) -> String {
        match self {
            CascadeStepKind::GitPull => "git pull --ff-only".into(),
            CascadeStepKind::Gradle {
                tasks,
                force_latest_snapshots,
            } => {
                let base = if tasks.is_empty() {
                    "gradle".into()
                } else {
                    format!("gradle {}", tasks.join(" "))
                };
                if *force_latest_snapshots {
                    format!("{base} (latest SNAPSHOT)")
                } else {
                    base
                }
            }
            CascadeStepKind::Skaffold { subcommand } => {
                format!("skaffold {subcommand}")
            }
        }
    }

    /// Task / argv column (tasks joined, or `-f` file leaf).
    pub fn task_column(&self, skaffold_file: Option<&std::path::Path>) -> String {
        match self {
            CascadeStepKind::GitPull => "ff-only".into(),
            CascadeStepKind::Gradle { tasks, .. } => tasks
                .iter()
                .map(|t| {
                    if t.starts_with(':') {
                        t.clone()
                    } else {
                        format!(":{t}")
                    }
                })
                .collect::<Vec<_>>()
                .join(" "),
            CascadeStepKind::Skaffold { .. } => skaffold_file
                .and_then(|p| p.file_name())
                .map(|n| format!("-f {}", n.to_string_lossy()))
                .unwrap_or_else(|| "-f skaffold.yaml".into()),
        }
    }
}

/// Lifecycle of one plan row while confirming / executing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CascadeStepStatus {
    #[default]
    Pending,
    Running,
    Ok,
    Failed,
    Skipped,
}

impl CascadeStepStatus {
    pub fn label(self) -> &'static str {
        match self {
            CascadeStepStatus::Pending => "pend",
            CascadeStepStatus::Running => "run",
            CascadeStepStatus::Ok => "ok",
            CascadeStepStatus::Failed => "fail",
            CascadeStepStatus::Skipped => "skip",
        }
    }
}

/// One ordered step in a cascade plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CascadeStep {
    pub kind: CascadeStepKind,
    pub project_id: String,
    pub project_name: String,
    pub project_path: PathBuf,
    /// Directory for `gradle -p` (gradle_root or project path).
    pub gradle_dir: PathBuf,
    pub skaffold_file: Option<PathBuf>,
    pub status: CascadeStepStatus,
    /// Job id once this step has been dispatched.
    pub job_id: Option<u64>,
}

impl CascadeStep {
    pub fn step_label(&self) -> String {
        self.kind.step_label()
    }

    pub fn task_column(&self) -> String {
        self.kind
            .task_column(self.skaffold_file.as_deref())
    }
}

/// Confirmable + executable cascade plan held on [`crate::app::App`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CascadePlan {
    pub recipe: String,
    pub source_id: String,
    pub source_name: String,
    pub source_version: String,
    /// Dependent libs (U plan): Nexus status only — not run as jobs.
    pub lib_checks: Vec<LibNexusRow>,
    /// Executable steps (services for U; delete→run for u).
    pub steps: Vec<CascadeStep>,
    /// Scroll / highlight cursor in the plan table (confirm UI).
    pub cursor: usize,
    /// Catalog range mismatch lines (and similar).
    pub warnings: Vec<String>,
    /// True after y/Enter — steps advance on JobFinished.
    pub executing: bool,
    /// Job ids currently in flight for this plan → step index.
    pub active_jobs: std::collections::HashMap<u64, usize>,
}

impl CascadePlan {
    pub fn is_confirming(&self) -> bool {
        !self.executing
    }

    pub fn move_cursor(&mut self, delta: i32) {
        if self.steps.is_empty() {
            return;
        }
        let cur = self.cursor as i32;
        let next = (cur + delta).clamp(0, (self.steps.len() as i32) - 1) as usize;
        self.cursor = next;
    }

    pub fn running_count(&self) -> usize {
        self.active_jobs.len()
    }

    pub fn completed_count(&self) -> usize {
        self.steps
            .iter()
            .filter(|s| {
                matches!(
                    s.status,
                    CascadeStepStatus::Ok | CascadeStepStatus::Failed | CascadeStepStatus::Skipped
                )
            })
            .count()
    }

    /// Whether this job id belongs to an in-flight cascade step.
    pub fn owns_job(&self, job_id: u64) -> bool {
        self.executing && self.active_jobs.contains_key(&job_id)
    }

    /// Mark the step that owns `job_id` finished; returns its step index if found.
    pub fn finish_job(&mut self, job_id: u64, ok: bool, cancelled: bool) -> Option<usize> {
        let idx = self.active_jobs.remove(&job_id)?;
        if let Some(step) = self.steps.get_mut(idx) {
            step.status = if cancelled || !ok {
                CascadeStepStatus::Failed
            } else {
                CascadeStepStatus::Ok
            };
        }
        Some(idx)
    }

    /// True when every step is terminal (ok / failed / skipped) and nothing runs.
    pub fn is_fully_done(&self) -> bool {
        self.active_jobs.is_empty()
            && self.steps.iter().all(|s| {
                !matches!(
                    s.status,
                    CascadeStepStatus::Pending | CascadeStepStatus::Running
                )
            })
    }

    /// Pending step indices that may start now, respecting:
    /// - source (`source_id`) steps must complete before other projects
    /// - within a project, earlier plan steps must complete first
    /// - order among ready steps is plan order
    pub fn ready_step_indices(&self) -> Vec<usize> {
        let mut ready = Vec::new();
        for (i, step) in self.steps.iter().enumerate() {
            if step.status != CascadeStepStatus::Pending {
                continue;
            }
            if self.step_prerequisites_met(i) {
                ready.push(i);
            }
        }
        ready
    }

    fn step_prerequisites_met(&self, idx: usize) -> bool {
        let Some(step) = self.steps.get(idx) else {
            return false;
        };
        let project_id = step.project_id.as_str();
        let source_id = self.source_id.as_str();

        for prior in self.steps.iter().take(idx) {
            let same_project = prior.project_id == project_id;
            let is_source = prior.project_id == source_id;
            if !same_project && !is_source {
                continue;
            }
            match prior.status {
                CascadeStepStatus::Ok | CascadeStepStatus::Skipped => {}
                CascadeStepStatus::Pending
                | CascadeStepStatus::Running
                | CascadeStepStatus::Failed => return false,
            }
        }
        true
    }

    /// Indices to start now: ready steps, capped by free slots (`max_parallel`).
    /// Source-only phase: when any source step is still not done, force limit 1.
    pub fn indices_to_dispatch(&self, max_parallel: usize) -> Vec<usize> {
        let limit = self.effective_parallelism(max_parallel);
        let free = limit.saturating_sub(self.running_count());
        if free == 0 {
            return vec![];
        }
        self.ready_step_indices().into_iter().take(free).collect()
    }

    fn effective_parallelism(&self, max_parallel: usize) -> usize {
        let max = max_parallel.max(1);
        if self.source_barrier_open() {
            max
        } else {
            // Source publish still pending/running — only one job (the source).
            1
        }
    }

    /// True once every source_id step is Ok or Skipped (or there are none).
    fn source_barrier_open(&self) -> bool {
        for step in &self.steps {
            if step.project_id != self.source_id {
                continue;
            }
            match step.status {
                CascadeStepStatus::Ok | CascadeStepStatus::Skipped => {}
                _ => return false,
            }
        }
        true
    }

    /// Remaining pending steps after a failure become skipped (running left alone).
    pub fn mark_remaining_skipped(&mut self) {
        for step in self.steps.iter_mut() {
            if step.status == CascadeStepStatus::Pending {
                step.status = CascadeStepStatus::Skipped;
            }
        }
    }

    /// Register a newly started job on `step_idx`.
    pub fn register_active(&mut self, job_id: u64, step_idx: usize) {
        self.active_jobs.insert(job_id, step_idx);
        if let Some(s) = self.steps.get_mut(step_idx) {
            s.status = CascadeStepStatus::Running;
            s.job_id = Some(job_id);
        }
    }
}

/// Gradle tasks for the **source** publish step: clean + build + publish (config defaults).
pub fn source_publish_tasks(config: &Config) -> Vec<String> {
    let mut tasks = Vec::new();
    for t in &config.gradle.default_tasks_clean {
        if !tasks.contains(t) {
            tasks.push(t.clone());
        }
    }
    for t in &config.gradle.default_tasks_build {
        if !tasks.contains(t) {
            tasks.push(t.clone());
        }
    }
    for t in &config.gradle.default_tasks_publish {
        if !tasks.contains(t) {
            tasks.push(t.clone());
        }
    }
    // Fallback if all empty: still try publish.
    if tasks.is_empty() {
        tasks.push("publish".into());
    }
    tasks
}

/// **U**: dependent libs → Nexus status only; services → pull/clean/B/delete→run.
pub fn build_update_dependents(
    projects: &[ProjectRow],
    graph: &DependencyGraph,
    source_index: usize,
    config: &Config,
) -> Result<CascadePlan, String> {
    let source = projects
        .get(source_index)
        .ok_or_else(|| "no project selected".to_string())?;
    let source_id = graph
        .id_at(source_index)
        .unwrap_or(source.name.as_str())
        .to_string();

    let consumers = graph.dependents_transitive_topo(&source_id);
    if consumers.is_empty() {
        return Err(format!(
            "nothing depends on {} in the workspace — nothing to update",
            source.name
        ));
    }

    let build_tasks = if config.cascade.default_consumer_gradle.is_empty() {
        config.gradle.default_tasks_build.clone()
    } else {
        config.cascade.default_consumer_gradle.clone()
    };
    let clean_tasks = if config.gradle.default_tasks_clean.is_empty() {
        vec!["clean".to_string()]
    } else {
        config.gradle.default_tasks_clean.clone()
    };
    let force_snaps = config.gradle.force_latest_snapshots;

    let mut lib_checks = Vec::new();
    let mut steps = Vec::new();
    let mut warnings = Vec::new();
    let mut n_services = 0usize;

    for consumer_id in &consumers {
        let Some(idx) = graph.index_of(consumer_id) else {
            continue;
        };
        let Some(row) = projects.get(idx) else {
            continue;
        };

        // Intermediate libs/avro: Nexus status only — local gradle build won't
        // put them on Nexus for services to consume.
        let is_lib_like = matches!(
            row.kind,
            ProjectKind::Library | ProjectKind::Avro
        ) || (!row.has_skaffold && row.skaffold_path.is_none());

        if is_lib_like && !matches!(row.kind, ProjectKind::Service) {
            let produces = graph.produces_of_id(consumer_id);
            let produces_s = if produces.is_empty() {
                "—".into()
            } else {
                produces
                    .iter()
                    .map(|p| p.coordinate.display())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            // Prefer live **r** probe label when present.
            let (status, detail) = match row.nexus.as_str() {
                "ok" => (
                    "ok".into(),
                    format!("nexus ok · local {}", row.version),
                ),
                "newer" => (
                    "warn".into(),
                    format!("newer on nexus · local {}", row.version),
                ),
                "miss" | "?" => (
                    "warn".into(),
                    format!("nexus {} · local {}", row.nexus, row.version),
                ),
                "—" | "" => (
                    "unknown".into(),
                    format!(
                        "local {} — press r to probe Maven repo (not built locally)",
                        row.version
                    ),
                ),
                other => (
                    "unknown".into(),
                    format!("nexus {other} · local {}", row.version),
                ),
            };
            lib_checks.push(LibNexusRow {
                project_id: consumer_id.clone(),
                project_name: row.name.clone(),
                local_version: row.version.clone(),
                produces: produces_s,
                status,
                detail,
            });
            continue;
        }

        // Services (or skaffold deployables): executable pipeline.
        n_services += 1;
        let gradle_dir = row.gradle_project_dir().to_path_buf();
        let skaffold_file = row.skaffold_path.clone();

        // git pull (skipped at spawn if no git root — mark skip)
        steps.push(CascadeStep {
            kind: CascadeStepKind::GitPull,
            project_id: consumer_id.clone(),
            project_name: row.name.clone(),
            project_path: row.path.clone(),
            gradle_dir: gradle_dir.clone(),
            skaffold_file: skaffold_file.clone(),
            status: CascadeStepStatus::Pending,
            job_id: None,
        });

        if !clean_tasks.is_empty() {
            steps.push(CascadeStep {
                kind: CascadeStepKind::Gradle {
                    tasks: clean_tasks.clone(),
                    force_latest_snapshots: false,
                },
                project_id: consumer_id.clone(),
                project_name: row.name.clone(),
                project_path: row.path.clone(),
                gradle_dir: gradle_dir.clone(),
                skaffold_file: skaffold_file.clone(),
                status: CascadeStepStatus::Pending,
                job_id: None,
            });
        }

        steps.push(CascadeStep {
            kind: CascadeStepKind::Gradle {
                tasks: build_tasks.clone(),
                force_latest_snapshots: force_snaps,
            },
            project_id: consumer_id.clone(),
            project_name: row.name.clone(),
            project_path: row.path.clone(),
            gradle_dir: gradle_dir.clone(),
            skaffold_file: skaffold_file.clone(),
            status: CascadeStepStatus::Pending,
            job_id: None,
        });

        if crate::deploy::cascade_skaffold_ok(row, config) {
            for sub in skaffold_redeploy_steps() {
                steps.push(CascadeStep {
                    kind: CascadeStepKind::Skaffold {
                        subcommand: (*sub).to_string(),
                    },
                    project_id: consumer_id.clone(),
                    project_name: row.name.clone(),
                    project_path: row.path.clone(),
                    gradle_dir: gradle_dir.clone(),
                    skaffold_file: skaffold_file.clone(),
                    status: CascadeStepStatus::Pending,
                    job_id: None,
                });
            }
        } else if row.has_skaffold || row.skaffold_path.is_some() {
            let mode = crate::deploy::effective_deploy_mode(row, config).label();
            warnings.push(format!(
                "won’t skaffold {} (deploy={mode}) — pull/clean/build only",
                row.name
            ));
        } else {
            warnings.push(format!(
                "{} has no skaffold — pull/clean/build only",
                row.name
            ));
        }
    }

    if lib_checks.is_empty() && steps.is_empty() {
        return Err(format!(
            "nothing actionable depends on {} — no services to redeploy",
            source.name
        ));
    }

    if !lib_checks.is_empty() {
        warnings.push(format!(
            "{} child lib(s): not built locally — must already be on Nexus (or p them yourself)",
            lib_checks.len()
        ));
        if lib_checks.iter().any(|l| l.status == "unknown") {
            warnings.push(
                "press r to probe Maven (from Gradle settings/build scripts)".into(),
            );
        }
    }

    if n_services > 0 {
        warnings.push(format!(
            "{n_services} service(s): git pull → clean → build (snap) → skaffold delete→run"
        ));
    }

    let produces = graph.produces_of_id(&source_id);
    for prod in produces {
        let ver = if prod.version.is_empty() || prod.version == "—" {
            source.version.clone()
        } else {
            prod.version.clone()
        };
        for excluded in graph.consumers_excluded_by_range(&prod.coordinate, &ver) {
            warnings.push(format!(
                "catalog range for {excluded} will NOT pick up {ver} ({})",
                prod.coordinate.display()
            ));
        }
    }

    Ok(CascadePlan {
        recipe: RECIPE_UPDATE_DEPENDENTS.into(),
        source_id,
        source_name: source.name.clone(),
        source_version: source.version.clone(),
        lib_checks,
        steps,
        cursor: 0,
        warnings,
        executing: false,
        active_jobs: std::collections::HashMap::new(),
    })
}

/// **u**: skaffold delete then run for the cursor project only.
pub fn build_skaffold_redeploy(
    projects: &[ProjectRow],
    source_index: usize,
    config: &Config,
) -> Result<CascadePlan, String> {
    let source = projects
        .get(source_index)
        .ok_or_else(|| "no project selected".to_string())?;
    if source.skaffold_path.is_none() && !source.has_skaffold {
        return Err(format!("no skaffold file: {}", source.name));
    }
    let mode = crate::deploy::effective_deploy_mode(source, config);
    if mode == crate::config::DeployMode::Manual {
        return Err(format!(
            "deploy mode manual for {} — skaffold disabled",
            source.name
        ));
    }
    // Argo: still allow plan but caller should confirm; we include steps either way
    // if has skaffold (guard is in app before open, or confirm overlay).

    let source_id = source.name.clone();
    let mut steps = Vec::new();
    let mut warnings = Vec::new();
    let skaffold_file = source.skaffold_path.clone();
    for sub in skaffold_redeploy_steps() {
        steps.push(CascadeStep {
            kind: CascadeStepKind::Skaffold {
                subcommand: (*sub).to_string(),
            },
            project_id: source_id.clone(),
            project_name: source.name.clone(),
            project_path: source.path.clone(),
            gradle_dir: source.gradle_project_dir().to_path_buf(),
            skaffold_file: skaffold_file.clone(),
            status: CascadeStepStatus::Pending,
            job_id: None,
        });
    }
    if source.deploy_owner.as_deref() == Some("argocd") || mode == crate::config::DeployMode::Argocd
    {
        warnings.push("project looks Argo-managed — confirm carefully".into());
    }

    Ok(CascadePlan {
        recipe: RECIPE_SKAFFOLD_REDEPLOY.into(),
        source_id,
        source_name: source.name.clone(),
        source_version: source.version.clone(),
        lib_checks: vec![],
        steps,
        cursor: 0,
        warnings,
        executing: false,
        active_jobs: std::collections::HashMap::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::ProjectKind;
    use crate::config::{CascadeConfig, Config, GradleConfig};
    use crate::gradle::model::{Coordinate, DepReq, Produces, VersionSpec};
    use crate::scan::DiscoveredProject;

    fn proj(
        id: &str,
        version: &str,
        kind: ProjectKind,
        has_skaffold: bool,
        produces: Vec<Produces>,
        depends: Vec<DepReq>,
    ) -> DiscoveredProject {
        DiscoveredProject {
            id: id.into(),
            path: PathBuf::from(format!("/ws/{id}")),
            git_root: Some(PathBuf::from(format!("/ws/{id}"))),
            name: id.into(),
            kind,
            version: version.into(),
            branch: "main".into(),
            git_dirty: false,
            has_skaffold,
            skaffold_files: if has_skaffold {
                vec![PathBuf::from(format!("/ws/{id}/skaffold.yaml"))]
            } else {
                vec![]
            },
            gradle_root: Some(PathBuf::from(format!("/ws/{id}"))),
            folder_group: id.into(),
            status: "idle".into(),
            depends,
            produces,
        }
    }

    #[test]
    fn plan_update_dependents_build_and_skaffold_delete_run() {
        let discovered = vec![
            proj(
                "common-lib",
                "1.4.2",
                ProjectKind::Library,
                false,
                vec![Produces::new(
                    Coordinate::new("com.example", "common-lib"),
                    "1.4.2",
                )],
                vec![],
            ),
            proj(
                "payments-api",
                "3.1.0",
                ProjectKind::Service,
                true,
                vec![],
                vec![DepReq::external(
                    Coordinate::new("com.example", "common-lib"),
                    VersionSpec::Range("[1.0.0, 2.0.0)".into()),
                    "implementation",
                    "libs.common.lib",
                )],
            ),
            proj(
                "orders-api",
                "2.0.0",
                ProjectKind::Service,
                true,
                vec![],
                vec![DepReq::external(
                    Coordinate::new("com.example", "common-lib"),
                    VersionSpec::Range("[1.0.0, 2.0.0)".into()),
                    "implementation",
                    "libs.common.lib",
                )],
            ),
            // Exact pin outside published version → warning, not consumer
            proj(
                "legacy-api",
                "1.0.0",
                ProjectKind::Service,
                true,
                vec![],
                vec![DepReq::external(
                    Coordinate::new("com.example", "common-lib"),
                    VersionSpec::Exact("1.0.0".into()),
                    "implementation",
                    "exact",
                )],
            ),
        ];
        let graph = DependencyGraph::from_projects(&discovered);
        let projects: Vec<ProjectRow> = discovered.iter().map(DiscoveredProject::to_row).collect();
        let config = Config::default();

        let plan = build_update_dependents(&projects, &graph, 0, &config).expect("plan builds");

        assert_eq!(plan.recipe, RECIPE_UPDATE_DEPENDENTS);
        assert_eq!(plan.source_name, "common-lib");
        // Services only: 2 × (pull + clean + build + delete + run) = 10
        assert_eq!(
            plan.steps.len(),
            10,
            "steps: {:?}",
            plan.steps.iter().map(|s| s.step_label()).collect::<Vec<_>>()
        );
        assert!(plan.lib_checks.is_empty(), "no intermediate libs in this fixture");

        // First service block starts with git pull
        assert_eq!(plan.steps[0].project_name, "orders-api");
        assert!(matches!(plan.steps[0].kind, CascadeStepKind::GitPull));
        assert!(matches!(
            &plan.steps[3].kind,
            CascadeStepKind::Skaffold { subcommand } if subcommand == "delete"
        ));
        assert!(matches!(
            &plan.steps[4].kind,
            CascadeStepKind::Skaffold { subcommand } if subcommand == "run"
        ));

        assert!(
            plan.warnings.iter().any(|w| w.contains("legacy-api") && w.contains("1.4.2")),
            "warnings: {:?}",
            plan.warnings
        );
        assert!(!plan.executing);
    }

    #[test]
    fn update_dependents_lists_child_libs_without_building_them() {
        // parent-lib → child-lib → service
        let parent = proj(
            "parent-lib",
            "1.0.0",
            ProjectKind::Library,
            false,
            vec![Produces::new(
                Coordinate::new("com.example", "parent-lib"),
                "1.0.0",
            )],
            vec![],
        );
        let child = proj(
            "child-lib",
            "2.0.0",
            ProjectKind::Library,
            false,
            vec![Produces::new(
                Coordinate::new("com.example", "child-lib"),
                "2.0.0",
            )],
            vec![DepReq::external(
                Coordinate::new("com.example", "parent-lib"),
                VersionSpec::Range("[1.0.0,2.0.0)".into()),
                "implementation",
                "x",
            )],
        );
        let svc = proj(
            "svc",
            "3.0.0",
            ProjectKind::Service,
            true,
            vec![],
            vec![DepReq::external(
                Coordinate::new("com.example", "child-lib"),
                VersionSpec::Range("[2.0.0,3.0.0)".into()),
                "implementation",
                "y",
            )],
        );
        let discovered = vec![parent, child, svc];
        let graph = DependencyGraph::from_projects(&discovered);
        let projects: Vec<ProjectRow> = discovered.iter().map(DiscoveredProject::to_row).collect();
        let plan = build_update_dependents(&projects, &graph, 0, &Config::default()).unwrap();

        assert_eq!(plan.lib_checks.len(), 1);
        assert_eq!(plan.lib_checks[0].project_name, "child-lib");
        // No gradle-only step for child-lib
        assert!(
            !plan
                .steps
                .iter()
                .any(|s| s.project_name == "child-lib"),
            "child lib must not be an executable step"
        );
        // Service still has work
        assert!(plan.steps.iter().any(|s| s.project_name == "svc"));
        assert!(plan.steps.iter().any(|s| matches!(s.kind, CascadeStepKind::GitPull)));
    }

    #[test]
    fn plan_skaffold_redeploy_is_delete_then_run() {
        let discovered = vec![proj(
            "payments-api",
            "3.1.0",
            ProjectKind::Service,
            true,
            vec![],
            vec![],
        )];
        let projects: Vec<ProjectRow> = discovered.iter().map(DiscoveredProject::to_row).collect();
        let plan = build_skaffold_redeploy(&projects, 0, &Config::default()).expect("plan");
        assert_eq!(plan.recipe, RECIPE_SKAFFOLD_REDEPLOY);
        assert_eq!(plan.steps.len(), 2);
        assert!(matches!(
            &plan.steps[0].kind,
            CascadeStepKind::Skaffold { subcommand } if subcommand == "delete"
        ));
        assert!(matches!(
            &plan.steps[1].kind,
            CascadeStepKind::Skaffold { subcommand } if subcommand == "run"
        ));
    }

    #[test]
    fn update_dependents_errors_when_none() {
        let discovered = vec![proj(
            "payments-api",
            "3.1.0",
            ProjectKind::Service,
            true,
            vec![],
            vec![],
        )];
        let graph = DependencyGraph::from_projects(&discovered);
        let projects: Vec<ProjectRow> = discovered.iter().map(DiscoveredProject::to_row).collect();
        let err = build_update_dependents(&projects, &graph, 0, &Config::default()).unwrap_err();
        assert!(err.contains("nothing depends"), "{err}");
    }

    #[test]
    fn plan_respects_cascade_config_tasks() {
        let discovered = vec![
            proj(
                "lib",
                "1.0.0",
                ProjectKind::Library,
                false,
                vec![Produces::new(Coordinate::new("g", "lib"), "1.0.0")],
                vec![],
            ),
            proj(
                "svc",
                "1.0.0",
                ProjectKind::Service,
                false, // no skaffold → build only
                vec![],
                vec![DepReq::external(
                    Coordinate::new("g", "lib"),
                    VersionSpec::Exact("1.0.0".into()),
                    "implementation",
                    "x",
                )],
            ),
        ];
        let graph = DependencyGraph::from_projects(&discovered);
        let projects: Vec<ProjectRow> = discovered.iter().map(DiscoveredProject::to_row).collect();
        let config = Config {
            gradle: GradleConfig {
                default_tasks_clean: vec!["clean".into()],
                default_tasks_build: vec!["assemble".into()],
                default_tasks_publish: vec!["publishToMavenLocal".into()],
                ..Default::default()
            },
            cascade: CascadeConfig {
                default_consumer_gradle: vec!["check".into()],
                default_consumer_skaffold: vec!["dev".into()],
                max_parallel: 5,
            },
            ..Default::default()
        };

        let plan = build_update_dependents(&projects, &graph, 0, &config).unwrap();
        // service: pull + clean + check (no skaffold on svc)
        assert!(
            plan.steps.len() >= 2,
            "steps: {:?}",
            plan.steps.iter().map(|s| s.step_label()).collect::<Vec<_>>()
        );
        assert!(matches!(plan.steps[0].kind, CascadeStepKind::GitPull));
        // last gradle step should be the configured consumer task "check"
        let gradle = plan
            .steps
            .iter()
            .filter_map(|s| match &s.kind {
                CascadeStepKind::Gradle { tasks, force_latest_snapshots } => {
                    Some((tasks.as_slice(), *force_latest_snapshots))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            gradle.iter().any(|(t, snap)| *t == ["check".to_string()].as_slice() && *snap),
            "expected force-snap check: {gradle:?}"
        );
    }

    #[test]
    fn finish_job_and_ready_after_source() {
        let mut plan = CascadePlan {
            recipe: RECIPE_UPDATE_DEPENDENTS.into(),
            source_id: "lib".into(),
            source_name: "lib".into(),
            source_version: "1.0".into(),
            lib_checks: vec![],
            steps: vec![
                CascadeStep {
                    kind: CascadeStepKind::Gradle {
                        tasks: vec!["publish".into()],
                        force_latest_snapshots: false,
                    },
                    project_id: "lib".into(),
                    project_name: "lib".into(),
                    project_path: PathBuf::from("/ws/lib"),
                    gradle_dir: PathBuf::from("/ws/lib"),
                    skaffold_file: None,
                    status: CascadeStepStatus::Running,
                    job_id: Some(1),
                },
                CascadeStep {
                    kind: CascadeStepKind::Gradle {
                        tasks: vec!["build".into()],
                        force_latest_snapshots: true,
                    },
                    project_id: "svc-a".into(),
                    project_name: "svc-a".into(),
                    project_path: PathBuf::from("/ws/svc-a"),
                    gradle_dir: PathBuf::from("/ws/svc-a"),
                    skaffold_file: None,
                    status: CascadeStepStatus::Pending,
                    job_id: None,
                },
                CascadeStep {
                    kind: CascadeStepKind::Gradle {
                        tasks: vec!["build".into()],
                        force_latest_snapshots: true,
                    },
                    project_id: "svc-b".into(),
                    project_name: "svc-b".into(),
                    project_path: PathBuf::from("/ws/svc-b"),
                    gradle_dir: PathBuf::from("/ws/svc-b"),
                    skaffold_file: None,
                    status: CascadeStepStatus::Pending,
                    job_id: None,
                },
            ],
            cursor: 0,
            warnings: vec![],
            executing: true,
            active_jobs: std::collections::HashMap::from([(1, 0)]),
        };

        // Before source finishes: only nothing new ready (source running).
        assert!(plan.ready_step_indices().is_empty());
        assert_eq!(plan.indices_to_dispatch(5), Vec::<usize>::new());

        assert_eq!(plan.finish_job(1, true, false), Some(0));
        assert_eq!(plan.steps[0].status, CascadeStepStatus::Ok);
        // Both consumers ready; parallel up to 5.
        assert_eq!(plan.ready_step_indices(), vec![1usize, 2]);
        assert_eq!(plan.indices_to_dispatch(5), vec![1usize, 2]);
        // Cap at 1.
        assert_eq!(plan.indices_to_dispatch(1), vec![1usize]);

        plan.register_active(2, 1);
        plan.register_active(3, 2);
        assert_eq!(plan.running_count(), 2);
        assert!(plan.indices_to_dispatch(5).is_empty());

        // Fail one consumer.
        assert_eq!(plan.finish_job(2, false, false), Some(1));
        assert_eq!(plan.steps[1].status, CascadeStepStatus::Failed);
        plan.mark_remaining_skipped();
        // svc-b still running — not skipped; only Pending is skipped.
        assert_eq!(plan.steps[2].status, CascadeStepStatus::Running);
    }

    #[test]
    fn per_project_order_blocks_skaffold_until_build() {
        let mut plan = CascadePlan {
            recipe: RECIPE_UPDATE_DEPENDENTS.into(),
            source_id: "lib".into(),
            source_name: "lib".into(),
            source_version: "1".into(),
            lib_checks: vec![],
            steps: vec![
                CascadeStep {
                    kind: CascadeStepKind::Gradle {
                        tasks: vec!["publish".into()],
                        force_latest_snapshots: false,
                    },
                    project_id: "lib".into(),
                    project_name: "lib".into(),
                    project_path: PathBuf::from("/ws/lib"),
                    gradle_dir: PathBuf::from("/ws/lib"),
                    skaffold_file: None,
                    status: CascadeStepStatus::Ok,
                    job_id: Some(1),
                },
                CascadeStep {
                    kind: CascadeStepKind::Gradle {
                        tasks: vec!["build".into()],
                        force_latest_snapshots: true,
                    },
                    project_id: "svc".into(),
                    project_name: "svc".into(),
                    project_path: PathBuf::from("/ws/svc"),
                    gradle_dir: PathBuf::from("/ws/svc"),
                    skaffold_file: None,
                    status: CascadeStepStatus::Running,
                    job_id: Some(2),
                },
                CascadeStep {
                    kind: CascadeStepKind::Skaffold {
                        subcommand: "run".into(),
                    },
                    project_id: "svc".into(),
                    project_name: "svc".into(),
                    project_path: PathBuf::from("/ws/svc"),
                    gradle_dir: PathBuf::from("/ws/svc"),
                    skaffold_file: Some(PathBuf::from("/ws/svc/skaffold.yaml")),
                    status: CascadeStepStatus::Pending,
                    job_id: None,
                },
            ],
            cursor: 0,
            warnings: vec![],
            executing: true,
            active_jobs: std::collections::HashMap::from([(2, 1)]),
        };
        // Skaffold not ready while build running.
        assert!(plan.ready_step_indices().is_empty());
        plan.finish_job(2, true, false);
        assert_eq!(plan.ready_step_indices(), vec![2]);
    }

    #[test]
    fn unrelated_job_id_ignored() {
        let mut plan = CascadePlan {
            recipe: RECIPE_UPDATE_DEPENDENTS.into(),
            source_id: "lib".into(),
            source_name: "lib".into(),
            source_version: "1".into(),
            lib_checks: vec![],
            steps: vec![],
            cursor: 0,
            warnings: vec![],
            executing: true,
            active_jobs: std::collections::HashMap::from([(10, 0)]),
        };
        assert!(plan.finish_job(99, true, false).is_none());
        assert!(plan.active_jobs.contains_key(&10));
    }
}
