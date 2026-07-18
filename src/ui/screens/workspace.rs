//! Workspace / scan-roots screen.
//!
//! Lists configured roots from `[scan].roots` and project excludes from
//! `[scan].exclude`. **Tab** switches focus; **n**/**e**/**z** act on the
//! focused list. Path/pattern form expands `~/` (roots) and saves to config.toml.

use std::path::Path;

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, ProjectKind, RootEditorState, WorkspacePanel};
use crate::events::Action;
use crate::ui::widgets::confirm_dialog::{centered_rect, render_confirm_dialog};
use crate::ui::widgets::footer::{
    render_keybind_footer, render_status_bar, split_with_footer,
};
use crate::ui::widgets::table_nav::render_selectable_list;

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let (main, status_bar, footer) = split_with_footer(area);

    let roots = &app.config.scan.roots;
    let excludes = &app.config.scan.exclude;
    let config_path = app.config_path.display().to_string();

    let title = "Workspace".to_string();

    // Vertical: summary strip + main body
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(4), Constraint::Min(5)])
        .split(main);

    render_summary_strip(frame, app, outer[0], &config_path);

    if roots.is_empty() && app.root_editor.is_none() {
        render_empty_roots(frame, app, outer[1], &title, &config_path);
    } else if roots.is_empty() {
        // First-run editor open — soft backdrop under dialog.
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("  Adding first scan root…", app.theme.status),
            ]))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(title.as_str())
                    .title_style(app.theme.title)
                    .border_style(app.theme.border)
                    .style(app.theme.root_style()),
            ),
            outer[1],
        );
    } else {
        render_workspace_body(frame, app, outer[1], roots, excludes);
    }

    let n_roots = roots.len();
    let n_excludes = excludes.len();
    let n_projects = app.projects.len();
    let last_scan = format_last_scan(app);
    let focus = match app.workspace_panel {
        WorkspacePanel::Roots => "roots",
        WorkspacePanel::Excludes => "excludes",
    };
    let footer_text = if app.root_editor.is_some() {
        let is_ex = app.root_editor.as_ref().is_some_and(|s| s.is_exclude());
        if is_ex {
            "type pattern   ←/→ Home/End: cursor   Enter: save   Esc: cancel".to_string()
        } else {
            "type path   ←/→ Home/End: cursor   Enter: save   Esc: cancel".to_string()
        }
    } else if app.root_delete_confirm.is_some() || app.exclude_delete_confirm.is_some() {
        "y/Enter: delete   n/Esc: cancel".to_string()
    } else if roots.is_empty() {
        "n: add root   1: projects   ?: help   q: quit".to_string()
    } else {
        format!(
            "Tab: {focus}   n: add   e: edit   z: delete   w: rescan   Enter: projects   ?: help   ·  {n_roots} root(s) · {n_excludes} exclude(s) · {n_projects} project(s){last_scan}"
        )
    };
    render_status_bar(frame, app, status_bar);
    render_keybind_footer(frame, footer, &app.theme, &footer_text);

    if let Some(state) = app.root_editor.as_ref() {
        render_list_editor(frame, app, area, state, roots.is_empty());
    }

    if let Some(index) = app.root_delete_confirm {
        render_delete_confirm(
            frame,
            app,
            area,
            "Delete scan root?",
            app.config.scan.roots.get(index).map(String::as_str),
            "Remove this scan root from the workspace?",
            "Only the config entry is removed — nothing on disk is deleted.\n\
             A rescan runs if any roots remain.",
        );
    }

    if let Some(index) = app.exclude_delete_confirm {
        render_delete_confirm(
            frame,
            app,
            area,
            "Delete exclude?",
            app.config.scan.exclude.get(index).map(String::as_str),
            "Remove this exclude pattern?",
            "Matching projects will reappear on the next rescan.",
        );
    }
}

/// Top strip: config path, scan settings, inventory counts by kind.
fn render_summary_strip(frame: &mut Frame, app: &App, area: Rect, config_path: &str) {
    let mut svc = 0usize;
    let mut lib = 0usize;
    let mut avro = 0usize;
    let mut other = 0usize;
    for p in &app.projects {
        match p.kind {
            ProjectKind::Service => svc += 1,
            ProjectKind::Library => lib += 1,
            ProjectKind::Avro => avro += 1,
            ProjectKind::Unknown => other += 1,
        }
    }

    let depth = app.config.scan.max_depth;
    let scanning = if app.scanning {
        "  ·  scanning…"
    } else {
        ""
    };

    let line1 = Line::from(vec![
        Span::styled(" config ", app.theme.dim),
        Span::styled(config_path, app.theme.secondary),
        Span::styled(scanning, app.theme.warning.add_modifier(Modifier::BOLD)),
    ]);
    let n_exclude = app.config.scan.exclude.len();
    let line2 = Line::from(vec![
        Span::styled(" scan ", app.theme.dim),
        Span::styled(
            format!("max_depth={depth}  ·  "),
            app.theme.dim,
        ),
        Span::styled(format!("{svc} "), app.theme.text.add_modifier(Modifier::BOLD)),
        Span::styled("services  ", app.theme.dim),
        Span::styled(format!("{lib} "), app.theme.text.add_modifier(Modifier::BOLD)),
        Span::styled("libs  ", app.theme.dim),
        Span::styled(format!("{avro} "), app.theme.text.add_modifier(Modifier::BOLD)),
        Span::styled("avro  ", app.theme.dim),
        if other > 0 {
            Span::styled(format!("· {other} other"), app.theme.dim)
        } else {
            Span::raw("")
        },
        if n_exclude > 0 {
            Span::styled(format!("  ·  {n_exclude} exclude"), app.theme.warning)
        } else {
            Span::raw("")
        },
    ]);

    frame.render_widget(
        Paragraph::new(vec![line1, line2]).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Summary ")
                .title_style(app.theme.title)
                .border_style(app.theme.border)
                .style(app.theme.root_style()),
        ),
        area,
    );
}

fn render_empty_roots(frame: &mut Frame, app: &App, area: Rect, title: &str, config_path: &str) {
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  No scan roots yet",
            app.theme.accent.add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Press ", app.theme.text),
            Span::styled("n", app.theme.secondary.add_modifier(Modifier::BOLD)),
            Span::styled(" to add a folder for tako to scan", app.theme.text),
        ]),
        Line::from(Span::styled(
            "  e.g.  ~/Developer   or   ~/Developer/tmp-dev",
            app.theme.dim,
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Paths expand ", app.theme.dim),
            Span::styled("~/", app.theme.secondary),
            Span::styled(" and must be absolute.", app.theme.dim),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            format!("  Config:  {config_path}"),
            app.theme.dim,
        )),
        Line::from(Span::styled(
            "  Advanced: edit [scan].roots in that file if you prefer.",
            app.theme.dim,
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  ", app.theme.dim),
            Span::styled("1", app.theme.secondary.add_modifier(Modifier::BOLD)),
            Span::styled("  project browser   ·   ", app.theme.dim),
            Span::styled("?", app.theme.secondary.add_modifier(Modifier::BOLD)),
            Span::styled("  help", app.theme.dim),
        ]),
    ];

    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {title} "))
                .title_style(app.theme.title)
                .border_style(app.theme.border)
                .style(app.theme.root_style()),
        ),
        area,
    );
}

/// Roots list (top or left) + excludes list + detail for the focused selection.
fn render_workspace_body(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    roots: &[String],
    excludes: &[String],
) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(area);

    let lists = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(cols[0]);

    let roots_focus = app.workspace_panel == WorkspacePanel::Roots;
    let excludes_focus = app.workspace_panel == WorkspacePanel::Excludes;

    // ── Scan roots ──────────────────────────────────────────────────────
    let root_sel = app
        .selected_root_index
        .min(roots.len().saturating_sub(1));
    let root_items: Vec<Vec<String>> = roots
        .iter()
        .enumerate()
        .map(|(i, path)| {
            let exists = Path::new(path).is_dir();
            let n = count_projects_under(app, path);
            vec![
                format!("{}", i + 1),
                path.clone(),
                if exists { "yes" } else { "missing" }.to_string(),
                n.to_string(),
            ]
        })
        .collect();
    let roots_title = if roots_focus {
        " Scan roots ▶ "
    } else {
        " Scan roots "
    };
    // Panel-level hit targets first; row clicks register after and win (later = top).
    app.register_click(
        lists[0].x,
        lists[0].y,
        lists[0].width,
        lists[0].height,
        Action::FocusWorkspaceRoots,
    );
    app.register_click(
        lists[1].x,
        lists[1].y,
        lists[1].width,
        lists[1].height,
        Action::FocusWorkspaceExcludes,
    );

    // Disable table_nav's built-in SelectRow clicks (wrong action on excludes);
    // we register the correct actions ourselves below.
    render_selectable_list(
        frame,
        app,
        lists[0],
        roots_title,
        &root_items,
        Some(&["#", "Path", "On disk", "Projects"]),
        root_sel,
        false,
    );
    register_root_clicks(app, lists[0], roots.len());

    // ── Excludes ────────────────────────────────────────────────────────
    let ex_sel = app
        .selected_exclude_index
        .min(excludes.len().saturating_sub(1));
    let ex_items: Vec<Vec<String>> = if excludes.is_empty() {
        vec![vec![
            "—".into(),
            "(none — n to hide projects by name/path/glob)".into(),
        ]]
    } else {
        excludes
            .iter()
            .enumerate()
            .map(|(i, pat)| vec![format!("{}", i + 1), pat.clone()])
            .collect()
    };
    let excludes_title = if excludes_focus {
        " Project excludes ▶ "
    } else {
        " Project excludes "
    };
    render_selectable_list(
        frame,
        app,
        lists[1],
        excludes_title,
        &ex_items,
        Some(&["#", "Pattern"]),
        if excludes.is_empty() { 0 } else { ex_sel },
        false,
    );
    if !excludes.is_empty() {
        register_exclude_clicks(app, lists[1], excludes.len());
    }

    // ── Detail ──────────────────────────────────────────────────────────
    match app.workspace_panel {
        WorkspacePanel::Roots => render_root_detail_panel(frame, app, cols[1], roots, root_sel),
        WorkspacePanel::Excludes => {
            render_exclude_detail_panel(frame, app, cols[1], excludes, ex_sel)
        }
    }
}

fn count_projects_under(app: &App, root: &str) -> usize {
    let root = Path::new(root);
    app.projects
        .iter()
        .filter(|p| p.path.starts_with(root))
        .count()
}

fn render_root_detail_panel(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    roots: &[String],
    selected: usize,
) {
    let Some(path) = roots.get(selected) else {
        frame.render_widget(
            Paragraph::new("No root selected.")
                .style(app.theme.dim)
                .block(detail_block(app, " Root detail ")),
            area,
        );
        return;
    };

    let p = Path::new(path);
    let exists = p.is_dir();
    let n = count_projects_under(app, path);
    let mut svc = 0usize;
    let mut lib = 0usize;
    let mut avro = 0usize;
    for proj in app.projects.iter().filter(|pr| pr.path.starts_with(p)) {
        match proj.kind {
            ProjectKind::Service => svc += 1,
            ProjectKind::Library => lib += 1,
            ProjectKind::Avro => avro += 1,
            ProjectKind::Unknown => {}
        }
    }

    let status_span = if !exists {
        Span::styled("missing on disk", app.theme.error.add_modifier(Modifier::BOLD))
    } else if n == 0 {
        Span::styled("empty (no projects yet)", app.theme.warning)
    } else {
        Span::styled("ok", app.theme.success.add_modifier(Modifier::BOLD))
    };

    let lines = vec![
        Line::from(Span::styled("Selected root", app.theme.dim)),
        Line::from(Span::styled(path.clone(), app.theme.text.add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from(vec![
            Span::styled("Status   ", app.theme.dim),
            status_span,
        ]),
        Line::from(vec![
            Span::styled("Projects ", app.theme.dim),
            Span::styled(format!("{n}"), app.theme.text.add_modifier(Modifier::BOLD)),
            Span::styled(
                format!("  ({svc} svc · {lib} lib · {avro} avro)"),
                app.theme.dim,
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled("Actions", app.theme.secondary.add_modifier(Modifier::BOLD))),
        Line::from(vec![
            Span::styled("  e  ", app.theme.secondary.add_modifier(Modifier::BOLD)),
            Span::styled("edit path", app.theme.text),
        ]),
        Line::from(vec![
            Span::styled("  z  ", app.theme.secondary.add_modifier(Modifier::BOLD)),
            Span::styled("remove from workspace", app.theme.text),
        ]),
        Line::from(vec![
            Span::styled("  r  ", app.theme.secondary.add_modifier(Modifier::BOLD)),
            Span::styled("rescan all roots", app.theme.text),
        ]),
        Line::from(vec![
            Span::styled("  Enter  ", app.theme.secondary.add_modifier(Modifier::BOLD)),
            Span::styled("open project browser", app.theme.text),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Tab → excludes. n/e/z edit this list.",
            app.theme.dim,
        )),
        Line::from(Span::styled(
            "Saved to [scan].roots in config.toml.",
            app.theme.dim,
        )),
    ];

    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(detail_block(app, " Root detail ")),
        area,
    );
}

fn render_exclude_detail_panel(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    excludes: &[String],
    selected: usize,
) {
    if excludes.is_empty() {
        let lines = vec![
            Line::from(Span::styled(
                "No exclude patterns",
                app.theme.accent.add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Hide projects that still live under a scan root",
                app.theme.dim,
            )),
            Line::from(Span::styled(
                "but should not appear in the inventory.",
                app.theme.dim,
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled("  n  ", app.theme.secondary.add_modifier(Modifier::BOLD)),
                Span::styled("add pattern", app.theme.text),
            ]),
            Line::from(""),
            Line::from(Span::styled("Examples", app.theme.secondary.add_modifier(Modifier::BOLD))),
            Line::from(Span::styled("  avro-schemas", app.theme.text)),
            Line::from(Span::styled("  services/legacy-api", app.theme.text)),
            Line::from(Span::styled("  **/tmp-*", app.theme.text)),
            Line::from(""),
            Line::from(Span::styled(
                "Saved to [scan].exclude · rescan applies.",
                app.theme.dim,
            )),
        ];
        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .block(detail_block(app, " Exclude detail ")),
            area,
        );
        return;
    }

    let Some(pat) = excludes.get(selected) else {
        frame.render_widget(
            Paragraph::new("No exclude selected.")
                .style(app.theme.dim)
                .block(detail_block(app, " Exclude detail ")),
            area,
        );
        return;
    };

    let lines = vec![
        Line::from(Span::styled("Selected exclude", app.theme.dim)),
        Line::from(Span::styled(
            pat.clone(),
            app.theme.text.add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Exact match on project name or id only",
            app.theme.dim,
        )),
        Line::from(Span::styled(
            "(avro-schemas ≠ avro-schemas/cats). Or * / ** globs.",
            app.theme.dim,
        )),
        Line::from(""),
        Line::from(Span::styled("Actions", app.theme.secondary.add_modifier(Modifier::BOLD))),
        Line::from(vec![
            Span::styled("  e  ", app.theme.secondary.add_modifier(Modifier::BOLD)),
            Span::styled("edit pattern", app.theme.text),
        ]),
        Line::from(vec![
            Span::styled("  z  ", app.theme.secondary.add_modifier(Modifier::BOLD)),
            Span::styled("remove exclude", app.theme.text),
        ]),
        Line::from(vec![
            Span::styled("  n  ", app.theme.secondary.add_modifier(Modifier::BOLD)),
            Span::styled("add another", app.theme.text),
        ]),
        Line::from(vec![
            Span::styled("  Tab  ", app.theme.secondary.add_modifier(Modifier::BOLD)),
            Span::styled("back to scan roots", app.theme.text),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Saved to [scan].exclude · rescan on save.",
            app.theme.dim,
        )),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(detail_block(app, " Exclude detail ")),
        area,
    );
}

fn detail_block<'a>(app: &'a App, title: &'a str) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .title(title)
        .title_style(app.theme.title)
        .border_style(app.theme.border)
        .style(app.theme.root_style())
}

fn register_root_clicks(app: &App, list_area: Rect, root_count: usize) {
    register_list_clicks(app, list_area, root_count, |i| Action::SelectRow(i));
}

fn register_exclude_clicks(app: &App, list_area: Rect, count: usize) {
    register_list_clicks(app, list_area, count, |i| Action::SelectExcludeRow(i));
}

fn register_list_clicks(
    app: &App,
    list_area: Rect,
    count: usize,
    action: impl Fn(usize) -> Action,
) {
    if count == 0 || list_area.height < 3 {
        return;
    }
    // Match table_nav geometry: inner area = border inset; header is 1 row.
    let inner = Block::default().borders(Borders::ALL).inner(list_area);
    let header_h = 1u16;
    let data_top = inner.y.saturating_add(header_h);
    let rows_h = inner.height.saturating_sub(header_h);
    for i in 0..count {
        if (i as u16) >= rows_h {
            break;
        }
        let y = data_top.saturating_add(i as u16);
        app.register_click(inner.x, y, inner.width, 1, action(i));
    }
}

fn render_delete_confirm(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    title: &str,
    value: Option<&str>,
    lead: &str,
    trail: &str,
) {
    let value = value.unwrap_or("(unknown)");
    let body = format!("{lead}\n\n{value}\n\n{trail}");
    render_confirm_dialog(
        frame,
        area,
        &app.theme,
        title,
        &body,
        Some("This cannot be undone from the TUI (re-add with n)."),
    );
}

fn render_list_editor(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    state: &RootEditorState,
    first_run: bool,
) {
    let dialog = centered_rect(70, 50, area);
    frame.render_widget(Clear, dialog);

    let is_exclude = state.is_exclude();
    let title = if is_exclude {
        if state.is_edit() {
            " Edit exclude pattern "
        } else {
            " Add exclude pattern "
        }
    } else if first_run {
        " Add scan root "
    } else if state.is_edit() {
        " Edit scan root "
    } else {
        " Add scan root "
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .title_style(app.theme.title)
        .border_style(app.theme.secondary.add_modifier(Modifier::BOLD))
        .style(app.theme.panel_style());
    let inner = block.inner(dialog);
    frame.render_widget(block, dialog);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // intro
            Constraint::Length(3), // field
            Constraint::Length(2), // preview
            Constraint::Min(2),    // error / help
            Constraint::Length(1), // footer
        ])
        .split(inner);

    let intro = if is_exclude {
        if state.is_edit() {
            "Update the exclude pattern (leaf name, path fragment, or glob)."
        } else {
            "Hide matching projects from the inventory after scan. Examples: legacy-api, **/sandbox/**"
        }
    } else if first_run {
        "Point tako at a folder tree that holds your services, libs, and avro repos."
    } else if state.is_edit() {
        "Update the absolute path (or ~/…) for this scan root."
    } else {
        "Add another folder to the workspace — absolute path or ~/…"
    };
    frame.render_widget(
        Paragraph::new(intro)
            .style(app.theme.status)
            .wrap(Wrap { trim: true })
            .alignment(Alignment::Left),
        chunks[0],
    );

    let field_title = if is_exclude { " Pattern " } else { " Path " };
    render_text_field(frame, app, chunks[1], field_title, &state.display_with_cursor());

    // Live preview
    let (preview, preview_style) = if is_exclude {
        match crate::app::normalize_exclude_pattern(&state.path) {
            Ok(p) => (format!("→ {p}"), app.theme.success),
            Err(_) if state.path.trim().is_empty() => ("→ …".to_string(), app.theme.dim),
            Err(e) => (format!("→ {e}"), app.theme.dim),
        }
    } else {
        match crate::app::normalize_root_path(&state.path) {
            Ok(p) => {
                let exists = Path::new(&p).is_dir();
                let mark = if exists { "exists" } else { "not found yet" };
                let style = if exists {
                    app.theme.success
                } else {
                    app.theme.warning
                };
                (format!("→ {p}  ({mark})"), style)
            }
            Err(_) if state.path.trim().is_empty() => ("→ …".to_string(), app.theme.dim),
            Err(e) => (format!("→ {e}"), app.theme.dim),
        }
    };
    frame.render_widget(Paragraph::new(preview).style(preview_style), chunks[2]);

    if let Some(err) = &state.error {
        frame.render_widget(
            Paragraph::new(err.as_str())
                .style(app.theme.error.add_modifier(Modifier::BOLD))
                .wrap(Wrap { trim: true }),
            chunks[3],
        );
    } else {
        let help = if is_exclude {
            "Saves to [scan].exclude in config.toml. Workspace rescans automatically."
        } else {
            "Saves to [scan].roots in config.toml. Non-empty workspace rescans automatically."
        };
        frame.render_widget(
            Paragraph::new(help)
                .style(app.theme.dim)
                .wrap(Wrap { trim: true }),
            chunks[3],
        );
    }

    let footer = "Enter: save & rescan     Esc: cancel";
    frame.render_widget(
        Paragraph::new(footer)
            .style(app.theme.secondary.add_modifier(Modifier::BOLD))
            .alignment(Alignment::Center),
        chunks[4],
    );
}

fn render_text_field(frame: &mut Frame, app: &App, area: Rect, title: &str, value: &str) {
    frame.render_widget(
        Paragraph::new(value).style(app.theme.text).block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .title_style(app.theme.focus_title(true))
                .border_style(app.theme.focus_border(true))
                .style(app.theme.root_style()),
        ),
        area,
    );
}

fn format_last_scan(app: &App) -> String {
    match app.last_scan_at {
        Some(at) => {
            let secs = at.elapsed().map(|d| d.as_secs()).unwrap_or(0);
            let human = if secs < 5 {
                "just now".into()
            } else if secs < 60 {
                format!("{secs}s ago")
            } else if secs < 3600 {
                format!("{}m ago", secs / 60)
            } else if secs < 86400 {
                format!("{}h ago", secs / 3600)
            } else {
                format!("{}d ago", secs / 86400)
            };
            format!(" · last scan {human}")
        }
        None => " · never scanned".to_string(),
    }
}
