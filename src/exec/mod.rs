//! Child-process execution: command planning + runner for gradle / skaffold / git.
//!
//! **Always invoke `gradle` from PATH** (or an absolute override in config) —
//! never `./gradlew`. Air-gapped machines use a preinstalled Gradle.
//!
//! Multi-step recipes (update dependents / skaffold redeploy) live in [`cascade`].

mod args;
mod cascade;
mod init_script;
mod runner;

pub use args::{
    git_pull_argv, gradle_argv, skaffold_argv, GitPullPlan, GradlePlan, PlannedCommand,
    SkaffoldPlan,
};
#[allow(unused_imports)] // public surface for cascade UI / recipes
pub use cascade::{
    build_skaffold_redeploy, build_update_dependents, recipe_summary, recipe_title,
    skaffold_redeploy_steps, source_publish_tasks, CascadePlan, CascadeStep, CascadeStepKind,
    CascadeStepStatus, LibNexusRow, RECIPE_SKAFFOLD_REDEPLOY, RECIPE_UPDATE_DEPENDENTS,
};
pub use init_script::ensure_snapshot_init_script;
pub use runner::{spawn_and_stream, SpawnOpts};

#[allow(unused_imports)] // public API for cancel helpers / future supervisors
pub use runner::kill_process_group;
