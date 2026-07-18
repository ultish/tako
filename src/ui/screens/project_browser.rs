//! Project browser — inventory list + full-screen project detail.
//!
//! Graph highlights (list): dependencies = cyan (`secondary`), dependents =
//! `warning`. Enter opens a full-screen detail page (scrollable deps + build/
//! deploy actions). Esc closes.

use std::collections::HashSet;

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{is_favorite, project_display_order_filtered, App, Drift, Screen};
use crate::deploy;
use crate::ui::widgets::footer::{
    render_keybind_footer, render_status_bar, split_with_footer,
};
use crate::ui::widgets::table_nav::{
    render_selectable_list_with_highlights, RowHighlights,
};

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    match app.screen {
        Screen::ProjectBrowser => {
            if app.project_detail_visible {
                render_project_detail_full(frame, app, area);
            } else {
                render_projects(frame, app, area);
            }
        }
        Screen::Jobs => crate::ui::screens::jobs::render(frame, app, area),
        Screen::Workspace => crate::ui::screens::workspace::render(frame, app, area),
        Screen::Settings => crate::ui::screens::settings::render(frame, app, area),
    }
}

fn render_projects(frame: &mut Frame, app: &App, area: Rect) {
    let (main, status_bar, footer) = split_with_footer(area);

    if app.projects.is_empty() {
        render_empty_projects(frame, app, main);
        render_status_bar(frame, app, status_bar);
        render_keybind_footer(
            frame,
            footer,
            &app.theme,
            "r: rescan   3: workspace   ?: help   q: quit",
        );
        return;
    }

    let n_deps = app.dep_indices.len();
    let n_dependents = app.dependent_indices.len();
    let n_multi = app.multi_selected.len();
    let n_drift = app
        .projects
        .iter()
        .filter(|p| matches!(p.drift, Drift::LocalAhead | Drift::ClusterAhead))
        .count();
    let filter_q = app.project_filter.as_deref();
    let title = if let Some(q) = filter_q {
        format!("Projects — filter: /{q}_")
    } else if app.filter_drift_only {
        format!(
            "Projects — drift only ({n_drift}) · needs:{n_deps} needed-by:{n_dependents}"
        )
    } else if n_multi > 0 {
        format!(
            "Projects — {n_multi} selected · needs:{n_deps} needed-by:{n_dependents}"
        )
    } else if n_deps + n_dependents > 0 {
        format!("Projects — needs:{n_deps} needed-by:{n_dependents}")
    } else if n_drift > 0 {
        format!("Projects — {n_drift} with drift")
    } else {
        "Projects".to_string()
    };

    let order = project_list_order(app);
    let display_selected = order
        .iter()
        .position(|&i| i == app.selected_index)
        .unwrap_or(0);

    let items: Vec<Vec<String>> = order
        .iter()
        .map(|&i| {
            let p = &app.projects[i];
            let fav = if is_favorite(p, &app.config.ui.favorites) {
                "★"
            } else {
                " "
            };
            let mark = if app.multi_selected.contains(&i) {
                "●" // multi-selected (Space / m)
            } else {
                " "
            };
            let deployed = match p.deployed_version.as_deref() {
                Some(v) => v.to_string(),
                None => match p.drift {
                    Drift::NotApplicable | Drift::NotProbed => "—".into(),
                    Drift::Unknown => "miss".into(),
                    _ => "?".into(),
                },
            };
            let git = if p.git_sync.is_empty() {
                if p.git_dirty {
                    "dirty"
                } else {
                    "—"
                }
            } else {
                p.git_sync.as_str()
            };
            let nexus = if p.nexus.is_empty() { "—" } else { p.nexus.as_str() };
            let branch = if p.branch.is_empty() {
                "—"
            } else {
                p.branch.as_str()
            };
            vec![
                format!("{fav}{mark}{}", p.display_name()),
                p.kind.label().to_string(),
                p.version.clone(),
                nexus.to_string(),
                git.to_string(),
                branch.to_string(),
                deployed,
                p.drift.label().to_string(),
                p.status.clone(),
            ]
        })
        .collect();

    let highlights = project_highlights_for_display(app, &order);

    let n_unprobed = app
        .projects
        .iter()
        .filter(|p| p.drift == Drift::NotProbed)
        .count();
    let n_git_behind = app
        .projects
        .iter()
        .filter(|p| p.git_sync.contains('↓'))
        .count();
    let n_nexus_newer = app.projects.iter().filter(|p| p.nexus == "newer").count();
    let title = {
        let mut t = title;
        if n_unprobed > 0 && app.kube_probed_at.is_none() && filter_q.is_none() {
            t = if t == "Projects" {
                "Projects — K: probe cluster".to_string()
            } else {
                format!("{t} · K: probe")
            };
        }
        if n_git_behind > 0 || n_nexus_newer > 0 {
            t = format!("{t} · r:stats (git↓{n_git_behind} nexus↑{n_nexus_newer})");
        } else if filter_q.is_none() {
            t = format!("{t} · r: git+nexus");
        }
        t
    };

    render_selectable_list_with_highlights(
        frame,
        app,
        main,
        &title,
        &items,
        Some(&[
            "Name",
            "Kind",
            "Local",
            "Nexus",
            "Git",
            "Branch",
            "Deployed",
            "Drift",
            "Status",
        ]),
        display_selected,
        true,
        Some(&highlights),
    );
    remap_project_row_clicks(app, main, &order, display_selected, items.is_empty());

    let footer_text = if app.project_filter.is_some() {
        "type to filter   j/k move   Enter/Esc: clear filter".to_string()
    } else if n_multi > 0 {
        format!(
            "MULTI {n_multi}  Space/m: toggle  b/B c G: bulk  Esc: clear multi  -:hide  ?:help"
        )
    } else {
        "j/k  Space/m multi  b/B c G  U  u  v/V  r:git+nexus  w:scan  ?:flows"
            .to_string()
    };
    render_status_bar(frame, app, status_bar);
    render_keybind_footer(frame, footer, &app.theme, &footer_text);
}

fn project_list_order(app: &App) -> Vec<usize> {
    project_display_order_filtered(
        &app.projects,
        &app.config.ui.favorites,
        app.filter_drift_only,
        app.project_filter.as_deref(),
    )
}

/// Full-screen project detail: identity + split panels
/// (left: all deps/produces, right: workspace graph) + actions.
fn render_project_detail_full(frame: &mut Frame, app: &App, area: Rect) {
    let (main, status_bar, footer) = split_with_footer(area);

    let Some(project) = app.projects.get(app.selected_index) else {
        render_status_bar(frame, app, status_bar);
        render_keybind_footer(frame, footer, &app.theme, "Esc: back");
        return;
    };
    let id = app
        .graph
        .id_at(app.selected_index)
        .unwrap_or(project.name.as_str());
    let produces = app.graph.produces_of_id(id);
    let depends = app.graph.depends_of_id(id);
    let dep_projects = app.graph.dependencies_of(id);
    let dependent_projects = app.graph.dependents_of(id);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4), // header
            Constraint::Min(8),    // split panels
        ])
        .split(main);

    // ── Header ──────────────────────────────────────────────────────────
    let sk = if project.has_skaffold {
        project
            .skaffold_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "yes".into())
    } else {
        "—".into()
    };
    let n_ws = depends
        .iter()
        .filter(|d| dep_workspace_link(app, d).is_some())
        .count();
    let deployed = project
        .deployed_version
        .as_deref()
        .map(|s| s.to_string())
        .unwrap_or_else(|| match project.drift {
            Drift::NotApplicable | Drift::NotProbed => "—".into(),
            Drift::Unknown => "miss".into(),
            _ => "?".into(),
        });
    let owner = project
        .deploy_owner
        .as_ref()
        .map(|o| format!(" ({o})"))
        .unwrap_or_default();
    let header_lines = vec![
        Line::from(vec![
            Span::styled(
                format!("{}  ", project.display_name()),
                app.theme.accent.add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{}  local:{}", project.kind.label(), project.version),
                app.theme.secondary,
            ),
            Span::styled(
                format!("  deployed:{deployed}{}  {}", owner, project.drift.label()),
                app.theme.dim,
            ),
            Span::styled(
                format!(
                    "  ·  {}{}",
                    project.branch,
                    if project.git_dirty { " ✗" } else { "" }
                ),
                app.theme.dim,
            ),
        ]),
        Line::from(vec![
            Span::styled(
                format!("path: {}  ·  ", project.path.display()),
                app.theme.dim,
            ),
            Span::styled(
                format!(
                    "{} deps ({} in workspace)  ·  {} produces  ·  skaffold: {}  ·  {}",
                    depends.len(),
                    n_ws,
                    produces.len(),
                    sk,
                    project.status
                ),
                app.theme.dim,
            ),
        ]),
    ];
    frame.render_widget(
        Paragraph::new(header_lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Project · {id} "))
                .title_style(app.theme.title)
                .border_style(app.theme.border)
                .style(app.theme.root_style()),
        ),
        chunks[0],
    );

    // ── Split: left = all dependencies, right = workspace graph ─────────
    let panels = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(chunks[1]);

    render_detail_deps_panel(frame, app, panels[0], produces, depends);
    if app.impact_visible {
        render_impact_checklist(frame, app, panels[1], &dependent_projects, project);
    } else {
        render_detail_workspace_graph(
            frame,
            app,
            panels[1],
            &dep_projects,
            &dependent_projects,
        );
    }

    let mode = deploy::effective_deploy_mode(project, &app.config).label();
    let footer_text = if project.has_skaffold {
        format!(
            "U:update-deps  u:delete→run  x:delete  B:force-build  v/V bump  p:publish  r:stats  deploy={mode}  Esc"
        )
    } else {
        format!(
            "U:update-deps  B:force-build  v/V bump  p:publish  r:stats  i:who-needs  deploy={mode}  Esc"
        )
    };
    render_status_bar(frame, app, status_bar);
    render_keybind_footer(frame, footer, &app.theme, &footer_text);
}

/// Impact checklist: what to do for each dependent after changing `source`.
fn render_impact_checklist(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    dependent_ids: &[String],
    source: &crate::app::ProjectRow,
) {
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(
        format!("Who needs {}?", source.name),
        app.theme.warning.add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        "U update dependents  ·  V bump their versions  ·  p publish this lib first if needed",
        app.theme.dim,
    )));
    lines.push(Line::from(""));

    if dependent_ids.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (nothing in the workspace depends on this)",
            app.theme.dim,
        )));
    } else {
        lines.push(Line::from(Span::styled(
            format!(
                "  {:<22} {:<10} {:<8} {:<8} {}",
                "PROJECT", "VER", "DEPLOY", "DRIFT", "SUGGESTED"
            ),
            app.theme.dim.add_modifier(Modifier::UNDERLINED),
        )));
        for id in dependent_ids {
            let Some(idx) = app.graph.index_of(id) else {
                continue;
            };
            let Some(p) = app.projects.get(idx) else {
                continue;
            };
            let mode = deploy::effective_deploy_mode(p, &app.config);
            let next = match mode {
                crate::config::DeployMode::Skaffold | crate::config::DeployMode::Auto
                    if p.has_skaffold && p.deploy_owner.as_deref() != Some("argocd") =>
                {
                    "rebuild + skaffold"
                }
                crate::config::DeployMode::Argocd => "rebuild only (Argo owns deploy)",
                _ => "rebuild",
            };
            let drift = p.drift.label();
            let name = if p.name.len() > 22 {
                format!("{}…", &p.name[..21])
            } else {
                p.name.clone()
            };
            lines.push(Line::from(Span::styled(
                format!(
                    "  {name:<22} {:<10} {:<8} {drift:<8} {next}",
                    p.version,
                    mode.label(),
                ),
                app.theme.text,
            )));
        }
    }

    if deploy::needs_version_label_hint(source) {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "tip: set app.kubernetes.io/version so Local vs Deployed can show real drift",
            app.theme.dim,
        )));
    }

    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Who needs this? (i) ")
                    .title_style(app.theme.title)
                    .border_style(app.theme.warning)
                    .style(app.theme.root_style()),
            ),
        area,
    );
}

/// If this dependency maps to a project in the workspace, return its graph id.
fn dep_workspace_link(app: &App, dep: &crate::gradle::model::DepReq) -> Option<String> {
    if let Some(path) = dep.project_path() {
        let norm = path.trim_start_matches(':');
        for i in 0..app.projects.len() {
            let Some(id) = app.graph.id_at(i) else {
                continue;
            };
            let name = &app.projects[i].name;
            if id == norm
                || id.ends_with(&format!("/{norm}"))
                || name == norm
                || name.ends_with(&format!("/{norm}"))
            {
                return Some(id.to_string());
            }
        }
        return None;
    }
    let coord = dep.coordinate.as_ref()?;
    app.graph.producers_matching_coord(coord).into_iter().next()
}

fn render_detail_deps_panel(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    produces: &[crate::gradle::model::Produces],
    depends: &[crate::gradle::model::DepReq],
) {
    let mut body_lines: Vec<Line> = Vec::new();

    body_lines.push(Line::from(Span::styled(
        format!("PUBLISHES ({})", produces.len()),
        app.theme.secondary.add_modifier(Modifier::BOLD),
    )));
    body_lines.push(Line::from(Span::styled(
        "  Artifacts this project can publish (Maven coordinates)",
        app.theme.dim,
    )));
    if produces.is_empty() {
        body_lines.push(Line::from(Span::styled("  (none)", app.theme.dim)));
    } else {
        for p in produces {
            body_lines.push(Line::from(Span::styled(
                format!("  · {}", p.display()),
                app.theme.text,
            )));
        }
    }

    body_lines.push(Line::from(""));
    let n_ws = depends
        .iter()
        .filter(|d| dep_workspace_link(app, d).is_some())
        .count();
    body_lines.push(Line::from(Span::styled(
        format!("DEPENDENCIES ({})  ·  {n_ws} also in this workspace", depends.len()),
        app.theme.secondary.add_modifier(Modifier::BOLD),
    )));
    body_lines.push(Line::from(Span::styled(
        "  ★ = another project under your scan roots (not just Maven Central)",
        app.theme.dim,
    )));
    body_lines.push(Line::from(""));

    if depends.is_empty() {
        body_lines.push(Line::from(Span::styled(
            "  (none listed — or versions not resolved from catalog)",
            app.theme.dim,
        )));
    } else {
        // Workspace hits first, then third-party — easier to scan.
        let mut indexed: Vec<(usize, &crate::gradle::model::DepReq)> =
            depends.iter().enumerate().collect();
        indexed.sort_by_key(|(_, d)| {
            if dep_workspace_link(app, d).is_some() {
                0
            } else {
                1
            }
        });

        for (_, d) in indexed {
            let ws = dep_workspace_link(app, d);
            let mark = if ws.is_some() { "★" } else { "·" };
            let mut spans = vec![
                Span::styled(
                    format!("  {mark} "),
                    if ws.is_some() {
                        app.theme.success.add_modifier(Modifier::BOLD)
                    } else {
                        app.theme.dim
                    },
                ),
                Span::styled(
                    d.display(),
                    if ws.is_some() {
                        app.theme.text.add_modifier(Modifier::BOLD)
                    } else {
                        app.theme.text
                    },
                ),
                Span::styled(format!("  [{}]", d.configuration), app.theme.dim),
            ];
            if let Some(ws_id) = ws {
                // Show linked project name + version when we know it.
                let ver = app
                    .graph
                    .index_of_id(&ws_id)
                    .and_then(|i| app.projects.get(i))
                    .map(|p| format!(" v{}", p.version))
                    .unwrap_or_default();
                spans.push(Span::styled(
                    format!("  → {ws_id}{ver}"),
                    app.theme.success,
                ));
            }
            body_lines.push(Line::from(spans));
        }
    }

    let body_h = area.height.saturating_sub(2).max(1) as usize;
    let total_lines = body_lines.len();
    let max_scroll = total_lines.saturating_sub(body_h);
    let scroll = app.project_detail_scroll.min(max_scroll);
    let visible: Vec<Line> = body_lines
        .into_iter()
        .skip(scroll)
        .take(body_h)
        .collect();
    let shown = visible.len();
    let scroll_hint = if max_scroll > 0 {
        format!(" · {}–{}/{total_lines}  j/k ", scroll + 1, scroll + shown)
    } else {
        String::new()
    };

    frame.render_widget(
        Paragraph::new(visible).block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Dependencies{scroll_hint}"))
                .title_style(app.theme.title)
                .border_style(app.theme.border)
                .style(app.theme.root_style()),
        ),
        area,
    );
}

fn render_detail_workspace_graph(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    dep_projects: &[String],
    dependent_projects: &[String],
) {
    let mut lines: Vec<Line> = Vec::new();

    lines.push(Line::from(Span::styled(
        "RELATED PROJECTS",
        app.theme.secondary.add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        "Other scan-root projects linked by Gradle dependencies.",
        app.theme.dim,
    )));
    lines.push(Line::from(""));

    // Projects this one depends on
    lines.push(Line::from(vec![
        Span::styled(
            format!("This needs ({}) ", dep_projects.len()),
            app.theme.secondary.add_modifier(Modifier::BOLD),
        ),
        Span::styled("— libraries / services it depends on", app.theme.dim),
    ]));
    if dep_projects.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (none in workspace — only third-party jars, or unresolved)",
            app.theme.dim,
        )));
    } else {
        for dep_id in dep_projects {
            let row = app
                .graph
                .index_of_id(dep_id)
                .and_then(|i| app.projects.get(i));
            let kind = row.map(|p| p.kind.label()).unwrap_or("?");
            let ver = row.map(|p| p.version.as_str()).unwrap_or("—");
            let status = row.map(|p| p.status.as_str()).unwrap_or("");
            let sk = row
                .map(|p| if p.has_skaffold { " · skaffold" } else { "" })
                .unwrap_or("");
            lines.push(Line::from(vec![
                Span::styled("  · ", app.theme.success.add_modifier(Modifier::BOLD)),
                Span::styled(dep_id.clone(), app.theme.secondary.add_modifier(Modifier::BOLD)),
                Span::styled(format!("  {kind}  v{ver}{sk}"), app.theme.dim),
                if status.is_empty() || status == "idle" {
                    Span::raw("")
                } else {
                    Span::styled(format!("  [{status}]"), app.theme.dim)
                },
            ]));
            if let Some(p) = row {
                lines.push(Line::from(Span::styled(
                    format!("      {}", p.path.display()),
                    app.theme.dim,
                )));
            }
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(
            format!("Needed by ({}) ", dependent_projects.len()),
            app.theme.warning.add_modifier(Modifier::BOLD),
        ),
        Span::styled("— projects that depend on this one", app.theme.dim),
    ]));
    if dependent_projects.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (none — nothing in the workspace depends on this)",
            app.theme.dim,
        )));
    } else {
        for dep_id in dependent_projects {
            let row = app
                .graph
                .index_of_id(dep_id)
                .and_then(|i| app.projects.get(i));
            let kind = row.map(|p| p.kind.label()).unwrap_or("?");
            let ver = row.map(|p| p.version.as_str()).unwrap_or("—");
            let sk = row
                .map(|p| if p.has_skaffold { " · skaffold" } else { "" })
                .unwrap_or("");
            lines.push(Line::from(vec![
                Span::styled("  · ", app.theme.warning.add_modifier(Modifier::BOLD)),
                Span::styled(dep_id.clone(), app.theme.warning.add_modifier(Modifier::BOLD)),
                Span::styled(format!("  {kind}  v{ver}{sk}"), app.theme.dim),
            ]));
            if let Some(p) = row {
                lines.push(Line::from(Span::styled(
                    format!("      {}", p.path.display()),
                    app.theme.dim,
                )));
            }
        }
    }

    // Full transitive dependent tree (from cursor project)
    if let Some(tree_root) = app.graph.id_at(app.selected_index) {
        let tree = app.graph.dependents_tree(tree_root);
        if !tree.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Full dependent tree (U builds in topo order):",
                app.theme.dim.add_modifier(Modifier::BOLD),
            )));
            for (depth, id) in tree.iter().take(40) {
                let indent = "  ".repeat(*depth + 1);
                let row = app
                    .graph
                    .index_of(id)
                    .and_then(|i| app.projects.get(i));
                let sk = row
                    .map(|p| if p.has_skaffold { " · skaffold" } else { "" })
                    .unwrap_or("");
                let kind = row.map(|p| p.kind.label()).unwrap_or("?");
                lines.push(Line::from(Span::styled(
                    format!("{indent}· {id}  ({kind}{sk})"),
                    if *depth == 0 {
                        app.theme.warning
                    } else {
                        app.theme.dim
                    },
                )));
            }
            if tree.len() > 40 {
                lines.push(Line::from(Span::styled(
                    format!("  … +{} more", tree.len() - 40),
                    app.theme.dim,
                )));
            }
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Typical keys:",
        app.theme.dim.add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        "  U  update dependents (full tree; services delete→run unless Argo)",
        app.theme.dim,
    )));
    lines.push(Line::from(Span::styled(
        "  V  bump dependent versions (manual)  ·  v  bump this project",
        app.theme.dim,
    )));
    lines.push(Line::from(Span::styled(
        "  p  publish this to Nexus  ·  B  force-build  ·  u  delete→run",
        app.theme.dim,
    )));

    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Related projects ")
                    .title_style(app.theme.title)
                    .border_style(app.theme.border)
                    .style(app.theme.root_style()),
            ),
        area,
    );
}

fn project_highlights_for_display(app: &App, order: &[usize]) -> RowHighlights {
    let mut secondary = HashSet::new();
    let mut warning = HashSet::new();
    for (display_idx, &project_idx) in order.iter().enumerate() {
        if app.dep_indices.contains(&project_idx) {
            secondary.insert(display_idx);
        }
        if app.dependent_indices.contains(&project_idx) {
            warning.insert(display_idx);
        }
    }
    RowHighlights {
        secondary,
        warning,
    }
}

fn render_empty_projects(frame: &mut Frame, app: &App, area: Rect) {
    let n_roots = app.config.scan.roots.len();
    let config_path = app.config_path.display();
    let default_body = if n_roots == 0 {
        format!(
            "No projects discovered yet.\n\n\
             No scan roots configured.\n\
             Open Workspace (3) and press n to add a root,\n\
             or edit [scan].roots in:\n  {config_path}\n\n\
             Template: config.example.toml in the tako repo.\n\
             After adding, tako rescans automatically."
        )
    } else {
        format!(
            "No projects discovered yet.\n\n\
             {n_roots} scan root(s) configured — see Workspace (3).\n\
             Config: {config_path}\n\n\
             Press r to rescan, or edit roots under tab 3 (n/e/z)."
        )
    };
    let message = Paragraph::new(default_body)
        .style(app.theme.status)
        .wrap(Wrap { trim: false })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Projects")
                .title_style(app.theme.title)
                .border_style(app.theme.border)
                .style(app.theme.root_style()),
        );
    frame.render_widget(message, area);
}

fn remap_project_row_clicks(
    app: &App,
    area: Rect,
    order: &[usize],
    display_selected: usize,
    empty: bool,
) {
    if empty || order.is_empty() {
        return;
    }
    use crate::events::Action;
    use crate::ui::widgets::table_nav::{
        selectable_list_offset, selectable_list_viewport_rows,
    };

    let has_header = true;
    let viewport = selectable_list_viewport_rows(area, has_header);
    let offset = selectable_list_offset(display_selected, order.len(), viewport);
    let inner = Block::default().borders(Borders::ALL).inner(area);
    let header_h = 1u16;
    let rows_y = inner.y + header_h;
    let rows_h = inner.height.saturating_sub(header_h);
    let visible = (rows_h as usize).min(order.len().saturating_sub(offset));
    for i in 0..visible {
        let display_idx = offset + i;
        let project_idx = order[display_idx];
        let y = rows_y + i as u16;
        app.register_click(
            inner.x,
            y,
            inner.width,
            1,
            Action::SelectRow(project_idx),
        );
    }
}


