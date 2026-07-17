//! Cascade recipes: build a confirmable plan, then run steps sequentially.
//!
//! - [`RECIPE_PUBLISH_AND_REBUILD`] (**B**) — publish producer, then gradle build
//!   each range-aware consumer (no skaffold). Consumer builds may inject a
//!   SNAPSHOT-refresh init script.
//! - [`RECIPE_PUBLISH_AND_REDEPLOY`] (**P**) — same plus skaffold delete/run per consumer.

use std::path::PathBuf;

use crate::app::ProjectRow;
use crate::config::Config;
use crate::graph::DependencyGraph;

/// Recipe id for **B**: publish → rebuild dependents (no skaffold).
pub const RECIPE_PUBLISH_AND_REBUILD: &str = "publish_and_rebuild_consumers";

/// Recipe id for **P**: publish → rebuild + skaffold redeploy dependents.
pub const RECIPE_PUBLISH_AND_REDEPLOY: &str = "publish_and_redeploy_consumers";

/// Kind of work a cascade step performs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CascadeStepKind {
    /// Gradle tasks; `force_latest_snapshots` adds tako's SNAPSHOT init script.
    Gradle {
        tasks: Vec<String>,
        force_latest_snapshots: bool,
    },
    Skaffold { subcommand: String },
}

impl CascadeStepKind {
    /// Column label for the plan table (`gradle clean build publish`, `skaffold run`).
    pub fn step_label(&self) -> String {
        match self {
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
                    format!("{base} (snap)")
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

    /// True once every source_id step is Ok or Skipped.
    fn source_barrier_open(&self) -> bool {
        let mut saw_source = false;
        for step in &self.steps {
            if step.project_id != self.source_id {
                continue;
            }
            saw_source = true;
            match step.status {
                CascadeStepStatus::Ok | CascadeStepStatus::Skipped => {}
                _ => return false,
            }
        }
        // No explicit source steps → open (degenerate plan).
        saw_source || self.steps.is_empty()
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

/// Build the **B** plan: publish source, then rebuild dependents (no skaffold).
pub fn build_publish_and_rebuild(
    projects: &[ProjectRow],
    graph: &DependencyGraph,
    source_index: usize,
    config: &Config,
) -> Result<CascadePlan, String> {
    build_publish_cascade(projects, graph, source_index, config, false)
}

/// Build the **P** plan: publish source, rebuild + skaffold redeploy dependents.
pub fn build_publish_and_redeploy(
    projects: &[ProjectRow],
    graph: &DependencyGraph,
    source_index: usize,
    config: &Config,
) -> Result<CascadePlan, String> {
    build_publish_cascade(projects, graph, source_index, config, true)
}

/// Shared plan builder for publish → consumers.
///
/// Consumers = [`DependencyGraph::dependents_of`] (project edges + range-aware coords).
/// Warnings include catalog range exclusions for the published version.
fn build_publish_cascade(
    projects: &[ProjectRow],
    graph: &DependencyGraph,
    source_index: usize,
    config: &Config,
    include_skaffold: bool,
) -> Result<CascadePlan, String> {
    let source = projects
        .get(source_index)
        .ok_or_else(|| "no project selected".to_string())?;
    let source_id = graph
        .id_at(source_index)
        .unwrap_or(source.name.as_str())
        .to_string();

    let publish_tasks = source_publish_tasks(config);
    if publish_tasks.is_empty() {
        return Err("no gradle publish tasks configured".into());
    }

    let mut steps = Vec::new();

    // 1) Source: clean / build / publish (no snapshot init — we are the publisher)
    steps.push(CascadeStep {
        kind: CascadeStepKind::Gradle {
            tasks: publish_tasks,
            force_latest_snapshots: false,
        },
        project_id: source_id.clone(),
        project_name: source.name.clone(),
        project_path: source.path.clone(),
        gradle_dir: source.gradle_project_dir().to_path_buf(),
        skaffold_file: source.skaffold_path.clone(),
        status: CascadeStepStatus::Pending,
        job_id: None,
    });

    // 2) Consumers (alphabetical from graph)
    let consumers = graph.dependents_of(&source_id);
    let consumer_gradle = if config.cascade.default_consumer_gradle.is_empty() {
        vec!["build".to_string()]
    } else {
        config.cascade.default_consumer_gradle.clone()
    };
    let consumer_skaffold = if config.cascade.default_consumer_skaffold.is_empty() {
        vec!["delete".into(), "run".into()]
    } else {
        config.cascade.default_consumer_skaffold.clone()
    };
    let force_snaps = config.gradle.force_latest_snapshots;

    for consumer_id in &consumers {
        let Some(idx) = graph.index_of(consumer_id) else {
            continue;
        };
        let Some(row) = projects.get(idx) else {
            continue;
        };

        steps.push(CascadeStep {
            kind: CascadeStepKind::Gradle {
                tasks: consumer_gradle.clone(),
                force_latest_snapshots: force_snaps,
            },
            project_id: consumer_id.clone(),
            project_name: row.name.clone(),
            project_path: row.path.clone(),
            gradle_dir: row.gradle_project_dir().to_path_buf(),
            skaffold_file: row.skaffold_path.clone(),
            status: CascadeStepStatus::Pending,
            job_id: None,
        });

        if include_skaffold && (row.has_skaffold || row.skaffold_path.is_some()) {
            let skaffold_file = row.skaffold_path.clone();
            for sub in &consumer_skaffold {
                steps.push(CascadeStep {
                    kind: CascadeStepKind::Skaffold {
                        subcommand: sub.clone(),
                    },
                    project_id: consumer_id.clone(),
                    project_name: row.name.clone(),
                    project_path: row.path.clone(),
                    gradle_dir: row.gradle_project_dir().to_path_buf(),
                    skaffold_file: skaffold_file.clone(),
                    status: CascadeStepStatus::Pending,
                    job_id: None,
                });
            }
        }
    }

    // Range mismatch warnings for published coords
    let mut warnings = Vec::new();
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

    if force_snaps && !consumers.is_empty() {
        warnings.push(
            "consumer gradle uses tako init script (cacheChangingModulesFor 0) so SNAPSHOTs re-resolve"
                .into(),
        );
    }

    let max_p = config.cascade.max_parallel.max(1);
    if max_p > 1 && !consumers.is_empty() {
        warnings.push(format!(
            "up to {max_p} consumer steps in parallel after publish (per-project order preserved)"
        ));
    }

    if produces.is_empty() && consumers.is_empty() {
        warnings.push(
            "no produces/depends edges found — plan is publish-only; re-scan if graph is stale"
                .into(),
        );
    } else if consumers.is_empty() {
        warnings.push("no consumers matched (range/project deps) — publish only".into());
    }

    let recipe = if include_skaffold {
        RECIPE_PUBLISH_AND_REDEPLOY
    } else {
        RECIPE_PUBLISH_AND_REBUILD
    };

    Ok(CascadePlan {
        recipe: recipe.into(),
        source_id,
        source_name: source.name.clone(),
        source_version: source.version.clone(),
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
    fn plan_publish_then_consumer_build_and_skaffold() {
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

        let plan =
            build_publish_and_redeploy(&projects, &graph, 0, &config).expect("plan builds");

        assert_eq!(plan.recipe, RECIPE_PUBLISH_AND_REDEPLOY);
        assert_eq!(plan.source_name, "common-lib");
        assert_eq!(plan.source_version, "1.4.2");

        // source publish + 2 consumers × (build + delete + run) = 1 + 6 = 7
        assert_eq!(plan.steps.len(), 7, "steps: {:?}", plan.steps.iter().map(|s| s.step_label()).collect::<Vec<_>>());

        // Step 1: clean build publish on common-lib (no snap init on publisher)
        assert!(matches!(
            &plan.steps[0].kind,
            CascadeStepKind::Gradle {
                tasks,
                force_latest_snapshots: false
            } if tasks == &["clean".to_string(), "build".into(), "publish".into()]
        ));
        assert_eq!(plan.steps[0].project_name, "common-lib");

        // Consumers alphabetical: orders-api, payments-api
        assert_eq!(plan.steps[1].project_name, "orders-api");
        assert!(matches!(
            &plan.steps[1].kind,
            CascadeStepKind::Gradle {
                tasks,
                force_latest_snapshots: true
            } if tasks == &["build".to_string()]
        ));
        assert!(matches!(
            &plan.steps[2].kind,
            CascadeStepKind::Skaffold { subcommand } if subcommand == "delete"
        ));
        assert!(matches!(
            &plan.steps[3].kind,
            CascadeStepKind::Skaffold { subcommand } if subcommand == "run"
        ));

        assert_eq!(plan.steps[4].project_name, "payments-api");

        // Range exclusion warning for legacy-api
        assert!(
            plan.warnings.iter().any(|w| w.contains("legacy-api") && w.contains("1.4.2")),
            "warnings: {:?}",
            plan.warnings
        );
        assert!(!plan.executing);
    }

    #[test]
    fn plan_rebuild_only_skips_skaffold() {
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
        ];
        let graph = DependencyGraph::from_projects(&discovered);
        let projects: Vec<ProjectRow> = discovered.iter().map(DiscoveredProject::to_row).collect();
        let plan =
            build_publish_and_rebuild(&projects, &graph, 0, &Config::default()).expect("plan");

        assert_eq!(plan.recipe, RECIPE_PUBLISH_AND_REBUILD);
        // source publish + consumer build only (no delete/run)
        assert_eq!(plan.steps.len(), 2, "steps: {:?}", plan.steps.iter().map(|s| s.step_label()).collect::<Vec<_>>());
        assert!(matches!(
            &plan.steps[1].kind,
            CascadeStepKind::Gradle {
                force_latest_snapshots: true,
                ..
            }
        ));
        assert!(plan
            .warnings
            .iter()
            .any(|w| w.contains("cacheChangingModulesFor")));
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

        let plan = build_publish_and_redeploy(&projects, &graph, 0, &config).unwrap();
        // source: clean assemble publishToMavenLocal
        match &plan.steps[0].kind {
            CascadeStepKind::Gradle { tasks, .. } => {
                assert_eq!(tasks, &["clean", "assemble", "publishToMavenLocal"]);
            }
            _ => panic!("expected gradle"),
        }
        // consumer: check only (no skaffold)
        assert_eq!(plan.steps.len(), 2);
        match &plan.steps[1].kind {
            CascadeStepKind::Gradle { tasks, .. } => assert_eq!(tasks, &["check"]),
            _ => panic!("expected gradle"),
        }
    }

    #[test]
    fn finish_job_and_ready_after_source() {
        let mut plan = CascadePlan {
            recipe: RECIPE_PUBLISH_AND_REDEPLOY.into(),
            source_id: "lib".into(),
            source_name: "lib".into(),
            source_version: "1.0".into(),
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
            recipe: RECIPE_PUBLISH_AND_REDEPLOY.into(),
            source_id: "lib".into(),
            source_name: "lib".into(),
            source_version: "1".into(),
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
            recipe: RECIPE_PUBLISH_AND_REDEPLOY.into(),
            source_id: "lib".into(),
            source_name: "lib".into(),
            source_version: "1".into(),
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
