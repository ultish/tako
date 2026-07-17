//! Maven coordinates and dependency/produce declarations.

use serde::{Deserialize, Serialize};

/// Maven-style `group:name` coordinate (version lives on the dep req or project).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Coordinate {
    pub group: String,
    pub name: String,
}

impl Coordinate {
    pub fn new(group: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            group: group.into(),
            name: name.into(),
        }
    }

    /// Parse `group:name` or `group:name:version` (version discarded).
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        let mut parts = s.splitn(3, ':');
        let group = parts.next()?.trim();
        let name = parts.next()?.trim();
        if group.is_empty() || name.is_empty() {
            return None;
        }
        Some(Self::new(group, name))
    }

    /// `group:name` key used for graph matching.
    pub fn key(&self) -> String {
        format!("{}:{}", self.group, self.name)
    }

    pub fn display(&self) -> String {
        self.key()
    }
}

impl std::fmt::Display for Coordinate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.group, self.name)
    }
}

/// How a dependency version is expressed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum VersionSpec {
    /// Exact version string (`1.4.2`).
    Exact(String),
    /// Maven/Ivy range or dynamic version (`[1.0.0, 2.0.0)`, `1.0.+`).
    Range(String),
    /// Same-settings project dependency (`:shared` / `shared`).
    Project(String),
    /// Could not resolve (e.g. unknown catalog alias).
    Unresolved,
}

impl VersionSpec {
    /// Classify a version token from a GAV string or catalog entry.
    pub fn from_version_string(v: &str) -> Self {
        let v = v.trim();
        if v.is_empty() {
            return VersionSpec::Unresolved;
        }
        if is_range_or_dynamic(v) {
            VersionSpec::Range(v.to_string())
        } else {
            VersionSpec::Exact(v.to_string())
        }
    }

    pub fn display(&self) -> String {
        match self {
            VersionSpec::Exact(v) | VersionSpec::Range(v) => v.clone(),
            VersionSpec::Project(p) => format!("project({p})"),
            VersionSpec::Unresolved => "—".into(),
        }
    }
}

/// True when `v` looks like a Maven range or dynamic version (not a plain exact).
pub fn is_range_or_dynamic(v: &str) -> bool {
    let v = v.trim();
    if v.is_empty() {
        return false;
    }
    if v.starts_with('[') || v.starts_with('(') {
        return true;
    }
    if v.ends_with('+') || v.contains(',') {
        return true;
    }
    // `1.0.+` style mid-token
    if v.contains(".+") {
        return true;
    }
    false
}

/// One dependency requirement extracted from a build script / catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DepReq {
    /// External Maven coordinate when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coordinate: Option<Coordinate>,
    pub version: VersionSpec,
    /// Gradle configuration (`implementation`, `api`, …).
    #[serde(default)]
    pub configuration: String,
    /// Original source expression for display/debug.
    #[serde(default)]
    pub raw: String,
}

impl DepReq {
    pub fn external(
        coordinate: Coordinate,
        version: VersionSpec,
        configuration: impl Into<String>,
        raw: impl Into<String>,
    ) -> Self {
        Self {
            coordinate: Some(coordinate),
            version,
            configuration: configuration.into(),
            raw: raw.into(),
        }
    }

    pub fn project_dep(
        path: impl Into<String>,
        configuration: impl Into<String>,
        raw: impl Into<String>,
    ) -> Self {
        let path = path.into();
        Self {
            coordinate: None,
            version: VersionSpec::Project(path),
            configuration: configuration.into(),
            raw: raw.into(),
        }
    }

    /// Project path without leading `:` when this is a project dep.
    pub fn project_path(&self) -> Option<&str> {
        match &self.version {
            VersionSpec::Project(p) => Some(p.as_str()),
            _ => None,
        }
    }

    pub fn display(&self) -> String {
        if let Some(c) = &self.coordinate {
            format!("{}:{}", c, self.version.display())
        } else if let Some(p) = self.project_path() {
            format!("project(:{p})")
        } else {
            self.raw.clone()
        }
    }
}

/// A coordinate this project publishes (version usually from project version).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Produces {
    pub coordinate: Coordinate,
    /// Published version string (may be empty / `—` when unknown).
    #[serde(default)]
    pub version: String,
}

impl Produces {
    pub fn new(coordinate: Coordinate, version: impl Into<String>) -> Self {
        Self {
            coordinate,
            version: version.into(),
        }
    }

    pub fn display(&self) -> String {
        if self.version.is_empty() || self.version == "—" {
            self.coordinate.display()
        } else {
            format!("{}:{}", self.coordinate, self.version)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinate_parse_gav() {
        let c = Coordinate::parse("com.example:common-lib:1.4.2").unwrap();
        assert_eq!(c.group, "com.example");
        assert_eq!(c.name, "common-lib");
        assert_eq!(c.key(), "com.example:common-lib");
    }

    #[test]
    fn coordinate_parse_ga() {
        let c = Coordinate::parse("com.acme:foo").unwrap();
        assert_eq!(c.name, "foo");
    }

    #[test]
    fn version_spec_classifies_ranges() {
        assert!(matches!(
            VersionSpec::from_version_string("1.4.2"),
            VersionSpec::Exact(_)
        ));
        assert!(matches!(
            VersionSpec::from_version_string("[1.0.0, 2.0.0)"),
            VersionSpec::Range(_)
        ));
        assert!(matches!(
            VersionSpec::from_version_string("1.0.+"),
            VersionSpec::Range(_)
        ));
    }
}
