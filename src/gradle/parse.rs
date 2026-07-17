//! Light static extraction from `build.gradle.kts` / `build.gradle`.
//!
//! No Gradle daemon — string/regex-style patterns for:
//! - `group = "…"`
//! - `archivesName` / `base.archivesName`
//! - `implementation("g:n:v")` / `api("…")` / `compileOnly` / `runtimeOnly`
//! - `implementation(libs.xxx.yyy)`
//! - `implementation(project(":module"))`
//!
//! Produces coordinates use `group` + artifact name + project version.

use std::fs;
use std::path::Path;

use super::catalog::{CatalogRegistry, VersionCatalog};
use super::model::{Coordinate, DepReq, Produces, VersionSpec};

/// Configurations we treat as dependency edges.
const DEP_CONFIGS: &[&str] = &[
    "implementation",
    "api",
    "compileOnly",
    "runtimeOnly",
    "testImplementation",
    "testApi",
    "compileOnlyApi",
    "runtimeOnlyApi",
];

/// Parsed Gradle identity + edges for one project directory.
#[derive(Debug, Clone, Default)]
pub struct GradleModel {
    pub group: Option<String>,
    pub artifact_name: Option<String>,
    pub depends: Vec<DepReq>,
    pub produces: Vec<Produces>,
}

/// Extract depends/produces for a project directory.
///
/// `version` is the already-resolved project version (from scan).
/// `gradle_root` is the nearest settings root (catalog search bound).
/// `registry` is optional workspace-wide catalogs (published version catalogs
/// living in other repos under the same scan roots).
pub fn extract_for_project(
    project_dir: &Path,
    version: &str,
    gradle_root: Option<&Path>,
    registry: Option<&CatalogRegistry>,
) -> GradleModel {
    // Concatenate build scripts so registry can pick a catalog that resolves
    // the `libs.*` accessors actually used.
    let mut script_blob = String::new();
    for name in ["build.gradle.kts", "build.gradle"] {
        let path = project_dir.join(name);
        if let Ok(content) = fs::read_to_string(&path) {
            script_blob.push_str(&content);
            script_blob.push('\n');
        }
    }

    let catalog = match registry {
        Some(reg) => reg.resolve_for_project(project_dir, gradle_root, &script_blob),
        None => super::catalog::load_catalog_for(project_dir, gradle_root),
    };
    let mut model = GradleModel::default();

    for name in ["build.gradle.kts", "build.gradle"] {
        let path = project_dir.join(name);
        if let Ok(content) = fs::read_to_string(&path) {
            let partial = parse_build_script(&content, &catalog);
            merge_model(&mut model, partial);
        }
    }

    // Artifact name fallback: directory name.
    if model.artifact_name.is_none() {
        if let Some(n) = project_dir.file_name() {
            model.artifact_name = Some(n.to_string_lossy().into_owned());
        }
    }

    // Also try settings.gradle.kts rootProject.name for monorepo roots.
    if model.artifact_name.is_none() {
        if let Some(root) = gradle_root {
            for name in ["settings.gradle.kts", "settings.gradle"] {
                if let Ok(content) = fs::read_to_string(root.join(name)) {
                    if let Some(n) = parse_root_project_name(&content) {
                        model.artifact_name = Some(n);
                        break;
                    }
                }
            }
        }
    }

    // Produce coordinate when we have a group (or invent from folder heuristics).
    if let (Some(group), Some(name)) = (model.group.clone(), model.artifact_name.clone()) {
        let ver = if version == "—" {
            String::new()
        } else {
            version.to_string()
        };
        model.produces.push(Produces::new(
            Coordinate::new(group, name),
            ver,
        ));
    }

    model
}

fn merge_model(into: &mut GradleModel, from: GradleModel) {
    if into.group.is_none() {
        into.group = from.group;
    }
    if into.artifact_name.is_none() {
        into.artifact_name = from.artifact_name;
    }
    into.depends.extend(from.depends);
    // produces assembled later
}

/// Parse a single build script body.
pub fn parse_build_script(content: &str, catalog: &VersionCatalog) -> GradleModel {
    let mut model = GradleModel::default();

    for line in content.lines() {
        let trimmed = strip_line_comment(line).trim();
        if trimmed.is_empty() {
            continue;
        }

        if model.group.is_none() {
            if let Some(g) = parse_assignment(trimmed, "group") {
                model.group = Some(g);
            }
        }
        if model.artifact_name.is_none() {
            if let Some(n) = parse_archives_name(trimmed) {
                model.artifact_name = Some(n);
            }
        }

        if let Some(dep) = parse_dependency_line(trimmed, catalog) {
            model.depends.push(dep);
        }
    }

    // Multi-line-ish: also scan whole content for project()/libs./GAV with configs
    // when simple line walk missed nested blocks — the line walk covers normal
    // single-line declarations which is the common case.

    model
}

fn parse_archives_name(trimmed: &str) -> Option<String> {
    // base.archivesName.set("foo") / archivesName.set("foo")
    for key in ["base.archivesName", "archivesName", "archivesBaseName"] {
        if let Some(rest) = trimmed.strip_prefix(key) {
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix(".set") {
                let rest = rest.trim_start().trim_start_matches('(').trim_start();
                return strip_quotes_value(rest);
            }
            if rest.starts_with('=') {
                return parse_assignment(trimmed, key);
            }
        }
    }
    None
}

fn parse_assignment(trimmed: &str, key: &str) -> Option<String> {
    let rest = trimmed.strip_prefix(key)?;
    let rest = rest.trim_start();
    if !rest.starts_with('=') {
        return None;
    }
    let rest = rest[1..].trim_start();
    strip_quotes_value(rest)
}

fn strip_quotes_value(s: &str) -> Option<String> {
    let s = s.trim().trim_end_matches(',').trim();
    if s.is_empty() {
        return None;
    }
    if (s.starts_with('"') || s.starts_with('\'')) && s.len() >= 2 {
        let quote = s.as_bytes()[0];
        if let Some(end) = s[1..].find(quote as char) {
            let inner = &s[1..1 + end];
            if !inner.is_empty() {
                return Some(inner.to_string());
            }
        }
    }
    None
}

fn strip_line_comment(line: &str) -> &str {
    if let Some(idx) = line.find("//") {
        let before = &line[..idx];
        let quotes = before.matches('"').count() + before.matches('\'').count();
        if quotes % 2 == 0 {
            return before;
        }
    }
    line
}

/// Parse one dependency declaration line.
pub fn parse_dependency_line(trimmed: &str, catalog: &VersionCatalog) -> Option<DepReq> {
    let (config, rest) = match split_config_call(trimmed) {
        Some(x) => x,
        None => return None,
    };

    let rest = rest.trim();
    // project(":foo") / project(path = ":foo")
    if let Some(path) = parse_project_dep(rest) {
        let raw = format!("{config}(project(\":{path}\"))");
        return Some(DepReq::project_dep(path, config, raw));
    }

    // libs.xxx.yyy  or  libs.xxx.yyy.get()
    if rest.starts_with("libs.") {
        let accessor = rest
            .trim_end_matches(')')
            .trim_end_matches('(')
            .split_whitespace()
            .next()
            .unwrap_or(rest);
        let accessor = accessor.trim_end_matches(',').trim();
        // Drop trailing method calls for matching
        let accessor_base = accessor.split('(').next().unwrap_or(accessor);
        if let Some(dep) = catalog.resolve_libs_accessor(accessor_base, config, accessor_base) {
            return Some(dep);
        }
        // Unresolved catalog ref — still record raw
        return Some(DepReq {
            coordinate: None,
            version: VersionSpec::Unresolved,
            configuration: config.to_string(),
            raw: accessor_base.to_string(),
        });
    }

    // Direct Maven GAV string — resolved at build time from Nexus/Maven Central
    // (or whatever repositories the project declares). We only parse the
    // declaration for the graph; we do not download artifacts.
    //   implementation("com.example:util-core:1.0.0")
    //   implementation("util-libs:util-core:[1.0,2.0)")
    if let Some(gav) = strip_quotes_value(rest) {
        if gav.contains(':') {
            return parse_gav_dep(&gav, config);
        }
    }

    // Named args: implementation(group = "g", name = "n", version = "v")
    if let Some(dep) = parse_named_dependency_args(rest, config) {
        return Some(dep);
    }

    // platform("…") / enforcedPlatform — skip expanding BOM in v1
    None
}

/// `group = "…", name = "…", version = "…"` inside a configuration call.
fn parse_named_dependency_args(rest: &str, config: &str) -> Option<DepReq> {
    if !rest.contains("group") || !rest.contains("name") {
        return None;
    }
    let group = extract_named_string_arg(rest, "group")?;
    let name = extract_named_string_arg(rest, "name")?;
    let version = extract_named_string_arg(rest, "version").unwrap_or_default();
    let raw = if version.is_empty() {
        format!("{group}:{name}")
    } else {
        format!("{group}:{name}:{version}")
    };
    Some(DepReq::external(
        Coordinate::new(group, name),
        if version.is_empty() {
            VersionSpec::Unresolved
        } else {
            VersionSpec::from_version_string(&version)
        },
        config,
        raw,
    ))
}

fn extract_named_string_arg(s: &str, key: &str) -> Option<String> {
    // group = "x" / group="x" / group = 'x'
    let key_pat = format!("{key}");
    let mut search = s;
    while let Some(idx) = search.find(&key_pat) {
        let after_key = &search[idx + key.len()..];
        let after_key = after_key.trim_start();
        // skip if this is part of a longer identifier
        if idx > 0 {
            let prev = search.as_bytes()[idx - 1];
            if prev.is_ascii_alphanumeric() || prev == b'_' {
                search = &search[idx + key.len()..];
                continue;
            }
        }
        if !after_key.starts_with('=') {
            search = &search[idx + key.len()..];
            continue;
        }
        let val = after_key[1..].trim_start();
        return strip_quotes_value(val);
    }
    None
}

fn split_config_call(trimmed: &str) -> Option<(&str, &str)> {
    for config in DEP_CONFIGS {
        // configuration("…") or configuration(libs…) or configuration(project(…
        let prefix = format!("{config}(");
        if let Some(rest) = trimmed.strip_prefix(&prefix) {
            let rest = rest.trim_end_matches(')').trim();
            return Some((*config, rest));
        }
        // configuration "…" (Groovy)
        let prefix_sp = format!("{config} ");
        if let Some(rest) = trimmed.strip_prefix(&prefix_sp) {
            return Some((*config, rest.trim()));
        }
    }
    None
}

fn parse_project_dep(rest: &str) -> Option<String> {
    let rest = rest.trim();
    let inner = if let Some(r) = rest.strip_prefix("project(") {
        r.trim_end_matches(')').trim()
    } else if rest.starts_with("project ") {
        rest.trim_start_matches("project").trim()
    } else {
        return None;
    };

    // path = ":foo" or just ":foo"
    let path_str = if let Some(r) = inner.strip_prefix("path") {
        let r = r.trim_start().trim_start_matches('=').trim_start();
        strip_quotes_value(r)?
    } else {
        strip_quotes_value(inner)?
    };

    Some(normalize_project_path(&path_str))
}

/// Strip leading `:` and normalize.
pub fn normalize_project_path(path: &str) -> String {
    path.trim().trim_start_matches(':').to_string()
}

fn parse_gav_dep(gav: &str, config: &str) -> Option<DepReq> {
    let parts: Vec<&str> = gav.splitn(3, ':').collect();
    if parts.len() < 2 {
        return None;
    }
    let group = parts[0].trim();
    let name = parts[1].trim();
    if group.is_empty() || name.is_empty() {
        return None;
    }
    let version = if parts.len() == 3 {
        VersionSpec::from_version_string(parts[2].trim())
    } else {
        VersionSpec::Unresolved
    };
    Some(DepReq::external(
        Coordinate::new(group, name),
        version,
        config,
        gav,
    ))
}

pub fn parse_root_project_name(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = strip_line_comment(line).trim();
        // rootProject.name = "foo"
        if let Some(rest) = trimmed.strip_prefix("rootProject.name") {
            let rest = rest.trim_start();
            if rest.starts_with('=') {
                return strip_quotes_value(rest[1..].trim_start());
            }
            if let Some(rest) = rest.strip_prefix(".set") {
                let rest = rest.trim_start().trim_start_matches('(').trim_start();
                return strip_quotes_value(rest);
            }
        }
    }
    None
}

/// Parse group from build script alone (shared helper for tests / scan).
pub fn parse_group(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = strip_line_comment(line).trim();
        if let Some(g) = parse_assignment(trimmed, "group") {
            return Some(g);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gradle::catalog::VersionCatalog;

    fn sample_catalog() -> VersionCatalog {
        VersionCatalog::parse(
            r#"
[versions]
common = "[1.0.0, 2.0.0)"
[libraries]
common-lib = { module = "com.example:common-lib", version.ref = "common" }
"#,
        )
        .unwrap()
    }

    #[test]
    fn parse_string_gav_implementation() {
        let cat = VersionCatalog::empty();
        let dep = parse_dependency_line(
            r#"implementation("com.example:common-lib:1.4.2")"#,
            &cat,
        )
        .unwrap();
        assert_eq!(dep.configuration, "implementation");
        assert_eq!(
            dep.coordinate.as_ref().map(|c| c.key()).as_deref(),
            Some("com.example:common-lib")
        );
        assert!(matches!(dep.version, VersionSpec::Exact(ref v) if v == "1.4.2"));
    }

    #[test]
    fn parse_api_range_string() {
        let cat = VersionCatalog::empty();
        let dep = parse_dependency_line(
            r#"api("com.example:common-lib:[1.0.0, 2.0.0)")"#,
            &cat,
        )
        .unwrap();
        assert!(matches!(dep.version, VersionSpec::Range(_)));
    }

    #[test]
    fn parse_project_dep() {
        let cat = VersionCatalog::empty();
        let dep = parse_dependency_line(r#"implementation(project(":shared"))"#, &cat).unwrap();
        assert_eq!(dep.project_path(), Some("shared"));
        assert!(dep.coordinate.is_none());
    }

    #[test]
    fn parse_libs_catalog_dep() {
        let cat = sample_catalog();
        let dep = parse_dependency_line("implementation(libs.common.lib)", &cat).unwrap();
        assert_eq!(
            dep.coordinate.as_ref().map(|c| c.key()).as_deref(),
            Some("com.example:common-lib")
        );
        assert!(matches!(dep.version, VersionSpec::Range(ref r) if r == "[1.0.0, 2.0.0)"));
    }

    #[test]
    fn parse_direct_gav_string_from_nexus_style() {
        let cat = VersionCatalog::empty();
        // Not a catalog accessor — plain Maven GAV (resolved from Nexus at build time).
        let dep = parse_dependency_line(
            r#"implementation("util-libs:util-core:[1.0.0,2.0.0)")"#,
            &cat,
        )
        .expect("GAV string");
        assert_eq!(
            dep.coordinate.as_ref().map(|c| c.key()).as_deref(),
            Some("util-libs:util-core")
        );
        assert!(matches!(dep.version, VersionSpec::Range(ref r) if r == "[1.0.0,2.0.0)"));

        let dep2 = parse_dependency_line(
            r#"api("com.tako.fixtures:cats:1.0.0")"#,
            &cat,
        )
        .expect("exact GAV");
        assert_eq!(
            dep2.coordinate.as_ref().map(|c| c.key()).as_deref(),
            Some("com.tako.fixtures:cats")
        );
        assert!(matches!(dep2.version, VersionSpec::Exact(ref v) if v == "1.0.0"));
    }

    #[test]
    fn parse_named_group_name_version_args() {
        let cat = VersionCatalog::empty();
        let dep = parse_dependency_line(
            r#"implementation(group = "com.acme", name = "foo", version = "2.1.0")"#,
            &cat,
        )
        .expect("named args");
        assert_eq!(
            dep.coordinate.as_ref().map(|c| c.key()).as_deref(),
            Some("com.acme:foo")
        );
        assert!(matches!(dep.version, VersionSpec::Exact(ref v) if v == "2.1.0"));
    }

    #[test]
    fn parse_group_and_produces_via_extract() {
        let dir = std::env::temp_dir().join(format!(
            "tako-parse-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(dir.join("gradle")).unwrap();
        fs::write(
            dir.join("gradle/libs.versions.toml"),
            r#"
[versions]
common = "[1.0.0, 2.0.0)"
[libraries]
common-lib = { module = "com.example:common-lib", version.ref = "common" }
"#,
        )
        .unwrap();
        fs::write(
            dir.join("build.gradle.kts"),
            r#"
plugins { `java-library` }
group = "com.example"
version = "1.4.2"
dependencies {
    api("com.acme:other:2.0.0")
    implementation(libs.common.lib)
    implementation(project(":core"))
}
"#,
        )
        .unwrap();

        let model = extract_for_project(&dir, "1.4.2", Some(&dir), None);
        assert_eq!(model.group.as_deref(), Some("com.example"));
        assert_eq!(model.produces.len(), 1);
        assert_eq!(
            model.produces[0].coordinate.key(),
            format!(
                "com.example:{}",
                dir.file_name().unwrap().to_string_lossy()
            )
        );
        assert_eq!(model.produces[0].version, "1.4.2");
        assert!(model.depends.len() >= 3);
        assert!(model
            .depends
            .iter()
            .any(|d| d.project_path() == Some("core")));
        assert!(model.depends.iter().any(|d| {
            d.coordinate
                .as_ref()
                .is_some_and(|c| c.key() == "com.example:common-lib")
        }));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_root_project_name_kts() {
        assert_eq!(
            parse_root_project_name(r#"rootProject.name = "platform""#).as_deref(),
            Some("platform")
        );
    }
}
