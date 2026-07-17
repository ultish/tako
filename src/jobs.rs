//! In-memory job console model: id, kind, status, ring-buffered logs.

use std::path::PathBuf;
use std::time::Instant;

use crate::ring_buffer::RingBuffer;

/// How many log lines are retained per job (oldest evicted).
pub const JOB_LOG_CAPACITY: usize = 5_000;

/// What kind of external process a job is running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobKind {
    Gradle { tasks: Vec<String> },
    Skaffold { subcommand: String },
    GitPull,
}

impl JobKind {
    /// Short label for the jobs list column.
    pub fn label(&self) -> String {
        match self {
            JobKind::Gradle { tasks } => {
                if tasks.is_empty() {
                    "gradle".into()
                } else {
                    tasks.join(" ")
                }
            }
            JobKind::Skaffold { subcommand } => format!("skaffold {subcommand}"),
            JobKind::GitPull => "git pull".into(),
        }
    }

    /// Status-chip verb while the job is running (e.g. `"build…"`).
    pub fn running_chip(&self) -> String {
        match self {
            JobKind::Gradle { tasks } => {
                let head = tasks.first().map(String::as_str).unwrap_or("gradle");
                format!("{head}…")
            }
            JobKind::Skaffold { subcommand } => format!("{subcommand}…"),
            JobKind::GitPull => "pull…".into(),
        }
    }

    /// Status-chip text when the job ends successfully.
    pub fn ok_chip(&self) -> &'static str {
        match self {
            JobKind::GitPull => "pull_ok",
            _ => "ok",
        }
    }

    /// Status-chip text when the job fails.
    pub fn fail_chip(&self) -> &'static str {
        match self {
            JobKind::GitPull => "pull_failed",
            _ => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Pending,
    Running,
    Ok,
    Failed,
    Cancelled,
}

impl JobStatus {
    pub fn label(self) -> &'static str {
        match self {
            JobStatus::Pending => "pend",
            JobStatus::Running => "run",
            JobStatus::Ok => "ok",
            JobStatus::Failed => "fail",
            JobStatus::Cancelled => "cancel",
        }
    }

    pub fn is_active(self) -> bool {
        matches!(self, JobStatus::Pending | JobStatus::Running)
    }
}

/// One background job shown in the Jobs console.
#[derive(Debug)]
pub struct Job {
    pub id: u64,
    pub project_name: String,
    /// Absolute project path (for status-chip updates on the matching row).
    pub project_path: PathBuf,
    /// Git root when this is a pull (for multi-select dedupe by root).
    pub git_root: Option<PathBuf>,
    pub kind: JobKind,
    pub status: JobStatus,
    pub log: RingBuffer<String>,
    /// Short exit summary (`exit 0`, `cancelled`, stderr snippet, …).
    pub summary: Option<String>,
    pub started_at: Instant,
    pub finished_at: Option<Instant>,
}

impl Job {
    pub fn new(
        id: u64,
        project_name: impl Into<String>,
        project_path: impl Into<PathBuf>,
        kind: JobKind,
    ) -> Self {
        Self {
            id,
            project_name: project_name.into(),
            project_path: project_path.into(),
            git_root: None,
            kind,
            status: JobStatus::Pending,
            log: RingBuffer::new(JOB_LOG_CAPACITY),
            summary: None,
            started_at: Instant::now(),
            finished_at: None,
        }
    }

    pub fn with_git_root(mut self, git_root: impl Into<PathBuf>) -> Self {
        self.git_root = Some(git_root.into());
        self
    }

    pub fn push_log(&mut self, line: String) {
        self.log.push(line);
    }

    pub fn mark_running(&mut self) {
        self.status = JobStatus::Running;
        self.started_at = Instant::now();
    }

    pub fn mark_finished(&mut self, ok: bool, cancelled: bool, summary: impl Into<String>) {
        self.finished_at = Some(Instant::now());
        self.summary = Some(summary.into());
        self.status = if cancelled {
            JobStatus::Cancelled
        } else if ok {
            JobStatus::Ok
        } else {
            JobStatus::Failed
        };
    }

    /// Elapsed wall time for display (running → now; finished → end).
    pub fn elapsed_secs(&self) -> u64 {
        let end = self.finished_at.unwrap_or_else(Instant::now);
        end.duration_since(self.started_at).as_secs()
    }

    /// Join log lines for the log pane (newest retained in ring order).
    pub fn log_text(&self) -> String {
        let mut out = String::new();
        for (i, line) in self.log.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(line);
        }
        out
    }
}
