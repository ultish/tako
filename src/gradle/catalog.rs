//! Parse Gradle version catalogs (`libs.versions.toml`).
//!
//! Supports the common subset:
//! ```toml
//! [versions]
//! common = "1.4.2"
//! commonRange = "[1.0.0, 2.0.0)"
//!
//! [libraries]
//! common-lib = { module = "com.example:common-lib", version.ref = "common" }
//! foo = { group = "com.acme", name = "foo", version = "2.0.0" }
//! ```
//!
//! Alias `common-lib` is referenced from Kotlin DSL as `libs.common.lib`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::model::{Coordinate, DepReq, VersionSpec};

/// Resolved version catalog for one Gradle tree.
#[derive(Debug, Clone, Default)]
pub struct VersionCatalog {
    /// version alias → version string (may be a range).
    pub versions: HashMap<String, String>,
    /// library alias (hyphen form, e.g. `common-lib`) → entry.
    pub libraries: HashMap<String, CatalogLibrary>,
    /// Absolute path the catalog was loaded from, when known.
    pub path: Option<PathBuf>,
}

/// One `[libraries]` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogLibrary {
    pub group: String,
    pub name: String,
    /// Raw version string or empty when only version.ref is present (resolved at load).
    pub version: String,
    /// Original version.ref alias, if any.
    pub version_ref: Option<String>,
}

impl CatalogLibrary {
    pub fn coordinate(&self) -> Coordinate {
        Coordinate::new(&self.group, &self.name)
    }

    pub fn version_spec(&self) -> VersionSpec {
        VersionSpec::from_version_string(&self.version)
    }
}

/// TOML shape for `libs.versions.toml`.
#[derive(Debug, Deserialize, Default)]
struct CatalogToml {
    #[serde(default)]
    versions: HashMap<String, toml::Value>,
    #[serde(default)]
    libraries: HashMap<String, LibraryToml>,
}

#[derive(Debug, Deserialize, Default)]
struct LibraryToml {
    /// `module = "group:name"`
    module: Option<String>,
    group: Option<String>,
    name: Option<String>,
    /// Inline version string, or table with `ref`.
    #[serde(default)]
    version: Option<toml::Value>,
}

impl VersionCatalog {
    pub fn empty() -> Self {
        Self::default()
    }

    /// Parse catalog TOML text.
    pub fn parse(content: &str) -> Result<Self, String> {
        let raw: CatalogToml =
            toml::from_str(content).map_err(|e| format!("catalog TOML: {e}"))?;

        let mut versions = HashMap::new();
        for (k, v) in raw.versions {
            if let Some(s) = value_as_string(&v) {
                versions.insert(k, s);
            } else if let Some(table) = v.as_table() {
                // version = { strictly = "1.0" } / prefer / require
                for key in ["strictly", "require", "prefer", "strict"] {
                    if let Some(s) = table.get(key).and_then(|x| value_as_string(x)) {
                        versions.insert(k.clone(), s);
                        break;
                    }
                }
            }
        }

        let mut libraries = HashMap::new();
        for (alias, lib) in raw.libraries {
            let (group, name) = if let Some(module) = &lib.module {
                let c = Coordinate::parse(module)
                    .ok_or_else(|| format!("bad module for library '{alias}': {module}"))?;
                (c.group, c.name)
            } else {
                let group = lib
                    .group
                    .clone()
                    .ok_or_else(|| format!("library '{alias}' missing group/module"))?;
                let name = lib
                    .name
                    .clone()
                    .ok_or_else(|| format!("library '{alias}' missing name/module"))?;
                (group, name)
            };

            let (version, version_ref) = resolve_library_version(&lib.version, &versions);
            libraries.insert(
                alias,
                CatalogLibrary {
                    group,
                    name,
                    version,
                    version_ref,
                },
            );
        }

        Ok(Self {
            versions,
            libraries,
            path: None,
        })
    }

    pub fn load_file(path: &Path) -> Result<Self, String> {
        let content =
            fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        let mut cat = Self::parse(&content)?;
        cat.path = Some(path.to_path_buf());
        Ok(cat)
    }

    /// Look up a library by TOML alias (`common-lib`) or dotted form (`common.lib`).
    pub fn library(&self, alias: &str) -> Option<&CatalogLibrary> {
        if let Some(lib) = self.libraries.get(alias) {
            return Some(lib);
        }
        // libs.common.lib → common-lib
        let hyphen = alias.replace('.', "-");
        self.libraries.get(&hyphen)
    }

    /// Resolve `libs.xxx.yyy` accessor path to a [`DepReq`].
    pub fn resolve_libs_accessor(
        &self,
        accessor: &str,
        configuration: &str,
        raw: &str,
    ) -> Option<DepReq> {
        let alias = libs_accessor_to_alias(accessor)?;
        let lib = self.library(&alias)?;
        Some(DepReq::external(
            lib.coordinate(),
            lib.version_spec(),
            configuration,
            raw,
        ))
    }

    /// Version string for a version alias, if present.
    pub fn version_alias(&self, alias: &str) -> Option<&str> {
        self.versions.get(alias).map(String::as_str)
    }
}

fn value_as_string(v: &toml::Value) -> Option<String> {
    match v {
        toml::Value::String(s) => Some(s.clone()),
        toml::Value::Integer(i) => Some(i.to_string()),
        toml::Value::Float(f) => Some(f.to_string()),
        _ => None,
    }
}

fn resolve_library_version(
    version: &Option<toml::Value>,
    versions: &HashMap<String, String>,
) -> (String, Option<String>) {
    let Some(v) = version else {
        return (String::new(), None);
    };
    if let Some(s) = value_as_string(v) {
        return (s, None);
    }
    if let Some(table) = v.as_table() {
        if let Some(r) = table.get("ref").and_then(|x| value_as_string(x)) {
            let resolved = versions.get(&r).cloned().unwrap_or_default();
            return (resolved, Some(r));
        }
        for key in ["strictly", "require", "prefer", "strict"] {
            if let Some(s) = table.get(key).and_then(|x| value_as_string(x)) {
                return (s, None);
            }
        }
    }
    (String::new(), None)
}

/// Convert `libs.common.lib` / `libs.versions.common.get()`-style path to catalog alias.
///
/// Only handles the library accessor form `libs.a.b.c` → `a-b-c`.
pub fn libs_accessor_to_alias(accessor: &str) -> Option<String> {
    let s = accessor.trim();
    let rest = s.strip_prefix("libs.")?;
    // Skip version accessors — those are not libraries.
    if rest.starts_with("versions.") {
        return None;
    }
    // Drop trailing `.get()` / `.asProvider()` etc.
    let rest = rest.split('(').next().unwrap_or(rest).trim_end_matches('.');
    if rest.is_empty() {
        return None;
    }
    Some(rest.replace('.', "-"))
}

/// Locate `gradle/libs.versions.toml` walking from `start` up to `gradle_root` (inclusive).
pub fn find_catalog_path(project_dir: &Path, gradle_root: Option<&Path>) -> Option<PathBuf> {
    let mut cur = project_dir.to_path_buf();
    let stop = gradle_root.map(|p| p.to_path_buf());
    loop {
        let candidate = cur.join("gradle").join("libs.versions.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        // Also accept root-level libs.versions.toml (less common).
        let alt = cur.join("libs.versions.toml");
        if alt.is_file() {
            return Some(alt);
        }
        if stop.as_ref().is_some_and(|s| s == &cur) {
            break;
        }
        if !cur.pop() {
            break;
        }
        if let Some(s) = &stop {
            // Don't walk above gradle root.
            if !cur.starts_with(s) && &cur != s {
                break;
            }
        }
    }
    // Final attempt at gradle_root if we stopped early.
    if let Some(root) = gradle_root {
        let candidate = root.join("gradle").join("libs.versions.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Load the nearest **local** catalog file, or empty if none.
pub fn load_catalog_for(project_dir: &Path, gradle_root: Option<&Path>) -> VersionCatalog {
    match find_catalog_path(project_dir, gradle_root) {
        Some(path) => VersionCatalog::load_file(&path).unwrap_or_else(|err| {
            tracing::debug!("catalog load failed for {}: {err}", path.display());
            VersionCatalog::empty()
        }),
        None => VersionCatalog::empty(),
    }
}

/// A catalog referenced from `settings.gradle.kts` `versionCatalogs { create("libs") { from(...) } }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogFrom {
    /// `from(files("gradle/libs.versions.toml"))` / `from(files("…"))`
    File(String),
    /// `from("group:name:version")` — published version catalog (e.g. from Nexus).
    Coordinate { group: String, name: String, version: String },
}

/// Workspace-wide catalog index so consumers that only have
/// `from("com.example:catalog:1.0.0")` (no local TOML) still resolve `libs.*`.
#[derive(Debug, Clone, Default)]
pub struct CatalogRegistry {
    /// Absolute path → catalog
    by_path: HashMap<PathBuf, VersionCatalog>,
    /// `group:name` (optional `:version`) → catalog
    by_coord: HashMap<String, VersionCatalog>,
    /// All catalogs for fallback resolve (union order)
    all: Vec<VersionCatalog>,
}

impl CatalogRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.all.is_empty()
    }

    pub fn insert_file(&mut self, path: PathBuf, catalog: VersionCatalog) {
        let canon = path.canonicalize().unwrap_or(path);
        if self.by_path.contains_key(&canon) {
            return;
        }
        self.by_path.insert(canon, catalog.clone());
        self.all.push(catalog);
    }

    pub fn insert_coordinate(&mut self, group: &str, name: &str, version: &str, catalog: VersionCatalog) {
        let key_ver = format!("{group}:{name}:{version}");
        let key = format!("{group}:{name}");
        self.by_coord.insert(key_ver, catalog.clone());
        self.by_coord.entry(key).or_insert_with(|| catalog.clone());
        // Avoid duplicating the same library set in `all` if already from a path.
        if catalog.path.as_ref().is_none_or(|p| !self.by_path.contains_key(p)) {
            self.all.push(catalog);
        }
    }

    /// Index every `gradle/libs.versions.toml` under `root` (shallow walk, skip build/.git).
    pub fn discover_under(&mut self, root: &Path, max_depth: usize) {
        let mut stack = vec![(root.to_path_buf(), 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            if depth > max_depth {
                continue;
            }
            let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if matches!(name, "build" | ".git" | "node_modules" | "target") {
                continue;
            }
            let candidate = dir.join("gradle").join("libs.versions.toml");
            if candidate.is_file() {
                if let Ok(cat) = VersionCatalog::load_file(&candidate) {
                    self.insert_file(candidate, cat);
                }
            }
            if depth == max_depth {
                continue;
            }
            let Ok(rd) = fs::read_dir(&dir) else {
                continue;
            };
            for ent in rd.flatten() {
                let p = ent.path();
                if p.is_dir() {
                    stack.push((p, depth + 1));
                }
            }
        }
    }

    /// Register a project that publishes a version catalog (version-catalog plugin).
    pub fn register_publisher(
        &mut self,
        project_dir: &Path,
        group: &str,
        artifact: &str,
        version: &str,
    ) {
        let Some(path) = find_catalog_path(project_dir, Some(project_dir))
            .or_else(|| find_catalog_path(project_dir, None))
        else {
            return;
        };
        let Ok(cat) = VersionCatalog::load_file(&path) else {
            return;
        };
        self.insert_file(path, cat.clone());
        if !group.is_empty() && !artifact.is_empty() {
            self.insert_coordinate(group, artifact, version, cat);
        }
    }

    pub fn get_coord(&self, group: &str, name: &str, version: &str) -> Option<&VersionCatalog> {
        let key_ver = format!("{group}:{name}:{version}");
        self.by_coord
            .get(&key_ver)
            .or_else(|| self.by_coord.get(&format!("{group}:{name}")))
    }

    /// Prefer local file, then settings `from(...)`, then any workspace catalog
    /// that can resolve at least one `libs.` accessor in the build script.
    pub fn resolve_for_project(
        &self,
        project_dir: &Path,
        gradle_root: Option<&Path>,
        build_script_hint: &str,
    ) -> VersionCatalog {
        // 1. Local / ancestor file
        if let Some(path) = find_catalog_path(project_dir, gradle_root) {
            if let Some(c) = self.by_path.get(&path.canonicalize().unwrap_or(path.clone())) {
                return c.clone();
            }
            if let Ok(c) = VersionCatalog::load_file(&path) {
                return c;
            }
        }

        // 2. settings.gradle.kts versionCatalogs { from(...) }
        let settings_dirs: Vec<&Path> = match gradle_root {
            Some(r) => vec![r, project_dir],
            None => vec![project_dir],
        };
        for dir in settings_dirs {
            for name in ["settings.gradle.kts", "settings.gradle"] {
                let sp = dir.join(name);
                if let Ok(content) = fs::read_to_string(&sp) {
                    for src in parse_settings_catalog_sources(&content) {
                        match src {
                            CatalogFrom::File(rel) => {
                                let path = dir.join(&rel);
                                if let Ok(c) = VersionCatalog::load_file(&path) {
                                    return c;
                                }
                                // Also try registry by path
                                if let Some(c) = self
                                    .by_path
                                    .get(&path.canonicalize().unwrap_or(path.clone()))
                                {
                                    return c.clone();
                                }
                            }
                            CatalogFrom::Coordinate {
                                group,
                                name,
                                version,
                            } => {
                                if let Some(c) = self.get_coord(&group, &name, &version) {
                                    return c.clone();
                                }
                            }
                        }
                    }
                }
            }
        }

        // 3. Fallback: first workspace catalog that resolves any libs.* in the script
        let accessors = collect_libs_accessors(build_script_hint);
        if !accessors.is_empty() {
            for cat in &self.all {
                let hits = accessors
                    .iter()
                    .filter(|a| cat.resolve_libs_accessor(a, "implementation", a).is_some())
                    .count();
                if hits > 0 {
                    return cat.clone();
                }
            }
        }

        VersionCatalog::empty()
    }
}

/// Parse `versionCatalogs { create("libs") { from(...) } }` sources from settings.
pub fn parse_settings_catalog_sources(content: &str) -> Vec<CatalogFrom> {
    let mut out = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        // from("g:n:v") or from('g:n:v')
        if let Some(rest) = trimmed.strip_prefix("from(") {
            let rest = rest.trim();
            // from(files("path")) or from(files('path'))
            if let Some(inner) = rest.strip_prefix("files(") {
                let inner = inner.trim_start();
                if let Some(path) = strip_quoted_token(inner) {
                    out.push(CatalogFrom::File(path));
                }
                continue;
            }
            if let Some(gav) = strip_quoted_token(rest) {
                let parts: Vec<&str> = gav.splitn(3, ':').collect();
                if parts.len() == 3 {
                    out.push(CatalogFrom::Coordinate {
                        group: parts[0].to_string(),
                        name: parts[1].to_string(),
                        version: parts[2].to_string(),
                    });
                }
            }
        }
    }
    out
}

fn strip_quoted_token(s: &str) -> Option<String> {
    let s = s.trim();
    let quote = s.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let end = s[1..].find(quote)?;
    Some(s[1..1 + end].to_string())
}

fn collect_libs_accessors(build_script: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in build_script.lines() {
        let t = line.trim();
        if let Some(idx) = t.find("libs.") {
            let rest = &t[idx..];
            let token: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '.' || *c == '_')
                .collect();
            if token.starts_with("libs.") && !token.starts_with("libs.versions") {
                out.push(token);
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[versions]
common = "1.4.2"
commonRange = "[1.0.0, 2.0.0)"
jackson = "2.15.0"

[libraries]
common-lib = { module = "com.example:common-lib", version.ref = "common" }
common-lib-range = { group = "com.example", name = "common-lib", version.ref = "commonRange" }
jackson-databind = { module = "com.fasterxml.jackson.core:jackson-databind", version.ref = "jackson" }
inline = { module = "com.acme:inline", version = "9.9.9" }
"#;

    #[test]
    fn parse_settings_from_published_coordinate() {
        let sources = parse_settings_catalog_sources(
            r#"
            versionCatalogs {
                create("libs") {
                    from("com.tako.fixtures:catalog:1.0.0")
                }
            }
            "#,
        );
        assert_eq!(
            sources,
            vec![CatalogFrom::Coordinate {
                group: "com.tako.fixtures".into(),
                name: "catalog".into(),
                version: "1.0.0".into(),
            }]
        );
    }

    #[test]
    fn libs_avro_cats_maps_to_avro_cats_alias() {
        assert_eq!(
            libs_accessor_to_alias("libs.avro.cats").as_deref(),
            Some("avro-cats")
        );
        assert_eq!(
            libs_accessor_to_alias("libs.util.core").as_deref(),
            Some("util-core")
        );
    }

    #[test]
    fn registry_resolves_published_catalog_from_settings() {
        use std::io::Write;
        let root = std::env::temp_dir().join(format!(
            "tako-reg-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let cat_dir = root.join("gradle-catalog");
        let svc = root.join("animals").join("cat");
        fs::create_dir_all(cat_dir.join("gradle")).unwrap();
        fs::create_dir_all(&svc).unwrap();
        fs::write(
            cat_dir.join("gradle/libs.versions.toml"),
            r#"
[versions]
avro-cats = "1.0.0"
util-core = "[1.0.0,2.0.0)"
[libraries]
avro-cats = { module = "com.tako.fixtures:cats", version.ref = "avro-cats" }
util-core = { module = "com.tako.fixtures:util-core", version.ref = "util-core" }
"#,
        )
        .unwrap();
        fs::write(
            cat_dir.join("build.gradle.kts"),
            r#"
plugins { `version-catalog`; `maven-publish` }
group = "com.tako.fixtures"
version = "1.0.0"
catalog { versionCatalog { from(files("gradle/libs.versions.toml")) } }
publishing {
  publications {
    create<MavenPublication>("catalog") { from(components["versionCatalog"]) }
  }
}
"#,
        )
        .unwrap();
        fs::write(
            svc.join("settings.gradle.kts"),
            r#"
dependencyResolutionManagement {
  versionCatalogs {
    create("libs") { from("com.tako.fixtures:catalog:1.0.0") }
  }
}
rootProject.name = "cat"
"#,
        )
        .unwrap();
        let build = r#"
dependencies {
    implementation(libs.avro.cats)
    implementation(libs.util.core)
}
"#;
        fs::write(svc.join("build.gradle.kts"), build).unwrap();

        let mut reg = CatalogRegistry::new();
        reg.discover_under(&root, 6);
        reg.register_publisher(&cat_dir, "com.tako.fixtures", "catalog", "1.0.0");

        let cat = reg.resolve_for_project(&svc, Some(&svc), build);
        assert!(
            cat.library("avro-cats").is_some(),
            "expected published catalog resolution"
        );
        let dep = cat
            .resolve_libs_accessor("libs.avro.cats", "implementation", "libs.avro.cats")
            .expect("libs.avro.cats");
        assert_eq!(
            dep.coordinate.as_ref().map(|c| c.key()),
            Some("com.tako.fixtures:cats".into())
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn parse_catalog_aliases_and_refs() {
        let cat = VersionCatalog::parse(SAMPLE).expect("parse");
        assert_eq!(cat.version_alias("common"), Some("1.4.2"));
        assert_eq!(cat.version_alias("commonRange"), Some("[1.0.0, 2.0.0)"));

        let lib = cat.library("common-lib").expect("common-lib");
        assert_eq!(lib.group, "com.example");
        assert_eq!(lib.name, "common-lib");
        assert_eq!(lib.version, "1.4.2");
        assert_eq!(lib.version_ref.as_deref(), Some("common"));

        let ranged = cat.library("common-lib-range").unwrap();
        assert!(matches!(ranged.version_spec(), VersionSpec::Range(_)));
        assert_eq!(ranged.version, "[1.0.0, 2.0.0)");

        let inline = cat.library("inline").unwrap();
        assert_eq!(inline.version, "9.9.9");
    }

    #[test]
    fn libs_accessor_maps_to_alias() {
        assert_eq!(
            libs_accessor_to_alias("libs.common.lib").as_deref(),
            Some("common-lib")
        );
        assert_eq!(
            libs_accessor_to_alias("libs.jackson.databind").as_deref(),
            Some("jackson-databind")
        );
        assert_eq!(libs_accessor_to_alias("libs.versions.common"), None);
    }

    #[test]
    fn resolve_libs_accessor() {
        let cat = VersionCatalog::parse(SAMPLE).unwrap();
        let dep = cat
            .resolve_libs_accessor("libs.common.lib", "implementation", "libs.common.lib")
            .unwrap();
        assert_eq!(
            dep.coordinate.as_ref().map(|c| c.key()),
            Some("com.example:common-lib".into())
        );
        assert!(matches!(dep.version, VersionSpec::Exact(ref v) if v == "1.4.2"));
    }

    #[test]
    fn load_file_roundtrip() {
        let dir = std::env::temp_dir().join(format!(
            "tako-catalog-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = fs::create_dir_all(dir.join("gradle"));
        let path = dir.join("gradle/libs.versions.toml");
        fs::write(&path, SAMPLE).unwrap();
        let cat = VersionCatalog::load_file(&path).unwrap();
        assert!(cat.library("common-lib").is_some());
        assert_eq!(cat.path.as_deref(), Some(path.as_path()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_catalog_walks_up() {
        let root = std::env::temp_dir().join(format!(
            "tako-cat-walk-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let proj = root.join("services/payments");
        fs::create_dir_all(root.join("gradle")).unwrap();
        fs::create_dir_all(&proj).unwrap();
        fs::write(root.join("gradle/libs.versions.toml"), SAMPLE).unwrap();

        let found = find_catalog_path(&proj, Some(&root));
        assert!(found.is_some());
        assert!(found.unwrap().ends_with("libs.versions.toml"));
        let _ = fs::remove_dir_all(&root);
    }
}
