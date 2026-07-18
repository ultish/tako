//! Effective deploy mode: config overrides + skaffold presence + Argo probe labels.

use crate::app::ProjectRow;
use crate::config::{Config, DeployMode};

/// Resolve deploy mode for a project row.
pub fn effective_deploy_mode(project: &ProjectRow, config: &Config) -> DeployMode {
    // Explicit override by name or id-ish name
    for key in [&project.name, project.name.rsplit('/').next().unwrap_or("")] {
        if key.is_empty() {
            continue;
        }
        if let Some(raw) = config.deploy.modes.get(key) {
            if let Some(m) = DeployMode::parse(raw) {
                if m != DeployMode::Auto {
                    return m;
                }
            }
        }
    }

    // Live Argo ownership from last kube probe
    if project.deploy_owner.as_deref() == Some("argocd") {
        return DeployMode::Argocd;
    }

    if project.has_skaffold || project.skaffold_path.is_some() {
        DeployMode::Skaffold
    } else {
        DeployMode::Manual
    }
}

/// True if skaffold deploy actions are allowed without an Argo confirm.
pub fn skaffold_allowed(project: &ProjectRow, config: &Config) -> bool {
    match effective_deploy_mode(project, config) {
        DeployMode::Skaffold | DeployMode::Auto => {
            // Auto already collapsed; still check guard
            if project.deploy_owner.as_deref() == Some("argocd") && config.deploy.argo_guard {
                return false;
            }
            project.has_skaffold || project.skaffold_path.is_some()
        }
        DeployMode::Argocd => !config.deploy.argo_guard,
        DeployMode::Manual => false,
    }
}

/// True if we should prompt before skaffold (Argo-owned + guard).
pub fn skaffold_needs_argo_confirm(project: &ProjectRow, config: &Config) -> bool {
    config.deploy.argo_guard
        && (project.deploy_owner.as_deref() == Some("argocd")
            || effective_deploy_mode(project, config) == DeployMode::Argocd)
        && (project.has_skaffold || project.skaffold_path.is_some())
}

/// True if cascade/lib-update should include skaffold steps for this consumer.
pub fn cascade_skaffold_ok(project: &ProjectRow, config: &Config) -> bool {
    match effective_deploy_mode(project, config) {
        DeployMode::Skaffold => true,
        DeployMode::Auto => {
            project.has_skaffold
                && project.deploy_owner.as_deref() != Some("argocd")
        }
        DeployMode::Argocd | DeployMode::Manual => false,
    }
}

/// Hint when Skaffold workload is live but has no product version label.
pub fn needs_version_label_hint(project: &ProjectRow) -> bool {
    project.has_skaffold && project.deployed_version.as_deref() == Some("live")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Drift, ProjectRow};
    use crate::config::{Config, DeployConfig, DeployMode};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn row(name: &str, has_skaffold: bool, owner: Option<&str>) -> ProjectRow {
        ProjectRow {
            name: name.into(),
            kind: crate::app::ProjectKind::Service,
            version: "1.0.0".into(),
            branch: "main".into(),
            has_skaffold,
            git_dirty: false,
            status: "idle".into(),
            folder_group: None,
            path: PathBuf::from(format!("/ws/{name}")),
            git_root: None,
            skaffold_path: has_skaffold
                .then(|| PathBuf::from(format!("/ws/{name}/skaffold.yaml"))),
            gradle_root: None,
            deployed_version: None,
            drift: Drift::NotProbed,
            deploy_owner: owner.map(str::to_string),
            git_sync: "—".into(),
            nexus: "—".into(),
        }
    }

    #[test]
    fn override_mode_wins() {
        let mut modes = BTreeMap::new();
        modes.insert("payments-api".into(), "argocd".into());
        let config = Config {
            deploy: DeployConfig {
                argo_guard: true,
                modes,
            },
            ..Default::default()
        };
        let p = row("payments-api", true, None);
        assert_eq!(effective_deploy_mode(&p, &config), DeployMode::Argocd);
        assert!(!cascade_skaffold_ok(&p, &config));
    }

    #[test]
    fn argo_owner_needs_confirm_when_guard_on() {
        let config = Config::default();
        let p = row("svc", true, Some("argocd"));
        assert!(skaffold_needs_argo_confirm(&p, &config));
        assert!(!cascade_skaffold_ok(&p, &config));
    }

    #[test]
    fn skaffold_service_ok_for_cascade() {
        let config = Config::default();
        let p = row("svc", true, None);
        assert_eq!(effective_deploy_mode(&p, &config), DeployMode::Skaffold);
        assert!(cascade_skaffold_ok(&p, &config));
        assert!(!skaffold_needs_argo_confirm(&p, &config));
    }

    #[test]
    fn live_without_version_label_hint() {
        let mut p = row("svc", true, None);
        p.deployed_version = Some("live".into());
        assert!(needs_version_label_hint(&p));
        p.deployed_version = Some("1.2.3".into());
        assert!(!needs_version_label_hint(&p));
    }
}
