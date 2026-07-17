//! Pure command-line planners (no spawn). Unit-tested offline.

use std::path::{Path, PathBuf};

/// A planned process invocation: program + argv + optional working directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedCommand {
    pub program: String,
    pub args: Vec<String>,
    /// When set, child `current_dir` is this path.
    pub cwd: Option<PathBuf>,
}

impl PlannedCommand {
    pub fn display_line(&self) -> String {
        let mut parts = Vec::with_capacity(1 + self.args.len());
        parts.push(self.program.as_str());
        for a in &self.args {
            parts.push(a.as_str());
        }
        parts.join(" ")
    }
}

/// Inputs for a Gradle invocation (`gradle` on PATH only — never the wrapper).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GradlePlan<'a> {
    /// Binary name or absolute path (`"gradle"` by default).
    pub command: &'a str,
    /// Project directory passed as `-p`.
    pub project_dir: &'a Path,
    /// Task names, e.g. `["build"]` or `["clean", "build"]`.
    pub tasks: &'a [String],
    /// Optional `--init-script` (e.g. force SNAPSHOT re-resolve on consumers).
    pub init_script: Option<&'a Path>,
}

/// Inputs for a Skaffold invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkaffoldPlan<'a> {
    pub command: &'a str,
    /// Subcommand: `dev`, `debug`, `delete`, `run`, …
    pub subcommand: &'a str,
    /// Absolute (or relative) path to the skaffold yaml.
    pub skaffold_file: &'a Path,
    /// Extra args after the subcommand / `-f` (profile, port-forward, …).
    pub extra_args: &'a [String],
}

/// Inputs for `git pull`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitPullPlan<'a> {
    pub git_root: &'a Path,
    /// When true, append `--ff-only` (default per SPEC).
    pub ff_only: bool,
    /// Extra args after `pull` / `--ff-only`.
    pub extra_args: &'a [String],
}

/// Build `gradle [-I <init>] -p <path> <tasks…>` (never `./gradlew`).
pub fn gradle_argv(plan: &GradlePlan<'_>) -> PlannedCommand {
    let mut args = Vec::with_capacity(4 + plan.tasks.len());
    if let Some(init) = plan.init_script {
        args.push("--init-script".into());
        args.push(init.display().to_string());
    }
    args.push("-p".into());
    args.push(plan.project_dir.display().to_string());
    for t in plan.tasks {
        args.push(t.clone());
    }
    PlannedCommand {
        program: plan.command.to_string(),
        args,
        cwd: None,
    }
}

/// Build `skaffold <sub> -f <file> [extra…]` with cwd = skaffold file's parent.
pub fn skaffold_argv(plan: &SkaffoldPlan<'_>) -> PlannedCommand {
    let mut args = Vec::with_capacity(3 + plan.extra_args.len());
    args.push(plan.subcommand.to_string());
    args.push("-f".into());
    // Prefer the absolute path in `-f` so cwd is just a safety net.
    args.push(plan.skaffold_file.display().to_string());
    for a in plan.extra_args {
        args.push(a.clone());
    }
    let cwd = plan
        .skaffold_file
        .parent()
        .map(|p| p.to_path_buf())
        .filter(|p| !p.as_os_str().is_empty());
    PlannedCommand {
        program: plan.command.to_string(),
        args,
        cwd,
    }
}

/// Build `git -C <root> pull [--ff-only] [extra…]`.
pub fn git_pull_argv(plan: &GitPullPlan<'_>) -> PlannedCommand {
    let mut args = Vec::with_capacity(4 + plan.extra_args.len());
    args.push("-C".into());
    args.push(plan.git_root.display().to_string());
    args.push("pull".into());
    if plan.ff_only {
        args.push("--ff-only".into());
    }
    for a in plan.extra_args {
        args.push(a.clone());
    }
    PlannedCommand {
        program: "git".into(),
        args,
        cwd: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn gradle_build_args() {
        let tasks = vec!["build".into()];
        let plan = GradlePlan {
            command: "gradle",
            project_dir: Path::new("/tmp/payments-api"),
            tasks: &tasks,
            init_script: None,
        };
        let cmd = gradle_argv(&plan);
        assert_eq!(cmd.program, "gradle");
        assert_eq!(
            cmd.args,
            vec!["-p", "/tmp/payments-api", "build"]
        );
        assert!(cmd.cwd.is_none());
        assert_eq!(cmd.display_line(), "gradle -p /tmp/payments-api build");
    }

    #[test]
    fn gradle_clean_publish_multiple_tasks() {
        let tasks = vec!["clean".into(), "publish".into()];
        let plan = GradlePlan {
            command: "gradle",
            project_dir: Path::new("/repo/lib"),
            tasks: &tasks,
            init_script: None,
        };
        let cmd = gradle_argv(&plan);
        assert_eq!(
            cmd.args,
            vec!["-p", "/repo/lib", "clean", "publish"]
        );
    }

    #[test]
    fn gradle_custom_command_path() {
        let tasks = vec!["build".into()];
        let plan = GradlePlan {
            command: "/opt/gradle/bin/gradle",
            project_dir: Path::new("/p"),
            tasks: &tasks,
            init_script: None,
        };
        assert_eq!(gradle_argv(&plan).program, "/opt/gradle/bin/gradle");
    }

    #[test]
    fn gradle_init_script_before_project_dir() {
        let tasks = vec!["build".into()];
        let init = Path::new("/tmp/tako/cache/force-latest-snapshots.gradle");
        let plan = GradlePlan {
            command: "gradle",
            project_dir: Path::new("/svc"),
            tasks: &tasks,
            init_script: Some(init),
        };
        let cmd = gradle_argv(&plan);
        assert_eq!(
            cmd.args,
            vec![
                "--init-script",
                "/tmp/tako/cache/force-latest-snapshots.gradle",
                "-p",
                "/svc",
                "build"
            ]
        );
    }

    #[test]
    fn skaffold_dev_with_file_and_cwd() {
        let extra: Vec<String> = vec![];
        let plan = SkaffoldPlan {
            command: "skaffold",
            subcommand: "dev",
            skaffold_file: Path::new("/mono/orders/skaffold.yaml"),
            extra_args: &extra,
        };
        let cmd = skaffold_argv(&plan);
        assert_eq!(cmd.program, "skaffold");
        assert_eq!(
            cmd.args,
            vec!["dev", "-f", "/mono/orders/skaffold.yaml"]
        );
        assert_eq!(cmd.cwd.as_deref(), Some(Path::new("/mono/orders")));
    }

    #[test]
    fn skaffold_debug_extra_args() {
        let extra = vec!["--port-forward".into()];
        let plan = SkaffoldPlan {
            command: "skaffold",
            subcommand: "debug",
            skaffold_file: Path::new("/svc/skaffold.yml"),
            extra_args: &extra,
        };
        let cmd = skaffold_argv(&plan);
        assert_eq!(
            cmd.args,
            vec![
                "debug",
                "-f",
                "/svc/skaffold.yml",
                "--port-forward"
            ]
        );
    }

    #[test]
    fn git_pull_ff_only() {
        let extra: Vec<String> = vec![];
        let plan = GitPullPlan {
            git_root: Path::new("/Users/you/repo"),
            ff_only: true,
            extra_args: &extra,
        };
        let cmd = git_pull_argv(&plan);
        assert_eq!(cmd.program, "git");
        assert_eq!(
            cmd.args,
            vec!["-C", "/Users/you/repo", "pull", "--ff-only"]
        );
    }

    #[test]
    fn git_pull_without_ff_only() {
        let extra: Vec<String> = vec![];
        let plan = GitPullPlan {
            git_root: Path::new("/repo"),
            ff_only: false,
            extra_args: &extra,
        };
        let cmd = git_pull_argv(&plan);
        assert_eq!(cmd.args, vec!["-C", "/repo", "pull"]);
    }
}
