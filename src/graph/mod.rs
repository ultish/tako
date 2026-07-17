//! Static dependency graph: dependents / dependencies / range-aware consumers.
//!
//! Built from scan results (`DiscoveredProject` depends/produces). No Gradle CLI.

use std::collections::{HashMap, HashSet};

use crate::gradle::model::{Coordinate, DepReq, Produces, VersionSpec};
use crate::gradle::ranges::version_matches;
use crate::scan::DiscoveredProject;

/// In-memory graph for highlight + cascade queries.
#[derive(Debug, Clone, Default)]
pub struct DependencyGraph {
    /// Parallel to inventory: project id (path-relative / DiscoveredProject.id).
    pub project_ids: Vec<String>,
    /// id → row index in the last build (also matches App.projects order when
    /// built from the same scan result).
    id_to_index: HashMap<String, usize>,
    /// id → path basename / module name candidates for project() matching.
    module_names: HashMap<String, HashSet<String>>,
    depends: HashMap<String, Vec<DepReq>>,
    produces: HashMap<String, Vec<Produces>>,
    /// Local published version per project id.
    versions: HashMap<String, String>,
}

impl DependencyGraph {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.project_ids.is_empty()
    }

    pub fn len(&self) -> usize {
        self.project_ids.len()
    }

    /// Build graph from discovered projects (order preserved).
    pub fn from_projects(projects: &[DiscoveredProject]) -> Self {
        let mut g = Self::default();
        for (idx, p) in projects.iter().enumerate() {
            g.project_ids.push(p.id.clone());
            g.id_to_index.insert(p.id.clone(), idx);
            // Also index by absolute path string for robustness.
            let path_key = p.path.to_string_lossy().into_owned();
            g.id_to_index.entry(path_key).or_insert(idx);

            let mut names = HashSet::new();
            names.insert(p.id.clone());
            if let Some(leaf) = p.path.file_name() {
                names.insert(leaf.to_string_lossy().into_owned());
            }
            // Relative path segments: "services/payments" → also "payments"
            for seg in p.id.split('/') {
                if !seg.is_empty() {
                    names.insert(seg.to_string());
                }
            }
            // folder_group/leaf style name
            names.insert(p.name.clone());
            for seg in p.name.split('/') {
                if !seg.is_empty() {
                    names.insert(seg.to_string());
                }
            }
            g.module_names.insert(p.id.clone(), names);

            g.depends.insert(p.id.clone(), p.depends.clone());
            g.produces.insert(p.id.clone(), p.produces.clone());
            g.versions.insert(p.id.clone(), p.version.clone());
        }
        g
    }

    pub fn index_of(&self, project_id: &str) -> Option<usize> {
        self.id_to_index.get(project_id).copied()
    }

    /// Inventory index for a project id, if present.
    pub fn index_of_id(&self, project_id: &str) -> Option<usize> {
        self.id_to_index.get(project_id).copied()
    }

    pub fn id_at(&self, index: usize) -> Option<&str> {
        self.project_ids.get(index).map(String::as_str)
    }

    pub fn depends_of_id(&self, project_id: &str) -> &[DepReq] {
        self.depends
            .get(project_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn produces_of_id(&self, project_id: &str) -> &[Produces] {
        self.produces
            .get(project_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Project ids that `project_id` depends on (project deps + matching producers).
    pub fn dependencies_of(&self, project_id: &str) -> Vec<String> {
        let Some(deps) = self.depends.get(project_id) else {
            return vec![];
        };
        let mut out: HashSet<String> = HashSet::new();

        for dep in deps {
            // Same-settings project(":x")
            if let Some(path) = dep.project_path() {
                if let Some(id) = self.resolve_project_path(path) {
                    if id != project_id {
                        out.insert(id);
                    }
                }
                continue;
            }
            // External coord → any other project that produces matching G:A
            // (version not required for "what I use" highlight — any producer of G:A).
            // Also match **artifact name only** when groups differ: consumers often
            // write `implementation("util-libs:util-core:1.x")` while the publisher
            // uses `group = "com.tako.fixtures"` (Nexus/module layout vs Maven group).
            if let Some(coord) = &dep.coordinate {
                for id in self.producers_matching_coord(coord) {
                    if id != project_id {
                        out.insert(id);
                    }
                }
            }
        }

        let mut v: Vec<String> = out.into_iter().collect();
        v.sort();
        v
    }

    /// Project ids that depend on `project_id` (project edges + coord/range match
    /// against this project's **current** produced version(s)).
    pub fn dependents_of(&self, project_id: &str) -> Vec<String> {
        let mut out: HashSet<String> = HashSet::new();

        // Project-path reverse edges
        let module_names = self
            .module_names
            .get(project_id)
            .cloned()
            .unwrap_or_default();
        for (other_id, deps) in &self.depends {
            if other_id == project_id {
                continue;
            }
            for dep in deps {
                if let Some(path) = dep.project_path() {
                    let norm = path.trim_start_matches(':');
                    if module_names.contains(norm) || module_names.contains(path) {
                        out.insert(other_id.clone());
                    }
                }
            }
        }

        // Coordinate + range against each produced version
        if let Some(produces) = self.produces.get(project_id) {
            for prod in produces {
                let ver = if prod.version.is_empty() || prod.version == "—" {
                    self.versions
                        .get(project_id)
                        .cloned()
                        .unwrap_or_default()
                } else {
                    prod.version.clone()
                };
                for consumer in self.consumers_of_version(&prod.coordinate, &ver) {
                    if consumer != project_id {
                        out.insert(consumer);
                    }
                }
            }
        }

        let mut v: Vec<String> = out.into_iter().collect();
        v.sort();
        v
    }

    /// Project ids that produce `coord` (exact G:A, else same artifact **name**).
    pub fn producers_matching_coord(&self, coord: &Coordinate) -> Vec<String> {
        let mut exact = Vec::new();
        let mut by_name = Vec::new();
        for (prod_id, produces) in &self.produces {
            for p in produces {
                if p.coordinate == *coord {
                    exact.push(prod_id.clone());
                } else if p.coordinate.name == coord.name {
                    by_name.push(prod_id.clone());
                }
            }
        }
        if !exact.is_empty() {
            exact.sort();
            exact.dedup();
            return exact;
        }
        by_name.sort();
        by_name.dedup();
        by_name
    }

    fn dep_matches_coord(dep_coord: &Coordinate, target: &Coordinate) -> bool {
        dep_coord == target || dep_coord.name == target.name
    }

    /// Projects whose dependency on `coord` includes `version` (exact or range).
    /// Matches full `group:name` or artifact **name** alone (see producers_matching_coord).
    pub fn consumers_of_version(&self, coord: &Coordinate, version: &str) -> Vec<String> {
        let mut out = Vec::new();
        for (id, deps) in &self.depends {
            for dep in deps {
                let Some(c) = &dep.coordinate else {
                    continue;
                };
                if !Self::dep_matches_coord(c, coord) {
                    continue;
                }
                if version_matches(&dep.version, version) {
                    out.push(id.clone());
                    break;
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// Consumers that depend on `coord` but whose range **excludes** `version`
    /// (catalog mismatch warning for M4; useful diagnostic in M2).
    pub fn consumers_excluded_by_range(&self, coord: &Coordinate, version: &str) -> Vec<String> {
        let mut out = Vec::new();
        for (id, deps) in &self.depends {
            for dep in deps {
                let Some(c) = &dep.coordinate else {
                    continue;
                };
                if !Self::dep_matches_coord(c, coord) {
                    continue;
                }
                match &dep.version {
                    VersionSpec::Exact(_) | VersionSpec::Range(_) => {
                        if !version_matches(&dep.version, version) {
                            out.push(id.clone());
                            break;
                        }
                    }
                    _ => {}
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// Map dependency/dependent project ids → inventory indices.
    pub fn indices_for_ids(&self, ids: &[String]) -> HashSet<usize> {
        ids.iter()
            .filter_map(|id| self.id_to_index.get(id).copied())
            .collect()
    }

    /// Highlight sets for a selected inventory index.
    pub fn highlight_for_index(&self, selected: usize) -> (HashSet<usize>, HashSet<usize>) {
        let Some(id) = self.id_at(selected) else {
            return (HashSet::new(), HashSet::new());
        };
        let deps = self.dependencies_of(id);
        let dependents = self.dependents_of(id);
        (
            self.indices_for_ids(&deps),
            self.indices_for_ids(&dependents),
        )
    }

    fn resolve_project_path(&self, path: &str) -> Option<String> {
        let norm = path.trim_start_matches(':');
        // Exact id match
        if self.id_to_index.contains_key(norm) {
            return Some(norm.to_string());
        }
        if self.id_to_index.contains_key(path) {
            return Some(path.to_string());
        }
        // Match against module name sets
        for (id, names) in &self.module_names {
            if names.contains(norm) || names.contains(path) {
                return Some(id.clone());
            }
        }
        // Suffix match: path "shared" matches id "libs/shared"
        for id in &self.project_ids {
            if id == norm || id.ends_with(&format!("/{norm}")) {
                return Some(id.clone());
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::ProjectKind;
    use crate::gradle::model::{Coordinate, DepReq, Produces, VersionSpec};
    use std::path::PathBuf;

    #[test]
    fn graph_links_gav_string_dep_to_producer_by_artifact_name() {
        // Consumer declares Nexus-style GAV with a different group than the
        // publisher's `group =` — still link on artifact name for cascade/UI.
        let lib = proj(
            "util-core",
            "1.0.0",
            vec![Produces::new(
                Coordinate::new("com.tako.fixtures", "util-core"),
                "1.0.0",
            )],
            vec![],
        );
        let mut svc = proj("cat", "0.1.0", vec![], vec![]);
        svc.depends = vec![DepReq::external(
            Coordinate::new("util-libs", "util-core"),
            VersionSpec::Range("[1.0.0,2.0.0)".into()),
            "implementation",
            "util-libs:util-core:[1.0.0,2.0.0)",
        )];
        let g = DependencyGraph::from_projects(&[lib, svc]);
        let deps = g.dependencies_of("cat");
        assert!(
            deps.iter().any(|d| d == "util-core"),
            "name-only match: {deps:?}"
        );
        let dependents = g.dependents_of("util-core");
        assert!(
            dependents.iter().any(|d| d == "cat"),
            "consumers: {dependents:?}"
        );
    }

    fn proj(
        id: &str,
        version: &str,
        produces: Vec<Produces>,
        depends: Vec<DepReq>,
    ) -> DiscoveredProject {
        DiscoveredProject {
            id: id.into(),
            path: PathBuf::from(format!("/ws/{id}")),
            git_root: None,
            name: id.into(),
            kind: ProjectKind::Library,
            version: version.into(),
            branch: "main".into(),
            git_dirty: false,
            has_skaffold: false,
            skaffold_files: vec![],
            gradle_root: Some(PathBuf::from("/ws")),
            folder_group: id.split('/').next().unwrap_or(id).into(),
            status: "idle".into(),
            depends,
            produces,
        }
    }

    #[test]
    fn project_dep_edges_both_ways() {
        let shared = proj(
            "shared",
            "1.0.0",
            vec![Produces::new(
                Coordinate::new("com.example", "shared"),
                "1.0.0",
            )],
            vec![],
        );
        let api = proj(
            "payments-api",
            "3.0.0",
            vec![],
            vec![DepReq::project_dep("shared", "implementation", "project")],
        );
        let g = DependencyGraph::from_projects(&[shared, api]);

        assert_eq!(g.dependencies_of("payments-api"), vec!["shared".to_string()]);
        assert_eq!(g.dependents_of("shared"), vec!["payments-api".to_string()]);
        assert!(g.dependencies_of("shared").is_empty());
    }

    #[test]
    fn range_consumer_selection() {
        let lib = proj(
            "common-lib",
            "1.4.2",
            vec![Produces::new(
                Coordinate::new("com.example", "common-lib"),
                "1.4.2",
            )],
            vec![],
        );
        let consumer_ok = proj(
            "orders-api",
            "2.0.0",
            vec![],
            vec![DepReq::external(
                Coordinate::new("com.example", "common-lib"),
                VersionSpec::Range("[1.0.0, 2.0.0)".into()),
                "implementation",
                "range",
            )],
        );
        let consumer_old = proj(
            "legacy-api",
            "1.0.0",
            vec![],
            vec![DepReq::external(
                Coordinate::new("com.example", "common-lib"),
                VersionSpec::Exact("1.0.0".into()),
                "implementation",
                "exact",
            )],
        );
        let g = DependencyGraph::from_projects(&[lib, consumer_ok, consumer_old]);

        let consumers = g.consumers_of_version(
            &Coordinate::new("com.example", "common-lib"),
            "1.4.2",
        );
        assert_eq!(consumers, vec!["orders-api".to_string()]);

        let dependents = g.dependents_of("common-lib");
        assert_eq!(dependents, vec!["orders-api".to_string()]);

        let excluded = g.consumers_excluded_by_range(
            &Coordinate::new("com.example", "common-lib"),
            "1.4.2",
        );
        assert_eq!(excluded, vec!["legacy-api".to_string()]);
    }

    #[test]
    fn highlight_indices() {
        let lib = proj(
            "common-lib",
            "1.4.2",
            vec![Produces::new(
                Coordinate::new("com.example", "common-lib"),
                "1.4.2",
            )],
            vec![],
        );
        let svc = proj(
            "svc",
            "1.0",
            vec![],
            vec![DepReq::external(
                Coordinate::new("com.example", "common-lib"),
                VersionSpec::Range("[1.0.0, 2.0.0)".into()),
                "api",
                "x",
            )],
        );
        let g = DependencyGraph::from_projects(&[lib, svc]);
        // select common-lib (index 0) → dependent svc (1)
        let (deps, dependents) = g.highlight_for_index(0);
        assert!(deps.is_empty());
        assert!(dependents.contains(&1));
        // select svc → dep common-lib (0)
        let (deps, dependents) = g.highlight_for_index(1);
        assert!(deps.contains(&0));
        assert!(dependents.is_empty());
    }

    #[test]
    fn cascade_consumer_set_for_publish() {
        // Flow B: publish common-lib 1.4.2 → range consumers rebuild
        let projects = vec![
            proj(
                "common-lib",
                "1.4.2",
                vec![Produces::new(
                    Coordinate::new("com.example", "common-lib"),
                    "1.4.2",
                )],
                vec![],
            ),
            proj(
                "payments",
                "3.1.0",
                vec![],
                vec![DepReq::external(
                    Coordinate::new("com.example", "common-lib"),
                    VersionSpec::Range("[1.0.0, 2.0.0)".into()),
                    "implementation",
                    "libs.common.lib",
                )],
            ),
            proj(
                "billing",
                "3.0.1",
                vec![],
                vec![DepReq::external(
                    Coordinate::new("com.example", "common-lib"),
                    VersionSpec::Range("[1.0.0, 2.0.0)".into()),
                    "implementation",
                    "libs.common.lib",
                )],
            ),
            proj(
                "unrelated",
                "1.0.0",
                vec![],
                vec![DepReq::external(
                    Coordinate::new("com.other", "thing"),
                    VersionSpec::Exact("1.0.0".into()),
                    "implementation",
                    "x",
                )],
            ),
        ];
        let g = DependencyGraph::from_projects(&projects);
        let cascade = g.dependents_of("common-lib");
        assert_eq!(cascade, vec!["billing".to_string(), "payments".to_string()]);
    }
}
