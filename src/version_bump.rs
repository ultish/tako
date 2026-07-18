//! Smart per-project version bumps (major / minor / patch) and rewrite of
//! Gradle version declarations in `build.gradle(.kts)` / `gradle.properties`.

use std::fs;
use std::path::{Path, PathBuf};

use crate::scan::{
    parse_version_from_build_script, parse_version_from_gradle_properties, UNKNOWN_VERSION,
};

/// Which component to increment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BumpKind {
    Major,
    Minor,
    #[default]
    Patch,
}

impl BumpKind {
    pub fn label(self) -> &'static str {
        match self {
            BumpKind::Major => "major",
            BumpKind::Minor => "minor",
            BumpKind::Patch => "patch",
        }
    }

    pub fn next(self) -> Self {
        match self {
            BumpKind::Patch => BumpKind::Minor,
            BumpKind::Minor => BumpKind::Major,
            BumpKind::Major => BumpKind::Patch,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            BumpKind::Patch => BumpKind::Major,
            BumpKind::Minor => BumpKind::Patch,
            BumpKind::Major => BumpKind::Minor,
        }
    }
}

/// One dependent project and the proposed version change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BumpRow {
    pub project_index: usize,
    pub project_name: String,
    pub project_path: PathBuf,
    pub old_version: String,
    pub new_version: String,
    /// File that will be rewritten (relative display or absolute).
    pub file: PathBuf,
    pub error: Option<String>,
}

/// Confirm plan for bumping all graph dependents of a source project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BumpDependentsPlan {
    pub source_name: String,
    pub source_version: String,
    pub kind: BumpKind,
    pub rows: Vec<BumpRow>,
    pub cursor: usize,
}

impl BumpDependentsPlan {
    pub fn move_cursor(&mut self, delta: i32) {
        if self.rows.is_empty() {
            return;
        }
        let cur = self.cursor as i32;
        self.cursor = (cur + delta).clamp(0, (self.rows.len() as i32) - 1) as usize;
    }

    /// Recompute `new_version` for every row under a new bump kind.
    pub fn set_kind(&mut self, kind: BumpKind) {
        self.kind = kind;
        for row in &mut self.rows {
            match bump_semver(&row.old_version, kind) {
                Ok(v) => {
                    row.new_version = v;
                    row.error = None;
                }
                Err(e) => {
                    row.error = Some(e);
                }
            }
        }
    }

    pub fn ok_count(&self) -> usize {
        self.rows.iter().filter(|r| r.error.is_none()).count()
    }
}

/// Bump a version string by major / minor / patch.
///
/// Handles `1.2.3`, `1.2`, `1`, optional pre-release / build suffix after `-` or `+`
/// (suffix is preserved for patch when possible; reset on major/minor for
/// simple `1.2.3-SNAPSHOT` → `1.3.0-SNAPSHOT` / `2.0.0-SNAPSHOT`).
pub fn bump_semver(version: &str, kind: BumpKind) -> Result<String, String> {
    let version = version.trim();
    if version.is_empty() || version == UNKNOWN_VERSION || version == "—" {
        return Err("no version declared".into());
    }

    // Split core from suffix: 1.2.3-SNAPSHOT / 1.2.3+build
    let (core, suffix) = split_version_suffix(version);
    let parts = parse_numeric_parts(core)?;
    let (major, minor, patch) = match parts.as_slice() {
        [a] => (*a, 0u64, 0u64),
        [a, b] => (*a, *b, 0u64),
        [a, b, c, ..] => (*a, *b, *c),
        [] => return Err(format!("unparseable version: {version}")),
    };

    let (nm, nn, np) = match kind {
        BumpKind::Major => (major.saturating_add(1), 0, 0),
        BumpKind::Minor => (major, minor.saturating_add(1), 0),
        BumpKind::Patch => (major, minor, patch.saturating_add(1)),
    };

    // Preserve original component count style when possible (1.2 stays two-part
    // on minor; patch forces at least three components so the bump is visible).
    let core_out = match (parts.len(), kind) {
        (1, BumpKind::Major) => format!("{nm}"),
        (1, BumpKind::Minor) => format!("{nm}.{nn}"),
        (1, BumpKind::Patch) => format!("{nm}.{nn}.{np}"),
        (2, BumpKind::Major) => format!("{nm}.0"),
        (2, BumpKind::Minor) => format!("{nm}.{nn}"),
        (2, BumpKind::Patch) => format!("{nm}.{nn}.{np}"),
        _ => format!("{nm}.{nn}.{np}"),
    };

    Ok(format!("{core_out}{suffix}"))
}

fn split_version_suffix(version: &str) -> (&str, &str) {
    // Prefer first '-' or '+' that looks like a pre-release (not inside weird tokens).
    if let Some(i) = version.find(|c| c == '-' || c == '+') {
        // Don't treat lone leading v / weird cases — require digits before.
        if i > 0 && version[..i].chars().any(|c| c.is_ascii_digit()) {
            return (&version[..i], &version[i..]);
        }
    }
    (version, "")
}

fn parse_numeric_parts(core: &str) -> Result<Vec<u64>, String> {
    let core = core.trim().trim_start_matches('v').trim_start_matches('V');
    if core.is_empty() {
        return Err("empty version core".into());
    }
    let mut parts = Vec::new();
    for seg in core.split('.') {
        // Allow pure numeric segments only for the bump core.
        let seg = seg.trim();
        if seg.is_empty() {
            return Err(format!("bad segment in {core}"));
        }
        // Strip non-digit prefix garbage from segment if any (rare).
        let digits: String = seg.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            return Err(format!("non-numeric version segment '{seg}' in {core}"));
        }
        let n: u64 = digits
            .parse()
            .map_err(|_| format!("version segment overflow: {seg}"))?;
        parts.push(n);
        if parts.len() >= 3 {
            break;
        }
    }
    if parts.is_empty() {
        return Err(format!("unparseable version core: {core}"));
    }
    Ok(parts)
}

/// Locate the file that holds the project's version declaration.
pub fn find_version_file(project_dir: &Path) -> Option<PathBuf> {
    for name in ["build.gradle.kts", "build.gradle"] {
        let p = project_dir.join(name);
        if let Ok(content) = fs::read_to_string(&p) {
            if parse_version_from_build_script(&content).is_some() {
                return Some(p);
            }
        }
    }
    let props = project_dir.join("gradle.properties");
    if let Ok(content) = fs::read_to_string(&props) {
        if parse_version_from_gradle_properties(&content).is_some() {
            return Some(props);
        }
    }
    None
}

/// Rewrite the first bare `version` assignment in `file` from `old` to `new`.
///
/// Preserves quote style and surrounding whitespace on that line.
pub fn rewrite_version_in_file(file: &Path, old: &str, new: &str) -> Result<(), String> {
    let content =
        fs::read_to_string(file).map_err(|e| format!("read {}: {e}", file.display()))?;
    let updated = rewrite_version_content(&content, old, new, file)?;
    fs::write(file, updated).map_err(|e| format!("write {}: {e}", file.display()))?;
    Ok(())
}

fn rewrite_version_content(
    content: &str,
    old: &str,
    new: &str,
    file: &Path,
) -> Result<String, String> {
    let is_props = file
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n == "gradle.properties");

    let mut out = String::new();
    let mut replaced = false;
    for (i, line) in content.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if !replaced {
            if let Some(new_line) = try_replace_version_line(line, old, new, is_props) {
                out.push_str(&new_line);
                replaced = true;
                continue;
            }
        }
        out.push_str(line);
    }
    if content.ends_with('\n') {
        out.push('\n');
    }

    if !replaced {
        return Err(format!(
            "version assignment for '{old}' not found in {}",
            file.display()
        ));
    }
    Ok(out)
}

fn try_replace_version_line(line: &str, old: &str, new: &str, is_props: bool) -> Option<String> {
    let trimmed = if is_props {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') || t.starts_with('!') {
            return None;
        }
        t
    } else {
        // strip // comments for matching only — keep full line for rewrite
        let t = line;
        let code = t.split("//").next().unwrap_or(t).trim();
        if code.is_empty() {
            return None;
        }
        code
    };

    // Detect that this line declares the project version equal to `old`.
    let parsed = if is_props {
        parse_version_from_gradle_properties(line)
    } else {
        // Use full-line parse helper by checking assignment on the code portion
        crate::scan::parse_version_from_build_script(trimmed)
            .or_else(|| crate::scan::parse_version_from_build_script(line))
    };
    if parsed.as_deref() != Some(old) {
        return None;
    }

    // Replace first occurrence of the old version token with quoting preserved.
    if let Some(idx) = line.find(old) {
        let mut s = String::new();
        s.push_str(&line[..idx]);
        s.push_str(new);
        s.push_str(&line[idx + old.len()..]);
        return Some(s);
    }
    None
}

/// Successful rewrite (for git commit grouping).
#[derive(Debug, Clone)]
pub struct AppliedBump {
    pub project_name: String,
    pub project_path: PathBuf,
    pub file: PathBuf,
    pub old_version: String,
    pub new_version: String,
}

/// Apply all successful rows in the plan (skip rows with errors).
/// Returns (applied rows, error messages).
pub fn apply_bump_plan(plan: &BumpDependentsPlan) -> (Vec<AppliedBump>, Vec<String>) {
    let mut applied = Vec::new();
    let mut errs = Vec::new();
    for row in &plan.rows {
        if let Some(e) = &row.error {
            errs.push(format!("{}: {e}", row.project_name));
            continue;
        }
        match rewrite_version_in_file(&row.file, &row.old_version, &row.new_version) {
            Ok(()) => applied.push(AppliedBump {
                project_name: row.project_name.clone(),
                project_path: row.project_path.clone(),
                file: row.file.clone(),
                old_version: row.old_version.clone(),
                new_version: row.new_version.clone(),
            }),
            Err(e) => errs.push(format!("{}: {e}", row.project_name)),
        }
    }
    (applied, errs)
}

/// Build commit message(s) and run `git add` + `git commit` per git root.
///
/// Subject: `Bump version: {name}` (or multi-line body if several rows).
/// Body lists each project `old → new`.
///
/// Returns (commits_created, error lines).
pub fn commit_applied_bumps(
    source_name: &str,
    applied: &[AppliedBump],
) -> (usize, Vec<String>) {
    use std::collections::BTreeMap;

    use crate::git::{self, find_git_root};

    if applied.is_empty() {
        return (0, vec![]);
    }

    // git_root → files + body lines
    let mut by_root: BTreeMap<PathBuf, (Vec<PathBuf>, Vec<String>)> = BTreeMap::new();
    let mut no_git: Vec<String> = Vec::new();

    for a in applied {
        let Some(root) = find_git_root(&a.project_path).or_else(|| find_git_root(&a.file)) else {
            no_git.push(format!("{}: no git root (left uncommitted)", a.project_name));
            continue;
        };
        let entry = by_root.entry(root).or_default();
        entry.0.push(a.file.clone());
        entry.1.push(format!(
            "- {}: {} → {}",
            a.project_name, a.old_version, a.new_version
        ));
    }

    let subject = if applied.len() == 1 {
        format!(
            "Bump version: {} ({} → {})",
            applied[0].project_name, applied[0].old_version, applied[0].new_version
        )
    } else {
        format!("Bump versions ({source_name})")
    };
    let mut commits = 0usize;
    let mut errs = no_git;

    for (root, (files, lines)) in by_root {
        // Dedupe files (same monorepo file shouldn't appear twice, but be safe).
        let mut uniq = files;
        uniq.sort();
        uniq.dedup();
        let body = lines.join("\n");
        let message = format!("{subject}\n\n{body}\n");
        match git::commit_paths(&root, &uniq, &message) {
            Ok(true) => commits += 1,
            Ok(false) => errs.push(format!(
                "{}: nothing to commit (already clean?)",
                root.display()
            )),
            Err(e) => errs.push(format!("{}: git commit failed: {e}", root.display())),
        }
    }

    (commits, errs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn bump_patch_minor_major() {
        assert_eq!(bump_semver("1.2.3", BumpKind::Patch).unwrap(), "1.2.4");
        assert_eq!(bump_semver("1.2.3", BumpKind::Minor).unwrap(), "1.3.0");
        assert_eq!(bump_semver("1.2.3", BumpKind::Major).unwrap(), "2.0.0");
    }

    #[test]
    fn bump_preserves_snapshot_suffix() {
        assert_eq!(
            bump_semver("1.2.3-SNAPSHOT", BumpKind::Patch).unwrap(),
            "1.2.4-SNAPSHOT"
        );
        assert_eq!(
            bump_semver("0.1.0-SNAPSHOT", BumpKind::Minor).unwrap(),
            "0.2.0-SNAPSHOT"
        );
    }

    #[test]
    fn bump_two_part_version() {
        assert_eq!(bump_semver("1.2", BumpKind::Minor).unwrap(), "1.3");
        assert_eq!(bump_semver("1.2", BumpKind::Patch).unwrap(), "1.2.1");
    }

    #[test]
    fn rewrite_kts_version_line() {
        let src = r#"
plugins {
    kotlin("jvm")
}
group = "com.example"
version = "0.1.0"

dependencies {
}
"#;
        let out = rewrite_version_content(src, "0.1.0", "0.1.1", Path::new("build.gradle.kts"))
            .unwrap();
        assert!(out.contains("version = \"0.1.1\""));
        assert!(!out.contains("version = \"0.1.0\""));
        assert!(out.contains("group = \"com.example\""));
    }

    #[test]
    fn rewrite_does_not_touch_plugin_versions() {
        let src = r#"
plugins {
    kotlin("jvm") version "2.0.0"
}
version = "1.0.0"
"#;
        let out =
            rewrite_version_content(src, "1.0.0", "1.0.1", Path::new("build.gradle.kts")).unwrap();
        assert!(out.contains("kotlin(\"jvm\") version \"2.0.0\""));
        assert!(out.contains("version = \"1.0.1\""));
    }

    #[test]
    fn rewrite_properties() {
        let src = "foo=bar\nversion=1.4.2\nbar=baz\n";
        let out =
            rewrite_version_content(src, "1.4.2", "1.4.3", Path::new("gradle.properties")).unwrap();
        assert!(out.contains("version=1.4.3"));
        assert!(out.contains("foo=bar"));
    }

    #[test]
    fn find_and_rewrite_temp_project() {
        let dir = std::env::temp_dir().join(format!("tako-bump-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let build = dir.join("build.gradle.kts");
        let mut f = fs::File::create(&build).unwrap();
        writeln!(f, "version = \"2.0.0\"").unwrap();
        drop(f);

        let file = find_version_file(&dir).expect("find");
        rewrite_version_in_file(&file, "2.0.0", "2.0.1").unwrap();
        let body = fs::read_to_string(&file).unwrap();
        assert!(body.contains("2.0.1"));
        let _ = fs::remove_dir_all(&dir);
    }
}
