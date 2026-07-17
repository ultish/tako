//! Version range inclusion for cascade consumer selection.
//!
//! Covers the Maven/Ivy subset actually used in catalogs:
//! - exact `1.2.3`
//! - closed/open ranges: `[1.0.0, 2.0.0)`, `(1.0, 2.0]`, `[1.0,2.0]`
//! - dynamic prefix: `1.0.+`
//!
//! Not a full Maven range engine — static, offline, no `gradle dependencies`.

use super::model::VersionSpec;

/// Does `version` satisfy `spec`?
pub fn version_matches(spec: &VersionSpec, version: &str) -> bool {
    let version = version.trim();
    if version.is_empty() || version == "—" {
        return false;
    }
    match spec {
        VersionSpec::Exact(v) => versions_equal(v, version),
        VersionSpec::Range(r) => range_includes(r, version),
        VersionSpec::Project(_) | VersionSpec::Unresolved => false,
    }
}

/// Convenience: does the range/exact string `req` include `version`?
pub fn requirement_includes(req: &str, version: &str) -> bool {
    let spec = VersionSpec::from_version_string(req);
    version_matches(&spec, version)
}

fn versions_equal(a: &str, b: &str) -> bool {
    normalize_version(a) == normalize_version(b)
}

fn normalize_version(v: &str) -> String {
    v.trim().to_string()
}

/// Parse and evaluate a Maven-style range or dynamic version against `candidate`.
pub fn range_includes(range: &str, candidate: &str) -> bool {
    let range = range.trim();
    let candidate = candidate.trim();
    if range.is_empty() || candidate.is_empty() {
        return false;
    }

    // Dynamic: `1.0.+` → candidate starts with `1.0.`
    if let Some(prefix) = range.strip_suffix(".+") {
        return candidate == prefix
            || candidate.starts_with(&format!("{prefix}."))
            || candidate.starts_with(&format!("{prefix}-"));
    }
    if range.ends_with('+') && !range.starts_with('[') && !range.starts_with('(') {
        let prefix = range.trim_end_matches('+');
        return candidate == prefix || candidate.starts_with(prefix);
    }

    // Bracket range: [low,high) etc. — may be a single bound `[1.0,)` or `(,2.0]`
    if (range.starts_with('[') || range.starts_with('('))
        && (range.ends_with(']') || range.ends_with(')'))
    {
        return bracket_range_includes(range, candidate);
    }

    // Fallback: treat as exact
    versions_equal(range, candidate)
}

fn bracket_range_includes(range: &str, candidate: &str) -> bool {
    let open_inclusive = range.starts_with('[');
    let close_inclusive = range.ends_with(']');
    let inner = &range[1..range.len() - 1];
    let (low_s, high_s) = split_range_bounds(inner);

    let low_ok = if low_s.is_empty() {
        true
    } else {
        match cmp_versions(candidate, low_s) {
            Some(std::cmp::Ordering::Greater) => true,
            Some(std::cmp::Ordering::Equal) => open_inclusive,
            Some(std::cmp::Ordering::Less) => false,
            None => {
                // Non-semver fallback: string compare
                if open_inclusive {
                    candidate >= low_s
                } else {
                    candidate > low_s
                }
            }
        }
    };

    let high_ok = if high_s.is_empty() {
        true
    } else {
        match cmp_versions(candidate, high_s) {
            Some(std::cmp::Ordering::Less) => true,
            Some(std::cmp::Ordering::Equal) => close_inclusive,
            Some(std::cmp::Ordering::Greater) => false,
            None => {
                if close_inclusive {
                    candidate <= high_s
                } else {
                    candidate < high_s
                }
            }
        }
    };

    low_ok && high_ok
}

fn split_range_bounds(inner: &str) -> (&str, &str) {
    // Split on first comma not inside quotes (we don't support quoted versions).
    if let Some(idx) = inner.find(',') {
        (inner[..idx].trim(), inner[idx + 1..].trim())
    } else {
        // Single-sided or degenerate — treat as exact lower bound only
        (inner.trim(), "")
    }
}

/// Compare dotted numeric versions with optional `-qualifier` suffix.
/// Returns `None` when either side is not parseable as a version-ish token.
pub fn cmp_versions(a: &str, b: &str) -> Option<std::cmp::Ordering> {
    let (a_nums, a_qual) = split_numeric_qualifier(a)?;
    let (b_nums, b_qual) = split_numeric_qualifier(b)?;

    let max_len = a_nums.len().max(b_nums.len());
    for i in 0..max_len {
        let av = a_nums.get(i).copied().unwrap_or(0);
        let bv = b_nums.get(i).copied().unwrap_or(0);
        match av.cmp(&bv) {
            std::cmp::Ordering::Equal => {}
            other => return Some(other),
        }
    }

    // Numeric equal: empty qualifier (release) > any qualifier (SNAPSHOT, RC, …)
    // matches common Maven/semver-ish ordering for cascade purposes.
    match (a_qual.is_empty(), b_qual.is_empty()) {
        (true, true) => Some(std::cmp::Ordering::Equal),
        (true, false) => Some(std::cmp::Ordering::Greater),
        (false, true) => Some(std::cmp::Ordering::Less),
        (false, false) => Some(a_qual.cmp(&b_qual)),
    }
}

fn split_numeric_qualifier(v: &str) -> Option<(Vec<u64>, String)> {
    let v = v.trim();
    if v.is_empty() {
        return None;
    }
    // Split on first non-version separator for qualifier: `-` or `+` or letter after digits.
    let (num_part, qual) = if let Some(idx) = v.find(|c: char| c == '-' || c == '+') {
        (&v[..idx], v[idx + 1..].to_ascii_lowercase())
    } else {
        (v, String::new())
    };

    // Also allow `1.0.0RC1` without dash
    let (num_part, qual) = if qual.is_empty() {
        if let Some(idx) = num_part.find(|c: char| c.is_ascii_alphabetic()) {
            (
                &num_part[..idx],
                num_part[idx..].to_ascii_lowercase(),
            )
        } else {
            (num_part, String::new())
        }
    } else {
        (num_part, qual)
    };

    if num_part.is_empty() {
        return None;
    }

    let mut nums = Vec::new();
    for part in num_part.split('.') {
        if part.is_empty() {
            return None;
        }
        // Reject pure non-numeric segments in the numeric prefix
        match part.parse::<u64>() {
            Ok(n) => nums.push(n),
            Err(_) => return None,
        }
    }
    Some((nums, qual))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gradle::model::VersionSpec;

    #[test]
    fn exact_match() {
        assert!(version_matches(
            &VersionSpec::Exact("1.4.2".into()),
            "1.4.2"
        ));
        assert!(!version_matches(
            &VersionSpec::Exact("1.4.2".into()),
            "1.4.3"
        ));
    }

    #[test]
    fn half_open_range() {
        // [1.0.0, 2.0.0)
        assert!(range_includes("[1.0.0, 2.0.0)", "1.0.0"));
        assert!(range_includes("[1.0.0, 2.0.0)", "1.4.2"));
        assert!(range_includes("[1.0.0, 2.0.0)", "1.9.9"));
        assert!(!range_includes("[1.0.0, 2.0.0)", "2.0.0"));
        assert!(!range_includes("[1.0.0, 2.0.0)", "0.9.0"));
    }

    #[test]
    fn open_closed_range() {
        // (1.0, 2.0]
        assert!(!range_includes("(1.0, 2.0]", "1.0"));
        assert!(!range_includes("(1.0, 2.0]", "1.0.0"));
        assert!(range_includes("(1.0, 2.0]", "1.0.1"));
        assert!(range_includes("(1.0, 2.0]", "2.0"));
        assert!(range_includes("(1.0, 2.0]", "2.0.0"));
        assert!(!range_includes("(1.0, 2.0]", "2.0.1"));
    }

    #[test]
    fn closed_range() {
        assert!(range_includes("[1.0,2.0]", "1.0"));
        assert!(range_includes("[1.0,2.0]", "2.0"));
        assert!(!range_includes("[1.0,2.0]", "2.1"));
    }

    #[test]
    fn dynamic_plus() {
        assert!(range_includes("1.0.+", "1.0.0"));
        assert!(range_includes("1.0.+", "1.0.5"));
        assert!(!range_includes("1.0.+", "1.1.0"));
        assert!(!range_includes("1.0.+", "2.0.0"));
    }

    #[test]
    fn requirement_includes_helper() {
        assert!(requirement_includes("[1.0.0, 2.0.0)", "1.4.2"));
        assert!(!requirement_includes("[1.0.0, 2.0.0)", "2.0.0"));
        assert!(requirement_includes("1.4.2", "1.4.2"));
    }

    #[test]
    fn snapshot_orders_below_release() {
        assert_eq!(
            cmp_versions("1.0.0", "1.0.0-SNAPSHOT"),
            Some(std::cmp::Ordering::Greater)
        );
        assert!(range_includes("[1.0.0, 2.0.0)", "1.0.0-SNAPSHOT") == false
            || range_includes("[1.0.0, 2.0.0)", "1.5.0-SNAPSHOT"));
        // 1.5.0-SNAPSHOT is still < 2.0.0
        assert!(range_includes("[1.0.0, 2.0.0)", "1.5.0-SNAPSHOT"));
    }

    #[test]
    fn unbounded_sides() {
        assert!(range_includes("[1.0,)", "99.0.0"));
        assert!(range_includes("(,2.0)", "1.9.9"));
        assert!(!range_includes("(,2.0)", "2.0.0"));
    }
}
