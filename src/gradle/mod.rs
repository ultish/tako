//! Static Gradle metadata: version catalog, light build-script parse, ranges.
//!
//! No `gradle dependencies` CLI — offline parse only (SPEC M2).

pub mod catalog;
pub mod model;
pub mod parse;
pub mod ranges;

#[allow(unused_imports)]
pub use catalog::{find_catalog_path, load_catalog_for, VersionCatalog};
#[allow(unused_imports)]
pub use model::{Coordinate, DepReq, Produces, VersionSpec};
#[allow(unused_imports)]
pub use parse::{extract_for_project, parse_build_script, parse_dependency_line, GradleModel};
#[allow(unused_imports)]
pub use ranges::{range_includes, requirement_includes, version_matches};
