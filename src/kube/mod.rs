//! Phase 2: read deployed versions from Kubernetes via `kubectl` (read-only).
//!
//! Uses the user's kubeconfig / context — no in-process cluster client.
//! Default version signal: Deployment label `app.kubernetes.io/version`.

use std::path::Path;
use std::process::Command;

use serde_json::Value;

use crate::app::{Drift, ProjectKind, ProjectRow};
use crate::config::KubeConfig;
use crate::gradle::ranges::cmp_versions;

/// How to extract a version string from a Deployment object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionSource {
    /// Try common signals in order (recommended for Skaffold/dev clusters).
    Auto,
    /// `metadata.labels[key]`
    Label(String),
    /// `metadata.annotations[key]`
    Annotation(String),
    /// First container image tag (`repo:tag` → `tag`; digests → unknown)
    ImageTag,
    /// First container env var `name`
    Env(String),
}

impl VersionSource {
    /// Parse config strings like `auto`, `label:app.kubernetes.io/version`,
    /// `image_tag`, `annotation:tako/version`, `env:APP_VERSION`.
    pub fn parse(s: &str) -> Self {
        let s = s.trim();
        if s.is_empty() || s.eq_ignore_ascii_case("auto") {
            return VersionSource::Auto;
        }
        if s == "image_tag" {
            return VersionSource::ImageTag;
        }
        if let Some(key) = s.strip_prefix("label:") {
            return VersionSource::Label(key.to_string());
        }
        if let Some(key) = s.strip_prefix("annotation:") {
            return VersionSource::Annotation(key.to_string());
        }
        if let Some(key) = s.strip_prefix("env:") {
            return VersionSource::Env(key.to_string());
        }
        // Bare key → label
        VersionSource::Label(s.to_string())
    }
}

/// One project's probe outcome (matched by absolute project path).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeployedProbe {
    pub project_path: std::path::PathBuf,
    pub project_name: String,
    /// Workload name we queried (deployment).
    pub deployment: String,
    pub namespace: String,
    pub deployed_version: Option<String>,
    /// Soft ownership hint (`argocd` when tracking labels/annotations present).
    pub deploy_owner: Option<String>,
    pub error: Option<String>,
}

/// Full probe batch result.
#[derive(Debug, Clone)]
pub struct ProbeBatch {
    pub probes: Vec<DeployedProbe>,
    /// Non-fatal summary (kubectl missing, empty config, …).
    pub error: Option<String>,
}

/// Whether kube probing is configured enough to attempt.
pub fn kube_probe_enabled(cfg: &KubeConfig) -> bool {
    cfg.enabled && !cfg.namespaces.is_empty()
}

/// Leaf name used as default Deployment name (last path segment of project name).
pub fn default_deployment_name(project: &ProjectRow) -> String {
    project
        .name
        .rsplit('/')
        .next()
        .unwrap_or(project.name.as_str())
        .to_string()
}

/// Projects that get a kube probe: services with skaffold (or kind service).
pub fn should_probe(project: &ProjectRow) -> bool {
    project.has_skaffold || matches!(project.kind, ProjectKind::Service)
}

/// Compare local Gradle version vs deployed cluster version.
///
/// `probed` false → [`Drift::NotProbed`] (or NotApplicable for non-services — callers
/// should not call this for non-probeable rows).
pub fn compute_drift(local: &str, deployed: Option<&str>, probed: bool) -> Drift {
    if !probed {
        return Drift::NotProbed;
    }
    let Some(dep) = deployed.map(str::trim).filter(|s| !s.is_empty() && *s != "—") else {
        return Drift::Unknown;
    };
    let local = local.trim();
    if local.is_empty() || local == "—" {
        return Drift::Unknown;
    }
    if local == dep {
        return Drift::Match;
    }
    match cmp_versions(local, dep) {
        Some(std::cmp::Ordering::Greater) => Drift::LocalAhead,
        Some(std::cmp::Ordering::Less) => Drift::ClusterAhead,
        Some(std::cmp::Ordering::Equal) => Drift::Match,
        None => {
            // Non-semver strings that differ
            if local.eq_ignore_ascii_case(dep) {
                Drift::Match
            } else {
                Drift::Unknown
            }
        }
    }
}

/// Extract version + owner hints from a Deployment JSON object.
///
/// When the preferred source yields nothing, falls back through common signals
/// (version labels → semver-ish image tags → `"live"` if the Deployment exists).
pub fn extract_from_deployment(
    deploy: &Value,
    source: &VersionSource,
) -> (Option<String>, Option<String>) {
    let preferred = extract_version_once(deploy, source);
    let version = preferred
        .or_else(|| extract_version_once(deploy, &VersionSource::Auto))
        .or_else(|| {
            // Deployment is live but no comparable version signal (common with
            // Skaffold content-hash image tags and bare `app:` labels).
            Some("live".into())
        });

    let owner = detect_deploy_owner(deploy);
    (version, owner)
}

fn extract_version_once(deploy: &Value, source: &VersionSource) -> Option<String> {
    match source {
        VersionSource::Auto => {
            // Prefer explicit version labels over image tags (Skaffold often
            // tags with content hashes that are not the Gradle version).
            for key in [
                "app.kubernetes.io/version",
                "version",
                "app.version",
            ] {
                if let Some(v) = label_get(deploy, key) {
                    return Some(v);
                }
            }
            for key in ["tako/version", "app.kubernetes.io/version"] {
                if let Some(v) = annotation_get(deploy, key) {
                    return Some(v);
                }
            }
            // Only use image tag when it looks like a product version, not a digest hash.
            first_image_tag(deploy).filter(|t| looks_like_product_version(t))
        }
        VersionSource::Label(key) => label_get(deploy, key),
        VersionSource::Annotation(key) => annotation_get(deploy, key),
        VersionSource::ImageTag => first_image_tag(deploy),
        VersionSource::Env(name) => first_env_value(deploy, name),
    }
}

/// True for tags like `0.1.0`, `v1.2.3-SNAPSHOT` — false for 40–64 hex digests.
fn looks_like_product_version(tag: &str) -> bool {
    let t = tag.trim();
    if t.is_empty() || t == "latest" {
        return false;
    }
    // Long pure-hex = content hash / image id
    if t.len() >= 32 && t.chars().all(|c| c.is_ascii_hexdigit()) {
        return false;
    }
    // Has a digit somewhere (0.1.0, 1.2.3-rc1, build-42)
    t.chars().any(|c| c.is_ascii_digit())
}

fn json_pointer_escape(key: &str) -> String {
    // RFC 6901: ~ → ~0, / → ~1
    key.replace('~', "~0").replace('/', "~1")
}

fn label_get(deploy: &Value, key: &str) -> Option<String> {
    deploy
        .get("metadata")?
        .get("labels")?
        .get(key)?
        .as_str()
        .map(str::to_string)
}

fn annotation_get(deploy: &Value, key: &str) -> Option<String> {
    deploy
        .get("metadata")?
        .get("annotations")?
        .get(key)?
        .as_str()
        .map(str::to_string)
}

fn first_image_tag(deploy: &Value) -> Option<String> {
    let containers = deploy
        .pointer("/spec/template/spec/containers")?
        .as_array()?;
    for c in containers {
        let image = c.get("image")?.as_str()?;
        if let Some(tag) = image_tag(image) {
            return Some(tag);
        }
    }
    None
}

/// `repo:1.2.3` → `1.2.3`; digests / no tag → None.
pub fn image_tag(image: &str) -> Option<String> {
    // strip digest
    let no_digest = image.split('@').next().unwrap_or(image);
    // last : after last /
    let name = no_digest.rsplit('/').next().unwrap_or(no_digest);
    if let Some((base, tag)) = name.rsplit_once(':') {
        if !base.is_empty() && !tag.is_empty() && tag != "latest" {
            return Some(tag.to_string());
        }
        if tag == "latest" {
            return Some("latest".into());
        }
    }
    None
}

fn first_env_value(deploy: &Value, name: &str) -> Option<String> {
    let containers = deploy
        .pointer("/spec/template/spec/containers")?
        .as_array()?;
    for c in containers {
        let env = match c.get("env").and_then(|e| e.as_array()) {
            Some(a) => a,
            None => continue,
        };
        for e in env {
            if e.get("name").and_then(|n| n.as_str()) == Some(name) {
                if let Some(v) = e.get("value").and_then(|v| v.as_str()) {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

/// Soft ownership: Argo tracking annotation/label, Helm managed-by, etc.
pub fn detect_deploy_owner(deploy: &Value) -> Option<String> {
    let ann = deploy.get("metadata").and_then(|m| m.get("annotations"));
    let labels = deploy.get("metadata").and_then(|m| m.get("labels"));

    if ann
        .and_then(|a| a.get("argocd.argoproj.io/tracking-id"))
        .is_some()
        || labels
            .and_then(|l| l.get("argocd.argoproj.io/instance"))
            .is_some()
    {
        return Some("argocd".into());
    }
    // instance label alone is ambiguous; only claim argo if tracking-id style present
    if let Some(mb) = labels
        .and_then(|l| l.get("app.kubernetes.io/managed-by"))
        .and_then(|v| v.as_str())
    {
        let lower = mb.to_ascii_lowercase();
        if lower.contains("argo") {
            return Some("argocd".into());
        }
        if lower.contains("helm") {
            return Some("helm".into());
        }
    }
    None
}

/// Parse `kubectl get deploy -o json` list or single object into name → Value map.
pub fn index_deployments(json: &Value) -> Vec<(String, Value)> {
    if json.get("kind").and_then(|k| k.as_str()) == Some("DeploymentList") {
        let items = json
            .get("items")
            .and_then(|i| i.as_array())
            .cloned()
            .unwrap_or_default();
        return items
            .into_iter()
            .filter_map(|item| {
                let name = item
                    .pointer("/metadata/name")?
                    .as_str()?
                    .to_string();
                Some((name, item))
            })
            .collect();
    }
    if json.get("kind").and_then(|k| k.as_str()) == Some("Deployment") {
        if let Some(name) = json.pointer("/metadata/name").and_then(|n| n.as_str()) {
            return vec![(name.to_string(), json.clone())];
        }
    }
    // items array without kind
    if let Some(items) = json.get("items").and_then(|i| i.as_array()) {
        return items
            .iter()
            .filter_map(|item| {
                let name = item.pointer("/metadata/name")?.as_str()?.to_string();
                Some((name, item.clone()))
            })
            .collect();
    }
    vec![]
}

/// Probe all probeable projects using live `kubectl` (blocking — call off UI thread).
pub fn probe_deployed_versions(projects: &[ProjectRow], cfg: &KubeConfig) -> ProbeBatch {
    if !kube_probe_enabled(cfg) {
        return ProbeBatch {
            probes: vec![],
            error: Some(
                "kube probe disabled — set [kube] enabled=true and namespaces in config".into(),
            ),
        };
    }

    let source = VersionSource::parse(&cfg.version_source);
    let kubectl = if cfg.command.is_empty() {
        "kubectl"
    } else {
        cfg.command.as_str()
    };

    // Cache: namespace → deployments by name
    let mut ns_cache: Vec<(String, Result<Vec<(String, Value)>, String>)> = Vec::new();

    for ns in &cfg.namespaces {
        let result = kubectl_get_deployments(kubectl, &cfg.context, ns);
        ns_cache.push((ns.clone(), result));
    }

    if ns_cache.iter().all(|(_, r)| r.is_err()) {
        let err = ns_cache
            .first()
            .and_then(|(_, r)| r.as_ref().err().cloned())
            .unwrap_or_else(|| "kubectl failed".into());
        return ProbeBatch {
            probes: vec![],
            error: Some(err),
        };
    }

    let mut probes = Vec::new();
    for p in projects {
        if !should_probe(p) {
            continue;
        }
        let deploy_name = default_deployment_name(p);
        let mut found: Option<DeployedProbe> = None;

        for (ns, result) in &ns_cache {
            let Ok(list) = result else {
                continue;
            };
            if let Some((name, obj)) = find_deployment_for_project(list, &deploy_name) {
                let (ver, owner) = extract_from_deployment(obj, &source);
                let error = if ver.is_none() {
                    Some(format!(
                        "Deployment/{name} found but no version via {}",
                        cfg.version_source
                    ))
                } else {
                    None
                };
                found = Some(DeployedProbe {
                    project_path: p.path.clone(),
                    project_name: p.name.clone(),
                    deployment: name,
                    namespace: ns.clone(),
                    deployed_version: ver,
                    deploy_owner: owner,
                    error,
                });
                break;
            }
        }

        probes.push(found.unwrap_or_else(|| {
            let ns_list = cfg.namespaces.join(",");
            DeployedProbe {
                project_path: p.path.clone(),
                project_name: p.name.clone(),
                deployment: deploy_name.clone(),
                namespace: cfg.namespaces.first().cloned().unwrap_or_default(),
                deployed_version: None,
                deploy_owner: None,
                error: Some(format!(
                    "no Deployment matching '{deploy_name}' in [{ns_list}] \
                     (name / app.kubernetes.io/name label)"
                )),
            }
        }));
    }

    ProbeBatch {
        probes,
        error: None,
    }
}

/// Match Deployment by name / common app labels (Skaffold often uses `app: name`).
fn find_deployment_for_project<'a>(
    list: &'a [(String, Value)],
    deploy_name: &str,
) -> Option<(String, &'a Value)> {
    let want = deploy_name.to_ascii_lowercase();

    // 1) Exact metadata.name
    if let Some((n, obj)) = list.iter().find(|(n, _)| n.eq_ignore_ascii_case(deploy_name)) {
        return Some((n.clone(), obj));
    }

    // 2) Common app labels
    for (n, obj) in list {
        for key in [
            "app.kubernetes.io/name",
            "app.kubernetes.io/instance",
            "app",
            "app.kubernetes.io/component",
        ] {
            if let Some(label) = label_get(obj, key) {
                if label.eq_ignore_ascii_case(deploy_name) {
                    return Some((n.clone(), obj));
                }
            }
        }
    }

    // 3) metadata.name ends with leaf (e.g. payments-api when looking for api is too
    //    loose — require separator or exact end with full leaf).
    list.iter()
        .find(|(n, _)| {
            let nl = n.to_ascii_lowercase();
            nl == want
                || nl.ends_with(&format!("-{want}"))
                || nl.ends_with(&format!("_{want}"))
                || nl.ends_with(&format!("/{want}"))
        })
        .map(|(n, obj)| (n.clone(), obj))
}

fn kubectl_get_deployments(
    kubectl: &str,
    context: &str,
    namespace: &str,
) -> Result<Vec<(String, Value)>, String> {
    let mut cmd = Command::new(kubectl);
    if !context.is_empty() {
        cmd.arg("--context").arg(context);
    }
    cmd.arg("-n")
        .arg(namespace)
        .arg("get")
        .arg("deploy")
        .arg("-o")
        .arg("json");

    let output = cmd
        .output()
        .map_err(|e| format!("spawn {kubectl}: {e} (is kubectl on PATH?)"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "kubectl get deploy -n {namespace} failed: {}",
            stderr.trim()
        ));
    }

    let json: Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("parse kubectl json: {e}"))?;
    Ok(index_deployments(&json))
}

/// Apply probe results onto project rows (matched by path).
pub fn apply_probes(projects: &mut [ProjectRow], batch: &ProbeBatch) {
    // Clear previous probe data for probeable rows
    for p in projects.iter_mut() {
        if should_probe(p) {
            p.deployed_version = None;
            p.deploy_owner = None;
            p.drift = Drift::Unknown; // will set per-probe below; default = not found
        } else {
            p.deployed_version = None;
            p.deploy_owner = None;
            p.drift = Drift::NotApplicable;
        }
    }

    for probe in &batch.probes {
        let Some(row) = projects.iter_mut().find(|p| p.path == probe.project_path) else {
            continue;
        };
        row.deployed_version = probe.deployed_version.clone();
        row.deploy_owner = probe.deploy_owner.clone();
        row.drift = compute_drift(
            &row.version,
            probe.deployed_version.as_deref(),
            true,
        );
    }
}

/// Clear deployed columns (e.g. after rescan before re-probe).
pub fn clear_deployed(projects: &mut [ProjectRow]) {
    for p in projects.iter_mut() {
        p.deployed_version = None;
        p.deploy_owner = None;
        p.drift = if should_probe(p) {
            Drift::NotProbed
        } else {
            Drift::NotApplicable
        };
    }
}

/// Exact inventory name/id to add when excluding from the project browser.
/// Always the full display name so nested rows are not over-matched.
pub fn exclude_pattern_for(project: &ProjectRow) -> String {
    project.name.clone()
}

#[allow(dead_code)]
pub fn _path_exists_for_tests(p: &Path) -> bool {
    p.exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_version_source() {
        assert_eq!(VersionSource::parse("auto"), VersionSource::Auto);
        assert_eq!(VersionSource::parse(""), VersionSource::Auto);
        assert_eq!(
            VersionSource::parse("label:app.kubernetes.io/version"),
            VersionSource::Label("app.kubernetes.io/version".into())
        );
        assert_eq!(VersionSource::parse("image_tag"), VersionSource::ImageTag);
        assert_eq!(
            VersionSource::parse("env:APP_VERSION"),
            VersionSource::Env("APP_VERSION".into())
        );
        assert_eq!(
            VersionSource::parse("annotation:tako/version"),
            VersionSource::Annotation("tako/version".into())
        );
    }

    #[test]
    fn skaffold_style_deploy_shows_live_without_version_label() {
        // Matches animals/cat: name=cat, labels app=cat, image tag is content hash.
        let deploy = json!({
            "kind": "Deployment",
            "metadata": {
                "name": "cat",
                "labels": { "app": "cat" }
            },
            "spec": {
                "template": {
                    "spec": {
                        "containers": [{
                            "name": "cat",
                            "image": "cat:9a1f51b6d4786a5677afb0b60f3a5402163ee663e996df740fe313a742fbd8ac"
                        }]
                    }
                }
            }
        });
        let list = vec![("cat".into(), deploy.clone())];
        let found = find_deployment_for_project(&list, "cat");
        assert!(found.is_some(), "should match by name/app label");
        let (ver, _) = extract_from_deployment(&deploy, &VersionSource::Auto);
        assert_eq!(ver.as_deref(), Some("live"), "hash image tag → live not miss");
        assert!(!looks_like_product_version(
            "9a1f51b6d4786a5677afb0b60f3a5402163ee663e996df740fe313a742fbd8ac"
        ));
        assert!(looks_like_product_version("0.1.0"));
    }

    #[test]
    fn image_tag_parses() {
        assert_eq!(
            image_tag("ghcr.io/acme/payments-api:3.2.0"),
            Some("3.2.0".into())
        );
        assert_eq!(image_tag("payments-api:latest"), Some("latest".into()));
        assert!(image_tag("payments-api@sha256:abc").is_none());
    }

    #[test]
    fn extract_label_version_and_argocd() {
        let deploy = json!({
            "kind": "Deployment",
            "metadata": {
                "name": "payments-api",
                "labels": {
                    "app.kubernetes.io/version": "3.1.0",
                    "app.kubernetes.io/name": "payments-api"
                },
                "annotations": {
                    "argocd.argoproj.io/tracking-id": "payments-api:apps/Deployment:dev/payments-api"
                }
            },
            "spec": {
                "template": {
                    "spec": {
                        "containers": [{
                            "name": "app",
                            "image": "ghcr.io/acme/payments-api:3.1.0"
                        }]
                    }
                }
            }
        });
        let (ver, owner) = extract_from_deployment(
            &deploy,
            &VersionSource::Label("app.kubernetes.io/version".into()),
        );
        assert_eq!(ver.as_deref(), Some("3.1.0"));
        assert_eq!(owner.as_deref(), Some("argocd"));

        let (tag, _) = extract_from_deployment(&deploy, &VersionSource::ImageTag);
        assert_eq!(tag.as_deref(), Some("3.1.0"));
    }

    #[test]
    fn index_deployment_list() {
        let list = json!({
            "kind": "DeploymentList",
            "items": [
                { "kind": "Deployment", "metadata": { "name": "a" } },
                { "kind": "Deployment", "metadata": { "name": "b" } }
            ]
        });
        let idx = index_deployments(&list);
        assert_eq!(idx.len(), 2);
        assert_eq!(idx[0].0, "a");
    }

    #[test]
    fn drift_compare() {
        assert_eq!(compute_drift("1.0.0", Some("1.0.0"), true), Drift::Match);
        assert_eq!(
            compute_drift("1.2.0", Some("1.1.0"), true),
            Drift::LocalAhead
        );
        assert_eq!(
            compute_drift("1.0.0", Some("1.1.0"), true),
            Drift::ClusterAhead
        );
        assert_eq!(compute_drift("1.0.0", None, true), Drift::Unknown);
        assert_eq!(compute_drift("1.0.0", None, false), Drift::NotProbed);
    }

    #[test]
    fn apply_probes_sets_rows() {
        let mut projects = vec![
            ProjectRow::new("payments-api", "/ws/payments-api", ProjectKind::Service, "3.2.0", "main"),
            ProjectRow::new("common-lib", "/ws/common-lib", ProjectKind::Library, "1.0.0", "main"),
        ];
        projects[0].has_skaffold = true;
        projects[1].has_skaffold = false;

        let batch = ProbeBatch {
            probes: vec![DeployedProbe {
                project_path: projects[0].path.clone(),
                project_name: "payments-api".into(),
                deployment: "payments-api".into(),
                namespace: "dev".into(),
                deployed_version: Some("3.1.0".into()),
                deploy_owner: Some("argocd".into()),
                error: None,
            }],
            error: None,
        };
        apply_probes(&mut projects, &batch);
        assert_eq!(projects[0].deployed_version.as_deref(), Some("3.1.0"));
        assert_eq!(projects[0].drift, Drift::LocalAhead);
        assert_eq!(projects[0].deploy_owner.as_deref(), Some("argocd"));
        assert_eq!(projects[1].drift, Drift::NotApplicable);
    }
}
