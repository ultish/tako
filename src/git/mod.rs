//! Git metadata helpers: nearest git root, branch, dirty status.
//!
//! Shells out to `git` (no libgit2). Callers that scan many projects under one
//! monorepo should use [`GitStatusCache`] so branch/dirty are resolved once per
//! `git_root`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Walk ancestors of `path` (inclusive) and return the nearest directory that
/// contains a `.git` entry (directory or file, as in worktrees).
pub fn find_git_root(path: &Path) -> Option<PathBuf> {
    let mut cur = if path.exists() {
        path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
    } else {
        path.to_path_buf()
    };

    loop {
        if cur.join(".git").exists() {
            return Some(cur);
        }
        if !cur.pop() {
            return None;
        }
    }
}

/// Current branch name via `git`.
///
/// Tries, in order:
/// 1. `git rev-parse --abbrev-ref HEAD` (normal case)
/// 2. `git symbolic-ref --short HEAD` (empty repos where rev-parse fails)
/// 3. Parse `.git/HEAD` on disk (works even when git refuses empty branches)
///
/// Returns `"—"` when all strategies fail.
pub fn git_branch(git_root: &Path) -> String {
    if let Some(s) = run_git(git_root, &["rev-parse", "--abbrev-ref", "HEAD"]) {
        if !s.is_empty() && s != "HEAD" {
            return s;
        }
    }
    if let Some(s) = run_git(git_root, &["symbolic-ref", "--short", "HEAD"]) {
        if !s.is_empty() {
            return s;
        }
    }
    if let Some(s) = branch_from_head_file(git_root) {
        return s;
    }
    "—".to_string()
}

/// Read branch from `.git/HEAD` (`ref: refs/heads/…` or short detached SHA).
fn branch_from_head_file(git_root: &Path) -> Option<String> {
    let head_path = git_root.join(".git").join("HEAD");
    let contents = if head_path.is_file() {
        fs::read_to_string(&head_path).ok()?
    } else {
        // Worktree: `.git` is a file with `gitdir: …`
        let git_file = fs::read_to_string(git_root.join(".git")).ok()?;
        let git_dir = git_file.lines().find_map(|line| {
            line.trim()
                .strip_prefix("gitdir:")
                .map(|p| PathBuf::from(p.trim()))
        })?;
        let git_dir = if git_dir.is_absolute() {
            git_dir
        } else {
            git_root.join(git_dir)
        };
        fs::read_to_string(git_dir.join("HEAD")).ok()?
    };
    parse_head_contents(&contents)
}

fn parse_head_contents(contents: &str) -> Option<String> {
    let line = contents.lines().next()?.trim();
    if let Some(branch) = line.strip_prefix("ref: refs/heads/") {
        if !branch.is_empty() {
            return Some(branch.to_string());
        }
    }
    // Detached HEAD — short SHA.
    if line.len() >= 7 && line.chars().all(|c| c.is_ascii_hexdigit()) {
        return Some(line.chars().take(7).collect());
    }
    None
}

/// `true` when `git status --porcelain` reports any output (staged, unstaged, untracked).
/// Returns `false` on failure (treat unknown as clean for display).
pub fn git_dirty(git_root: &Path) -> bool {
    match run_git(git_root, &["status", "--porcelain"]) {
        Some(s) => !s.is_empty(),
        None => false,
    }
}

fn run_git(git_root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(git_root)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn run_git_result(git_root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(git_root)
        .args(args)
        .output()
        .map_err(|e| format!("spawn git: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = if !stderr.trim().is_empty() {
            stderr.trim().to_string()
        } else {
            stdout.trim().to_string()
        };
        return Err(if detail.is_empty() {
            format!("git {:?} failed", args)
        } else {
            detail
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Stage `paths` (absolute or relative to `git_root`) and create a commit.
///
/// Returns `Ok(true)` if a commit was created, `Ok(false)` if there was nothing
/// to commit after staging (already clean).
pub fn commit_paths(
    git_root: &Path,
    paths: &[PathBuf],
    message: &str,
) -> Result<bool, String> {
    if paths.is_empty() {
        return Ok(false);
    }
    if message.trim().is_empty() {
        return Err("empty commit message".into());
    }

    let mut add_args: Vec<String> = vec!["add".into(), "--".into()];
    for p in paths {
        // Prefer path relative to git root for cleaner index entries.
        let rel = p
            .strip_prefix(git_root)
            .map(|r| r.to_path_buf())
            .unwrap_or_else(|_| p.clone());
        add_args.push(rel.to_string_lossy().into_owned());
    }
    let add_refs: Vec<&str> = add_args.iter().map(String::as_str).collect();
    run_git_result(git_root, &add_refs)?;

    // Nothing staged → no commit (e.g. file already matched HEAD).
    let staged = run_git_result(git_root, &["diff", "--cached", "--name-only"])?;
    if staged.is_empty() {
        return Ok(false);
    }

    run_git_result(git_root, &["commit", "-m", message])?;
    Ok(true)
}

/// Cached branch + dirty + remote lag for a single git root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitInfo {
    pub branch: String,
    pub dirty: bool,
    /// Commits on upstream not in HEAD (`git rev-list --count HEAD..@{u}`).
    pub behind: Option<u32>,
    /// Commits on HEAD not on upstream.
    pub ahead: Option<u32>,
    /// Whether we successfully resolved an upstream tracking branch.
    pub has_upstream: bool,
}

impl GitInfo {
    /// Short projects-table cell: `↓3`, `↑2`, `↓1↑2`, `dirty`, `ok`, `—`.
    pub fn sync_label(&self) -> String {
        if self.branch == "—" {
            return "—".into();
        }
        let mut parts = Vec::new();
        if let Some(b) = self.behind {
            if b > 0 {
                parts.push(format!("↓{b}"));
            }
        }
        if let Some(a) = self.ahead {
            if a > 0 {
                parts.push(format!("↑{a}"));
            }
        }
        if self.dirty {
            parts.push("·".into()); // dirty marker after lag
        }
        if parts.is_empty() {
            if self.has_upstream {
                if self.dirty {
                    "dirty".into()
                } else {
                    "ok".into()
                }
            } else if self.dirty {
                "dirty".into()
            } else {
                "—".into() // no upstream configured
            }
        } else if parts.len() == 1 && parts[0] == "·" {
            "dirty".into()
        } else {
            // "↓3·" → "↓3" + dirty implied, or join
            let dirty = self.dirty;
            let lag: String = parts
                .into_iter()
                .filter(|p| p != "·")
                .collect::<Vec<_>>()
                .join("");
            if dirty && lag.is_empty() {
                "dirty".into()
            } else if dirty {
                format!("{lag}*")
            } else {
                lag
            }
        }
    }
}

/// Fetch + lag counts for one root. `fetch` updates remote refs when true.
pub fn git_remote_sync(git_root: &Path, fetch: bool) -> GitInfo {
    let branch = git_branch(git_root);
    let dirty = git_dirty(git_root);
    if branch == "—" {
        return GitInfo {
            branch,
            dirty,
            behind: None,
            ahead: None,
            has_upstream: false,
        };
    }
    if fetch {
        // Best-effort; ignore failures (offline, auth).
        let _ = run_git(git_root, &["fetch", "--quiet", "--prune"]);
    }
    let upstream = run_git(git_root, &["rev-parse", "--abbrev-ref", "@{upstream}"]);
    let has_upstream = upstream.as_ref().is_some_and(|u| !u.is_empty());
    let (behind, ahead) = if has_upstream {
        let behind = run_git(git_root, &["rev-list", "--count", "HEAD..@{upstream}"])
            .and_then(|s| s.parse().ok());
        let ahead = run_git(git_root, &["rev-list", "--count", "@{upstream}..HEAD"])
            .and_then(|s| s.parse().ok());
        (behind, ahead)
    } else {
        (None, None)
    };
    GitInfo {
        branch,
        dirty,
        behind,
        ahead,
        has_upstream,
    }
}

/// Per-scan cache so monorepo multi-skaffold rows share one status query.
#[derive(Debug, Default)]
pub struct GitStatusCache {
    inner: HashMap<PathBuf, GitInfo>,
}

impl GitStatusCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolve (and cache) branch + dirty for `git_root` (no fetch — scan-time).
    pub fn info(&mut self, git_root: &Path) -> &GitInfo {
        if !self.inner.contains_key(git_root) {
            let info = GitInfo {
                branch: git_branch(git_root),
                dirty: git_dirty(git_root),
                behind: None,
                ahead: None,
                has_upstream: false,
            };
            self.inner.insert(git_root.to_path_buf(), info);
        }
        self.inner.get(git_root).expect("just inserted")
    }

    pub fn branch(&mut self, git_root: &Path) -> String {
        self.info(git_root).branch.clone()
    }

    pub fn dirty(&mut self, git_root: &Path) -> bool {
        self.info(git_root).dirty
    }

    /// Replace cache entry with full remote sync (used by **r** stats).
    pub fn put(&mut self, git_root: PathBuf, info: GitInfo) {
        self.inner.insert(git_root, info);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
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
            "tako-git-test-{}-{}-{}-{}",
            label,
            std::process::id(),
            nanos,
            n
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn find_git_root_with_plain_git_file() {
        let root = temp_dir("plain-file");
        fs::write(root.join(".git"), "gitdir: /somewhere\n").expect("write .git file");
        let nested = root.join("a").join("b");
        fs::create_dir_all(&nested).expect("nested");
        assert_eq!(find_git_root(&nested), Some(root.canonicalize().unwrap()));
        assert_eq!(find_git_root(&root), Some(root.canonicalize().unwrap()));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn commit_paths_creates_commit() {
        let dir = temp_dir("commit");
        assert!(Command::new("git")
            .args(["init"])
            .current_dir(&dir)
            .status()
            .unwrap()
            .success());
        // Identity for commit in CI/sandbox.
        let _ = Command::new("git")
            .args(["-C"])
            .arg(&dir)
            .args(["config", "user.email", "tako@test"])
            .status();
        let _ = Command::new("git")
            .args(["-C"])
            .arg(&dir)
            .args(["config", "user.name", "tako"])
            .status();

        let f = dir.join("build.gradle.kts");
        fs::write(&f, "version = \"1.0.0\"\n").unwrap();
        assert!(Command::new("git")
            .args(["-C"])
            .arg(&dir)
            .args(["add", "build.gradle.kts"])
            .status()
            .unwrap()
            .success());
        assert!(Command::new("git")
            .args(["-C"])
            .arg(&dir)
            .args(["commit", "-m", "init"])
            .status()
            .unwrap()
            .success());

        fs::write(&f, "version = \"1.0.1\"\n").unwrap();
        let msg = "Bump versions due to common-lib dependency\n\n- svc: 1.0.0 → 1.0.1\n";
        let created = commit_paths(&dir, &[f], msg).expect("commit");
        assert!(created);
        let log = run_git(&dir, &["log", "-1", "--pretty=%B"]).expect("log");
        assert!(log.contains("Bump versions due to common-lib dependency"));
        assert!(log.contains("1.0.0 → 1.0.1"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_git_root_none_without_git() {
        let root = temp_dir("no-git");
        assert!(find_git_root(&root).is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn git_branch_and_dirty_via_real_repo() {
        let root = temp_dir("real-repo");
        let status = Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(&root)
            .output()
            .expect("git init");
        assert!(
            status.status.success(),
            "git init failed: {}",
            String::from_utf8_lossy(&status.stderr)
        );
        let root_s = root.to_str().unwrap();
        let _ = Command::new("git")
            .args(["-C", root_s, "config", "user.email", "tako@test"])
            .status();
        let _ = Command::new("git")
            .args(["-C", root_s, "config", "user.name", "tako"])
            .status();

        let branch = git_branch(&root);
        assert_eq!(branch, "main");

        fs::write(root.join("README"), "hi").expect("write");
        assert!(git_dirty(&root), "untracked file should mark dirty");

        let mut cache = GitStatusCache::new();
        assert_eq!(cache.branch(&root), "main");
        assert!(cache.dirty(&root));
        assert_eq!(cache.branch(&root), "main");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn git_branch_unknown_returns_em_dash() {
        let root = temp_dir("not-a-repo");
        assert_eq!(git_branch(&root), "—");
        assert!(!git_dirty(&root));
        let _ = fs::remove_dir_all(&root);
    }
}
