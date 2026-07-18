//! Best-effort Maven repo version probe via `curl` + maven-metadata.xml.
//!
//! Repo base URL resolution order (Gradle-first, since these are Gradle projects):
//! 1. Optional `[nexus].repository_url` override in tako config
//! 2. `maven { url = … }` / `maven("…")` in each project's
//!    `settings.gradle(.kts)` and `build.gradle(.kts)` (and gradle roots)
//! 3. Fallback: `~/.m2/settings.xml` mirrors/repositories
//! 4. Otherwise leave the Nexus column as `—`
//!
//! No separate “enable Nexus” flag — repos come from how Gradle is already set up.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::app::{ProjectKind, ProjectRow};
use crate::config::NexusConfig;
use crate::graph::DependencyGraph;

/// Result of probing one published artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NexusProbe {
    /// Project path (stable key into inventory).
    pub project_path: String,
    /// Short table cell: `ok` | `newer` | `miss` | `?` | `—`
    pub label: String,
    /// Longer detail for status bar / U plan.
    pub detail: String,
}

/// Batch result applied onto project rows.
#[derive(Debug, Clone, Default)]
pub struct NexusBatch {
    pub probes: Vec<NexusProbe>,
    pub error: Option<String>,
    /// Which repo base(s) were used (for status bar).
    pub repo_bases: Vec<String>,
}

/// True when this project is a publisher we should check on Maven/Nexus.
pub fn should_probe_nexus(project: &ProjectRow) -> bool {
    matches!(project.kind, ProjectKind::Library | ProjectKind::Avro)
        || (!project.has_skaffold
            && project.skaffold_path.is_none()
            && !matches!(project.kind, ProjectKind::Service))
}

/// Resolve repository base URLs: override → Gradle scripts on projects → m2 settings.
pub fn resolve_repository_bases(cfg: &NexusConfig, projects: &[ProjectRow]) -> Vec<String> {
    let mut out = Vec::new();
    let push = |out: &mut Vec<String>, u: &str| {
        let u = u.trim().trim_end_matches('/').to_string();
        if !u.is_empty() && !out.contains(&u) {
            out.push(u);
        }
    };

    let override_url = cfg.repository_url.trim();
    if !override_url.is_empty() {
        push(&mut out, override_url);
        return out;
    }

    for u in discover_repos_from_gradle_projects(projects) {
        push(&mut out, &u);
    }
    // Fallback only if Gradle scripts didn't declare anything useful.
    if out.is_empty() {
        for u in discover_repos_from_m2_settings() {
            push(&mut out, &u);
        }
    }
    out
}

/// Walk inventory projects' Gradle roots and parse repository URLs from scripts.
fn discover_repos_from_gradle_projects(projects: &[ProjectRow]) -> Vec<String> {
    let mut seen_dirs: Vec<PathBuf> = Vec::new();
    let mut urls = Vec::new();
    for p in projects {
        for dir in [p.gradle_project_dir(), p.path.as_path()] {
            if seen_dirs.iter().any(|d| d == dir) {
                continue;
            }
            seen_dirs.push(dir.to_path_buf());
            for name in [
                "settings.gradle.kts",
                "settings.gradle",
                "build.gradle.kts",
                "build.gradle",
            ] {
                let path = dir.join(name);
                let Ok(text) = fs::read_to_string(&path) else {
                    continue;
                };
                for u in extract_urls_from_gradle(&text) {
                    if !urls.contains(&u) {
                        urls.push(u);
                    }
                }
            }
            // Parent of multi-module root sometimes holds settings only.
            if let Some(parent) = dir.parent() {
                if seen_dirs.iter().any(|d| d == parent) {
                    continue;
                }
                for name in ["settings.gradle.kts", "settings.gradle"] {
                    let path = parent.join(name);
                    let Ok(text) = fs::read_to_string(&path) else {
                        continue;
                    };
                    seen_dirs.push(parent.to_path_buf());
                    for u in extract_urls_from_gradle(&text) {
                        if !urls.contains(&u) {
                            urls.push(u);
                        }
                    }
                }
            }
        }
    }
    urls
}

/// Extract Maven repo base URLs from Gradle Groovy/KTS script text.
fn extract_urls_from_gradle(text: &str) -> Vec<String> {
    let mut urls = Vec::new();
    // uri("https://…") / uri('https://…')
    for (prefix, quote) in [("uri(\"", '"'), ("uri('", '\''), ("URI.create(\"", '"')] {
        let mut rest = text;
        while let Some(i) = rest.find(prefix) {
            let after = &rest[i + prefix.len()..];
            if let Some(end) = after.find(quote) {
                let raw = after[..end].trim();
                if looks_like_repo_url(raw) {
                    let u = normalize_repo_url(raw);
                    if !urls.contains(&u) {
                        urls.push(u);
                    }
                }
                rest = &after[end + 1..];
            } else {
                break;
            }
        }
    }
    // maven("https://…") / maven('https://…')
    for (prefix, quote) in [("maven(\"", '"'), ("maven('", '\'')] {
        let mut rest = text;
        while let Some(i) = rest.find(prefix) {
            let after = &rest[i + prefix.len()..];
            if let Some(end) = after.find(quote) {
                let raw = after[..end].trim();
                if looks_like_repo_url(raw) {
                    let u = normalize_repo_url(raw);
                    if !urls.contains(&u) {
                        urls.push(u);
                    }
                }
                rest = &after[end + 1..];
            } else {
                break;
            }
        }
    }
    // Groovy: url "https://…" / url 'https://…'
    for (prefix, quote) in [("url \"", '"'), ("url '", '\'')] {
        let mut rest = text;
        while let Some(i) = rest.find(prefix) {
            let after = &rest[i + prefix.len()..];
            if let Some(end) = after.find(quote) {
                let raw = after[..end].trim();
                if looks_like_repo_url(raw) {
                    let u = normalize_repo_url(raw);
                    if !urls.contains(&u) {
                        urls.push(u);
                    }
                }
                rest = &after[end + 1..];
            } else {
                break;
            }
        }
    }
    // Built-ins
    if text.contains("mavenCentral()") {
        let u = "https://repo1.maven.org/maven2".to_string();
        if !urls.contains(&u) {
            urls.push(u);
        }
    }
    if text.contains("google()") {
        let u = "https://dl.google.com/dl/android/maven2".to_string();
        if !urls.contains(&u) {
            urls.push(u);
        }
    }
    urls
}

fn looks_like_repo_url(s: &str) -> bool {
    (s.starts_with("http://") || s.starts_with("https://")) && !s.contains("${")
}

fn normalize_repo_url(s: &str) -> String {
    s.trim().trim_end_matches('/').to_string()
}

/// Parse repository / mirror URLs from `~/.m2/settings.xml` (fallback).
fn discover_repos_from_m2_settings() -> Vec<String> {
    let mut paths = Vec::new();
    if let Some(home) = dirs::home_dir() {
        paths.push(home.join(".m2").join("settings.xml"));
    }
    if let Ok(m2) = std::env::var("M2_HOME") {
        paths.push(PathBuf::from(m2).join("conf").join("settings.xml"));
    }
    if let Ok(m2) = std::env::var("MAVEN_HOME") {
        paths.push(PathBuf::from(m2).join("conf").join("settings.xml"));
    }

    let mut urls = Vec::new();
    for path in paths {
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        urls.extend(extract_urls_from_settings_xml(&text));
        if !urls.is_empty() {
            break;
        }
    }
    urls
}

#[allow(dead_code)]
fn _path_exists_for_tests(p: &Path) -> bool {
    p.exists()
}

/// Pull `http(s)://…` from `<url>…</url>` tags (mirrors, repositories, pluginRepos).
fn extract_urls_from_settings_xml(xml: &str) -> Vec<String> {
    let mut urls = Vec::new();
    let mut rest = xml;
    while let Some(start_rel) = rest.find("<url>") {
        let after = &rest[start_rel + 5..];
        let Some(end_rel) = after.find("</url>") else {
            break;
        };
        let raw = after[..end_rel].trim();
        // Skip property placeholders we can't resolve: ${env.FOO}
        if raw.starts_with("http://") || raw.starts_with("https://") {
            let u = raw.trim_end_matches('/').to_string();
            if !urls.contains(&u) {
                urls.push(u);
            }
        }
        rest = &after[end_rel + 6..];
    }
    urls
}

/// Probe latest published versions for library/avro projects that produce coordinates.
pub fn probe_nexus_versions(
    projects: &[ProjectRow],
    graph: &DependencyGraph,
    cfg: &NexusConfig,
) -> NexusBatch {
    let bases = resolve_repository_bases(cfg, projects);
    if bases.is_empty() {
        return NexusBatch {
            probes: projects
                .iter()
                .filter(|p| should_probe_nexus(p))
                .map(|p| NexusProbe {
                    project_path: p.path.display().to_string(),
                    label: "—".into(),
                    detail: "no Maven repo found (Gradle settings/build scripts or ~/.m2)"
                        .into(),
                })
                .collect(),
            error: None,
            repo_bases: vec![],
        };
    }

    let mut probes = Vec::new();
    let mut errors = 0usize;

    for (idx, project) in projects.iter().enumerate() {
        if !should_probe_nexus(project) {
            continue;
        }
        let id = graph
            .id_at(idx)
            .unwrap_or(project.name.as_str())
            .to_string();
        let produces = graph.produces_of_id(&id);
        if produces.is_empty() {
            probes.push(NexusProbe {
                project_path: project.path.display().to_string(),
                label: "—".into(),
                detail: "no produces coordinates".into(),
            });
            continue;
        }

        let coord = &produces[0].coordinate;
        let local = if produces[0].version.is_empty() || produces[0].version == "—" {
            project.version.as_str()
        } else {
            produces[0].version.as_str()
        };

        // Try each discovered repo until one returns metadata.
        let mut last_err = String::new();
        let mut found = None;
        for base in &bases {
            let url = metadata_url(base, &coord.group, &coord.name);
            match fetch_maven_latest(&url) {
                Ok(remote) => {
                    found = Some((remote, base.clone()));
                    break;
                }
                Err(err) => last_err = err,
            }
        }

        match found {
            Some((remote, base)) => {
                let (label, detail) = compare_versions(local, &remote);
                probes.push(NexusProbe {
                    project_path: project.path.display().to_string(),
                    label,
                    detail: format!("{detail} · {base}"),
                });
            }
            None => {
                errors += 1;
                probes.push(NexusProbe {
                    project_path: project.path.display().to_string(),
                    label: if last_err.contains("404") || last_err.contains("Not Found") {
                        "miss".into()
                    } else {
                        "?".into()
                    },
                    detail: last_err,
                });
            }
        }
    }

    NexusBatch {
        error: if errors > 0 && probes.iter().all(|p| p.label == "?" || p.label == "miss") {
            Some(format!(
                "{errors} Maven probe(s) failed (repos: {})",
                bases.join(", ")
            ))
        } else {
            None
        },
        probes,
        repo_bases: bases,
    }
}

/// Apply nexus labels onto matching project rows.
pub fn apply_nexus_batch(projects: &mut [ProjectRow], batch: &NexusBatch) {
    for p in projects.iter_mut() {
        if !should_probe_nexus(p) {
            p.nexus = "—".into();
            continue;
        }
        let key = p.path.display().to_string();
        if let Some(probe) = batch.probes.iter().find(|x| x.project_path == key) {
            p.nexus = probe.label.clone();
        } else if p.nexus.is_empty() {
            p.nexus = "—".into();
        }
    }
}

fn metadata_url(repo_base: &str, group: &str, artifact: &str) -> String {
    let group_path = group.replace('.', "/");
    format!(
        "{}/{group_path}/{artifact}/maven-metadata.xml",
        repo_base.trim_end_matches('/')
    )
}

fn fetch_maven_latest(url: &str) -> Result<String, String> {
    let output = Command::new("curl")
        .args([
            "-fsSL",
            "--max-time",
            "10",
            "-H",
            "Accept: application/xml,text/xml,*/*",
            url,
        ])
        .output()
        .map_err(|e| format!("curl: {e}"))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        let code = output.status.code().unwrap_or(-1);
        return Err(if err.trim().is_empty() {
            format!("HTTP {code} for {url}")
        } else {
            err.trim().to_string()
        });
    }
    let body = String::from_utf8_lossy(&output.stdout);
    parse_maven_metadata_latest(&body).ok_or_else(|| "could not parse maven-metadata.xml".into())
}

/// Prefer `<latest>`, then `<release>`, then last `<version>` in the doc.
fn parse_maven_metadata_latest(xml: &str) -> Option<String> {
    if let Some(v) = xml_tag(xml, "latest") {
        if !v.is_empty() {
            return Some(v);
        }
    }
    if let Some(v) = xml_tag(xml, "release") {
        if !v.is_empty() {
            return Some(v);
        }
    }
    let mut last = None;
    let mut rest = xml;
    while let Some(v) = xml_tag(rest, "version") {
        last = Some(v.clone());
        if let Some(pos) = rest.find("</version>") {
            rest = &rest[pos + 10..];
        } else {
            break;
        }
    }
    last
}

fn xml_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    let v = xml[start..end].trim();
    if v.is_empty() {
        None
    } else {
        Some(v.to_string())
    }
}

fn compare_versions(local: &str, remote: &str) -> (String, String) {
    let local = local.trim();
    let remote = remote.trim();
    if local.is_empty() || local == "—" {
        return (
            "newer".into(),
            format!("repo {remote} (no local version)"),
        );
    }
    if local == remote {
        ("ok".into(), format!("repo {remote} matches local"))
    } else {
        (
            "newer".into(),
            format!("repo {remote} ≠ local {local}"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_latest_and_release() {
        let xml = r#"
        <metadata>
          <versioning>
            <latest>1.2.3-SNAPSHOT</latest>
            <release>1.2.2</release>
            <versions>
              <version>1.2.2</version>
              <version>1.2.3-SNAPSHOT</version>
            </versions>
          </versioning>
        </metadata>
        "#;
        assert_eq!(
            parse_maven_metadata_latest(xml).as_deref(),
            Some("1.2.3-SNAPSHOT")
        );
    }

    #[test]
    fn extract_urls_from_settings() {
        let xml = r#"
        <settings>
          <mirrors>
            <mirror>
              <url>https://nexus.example.com/repository/maven-public/</url>
            </mirror>
          </mirrors>
          <profiles>
            <profile>
              <repositories>
                <repository>
                  <url>https://repo1.maven.org/maven2</url>
                </repository>
              </repositories>
            </profile>
          </profiles>
        </settings>
        "#;
        let urls = extract_urls_from_settings_xml(xml);
        assert!(urls.iter().any(|u| u.contains("nexus.example.com")));
        assert!(urls.iter().any(|u| u.contains("repo1.maven.org")));
    }

    #[test]
    fn extract_urls_from_gradle_kts() {
        let kts = r#"
        dependencyResolutionManagement {
            repositories {
                maven {
                    url = uri("https://nexus.corp.example/repository/maven-public/")
                }
                mavenCentral()
            }
        }
        "#;
        let urls = extract_urls_from_gradle(kts);
        assert!(
            urls.iter().any(|u| u.contains("nexus.corp.example")),
            "{urls:?}"
        );
        assert!(urls.iter().any(|u| u.contains("repo1.maven.org")), "{urls:?}");
    }

    #[test]
    fn extract_urls_from_groovy_maven() {
        let groovy = r#"
        repositories {
            maven { url 'https://pkgs.dev.azure.com/org/feed/_packaging/feed/maven/v1' }
            maven("https://repo.spring.io/milestone")
        }
        "#;
        let urls = extract_urls_from_gradle(groovy);
        assert!(urls.iter().any(|u| u.contains("azure.com")), "{urls:?}");
        assert!(urls.iter().any(|u| u.contains("repo.spring.io")), "{urls:?}");
    }

    #[test]
    fn compare_ok_and_newer() {
        let (l, _) = compare_versions("1.0.0", "1.0.0");
        assert_eq!(l, "ok");
        let (l, d) = compare_versions("1.0.0", "1.0.1");
        assert_eq!(l, "newer");
        assert!(d.contains("1.0.1"));
    }
}
