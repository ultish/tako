//! Overlay: bump each dependent project's own Gradle version (smart per-row).

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::App;
use crate::ui::widgets::confirm_dialog::centered_rect;
use crate::version_bump::BumpKind;

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let Some(plan) = app.bump_plan.as_ref() else {
        return;
    };

    let dialog = centered_rect(86, 76, area);
    frame.render_widget(Clear, dialog);

    let theme = &app.theme;
    let kind = plan.kind.label();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Bump version · {kind} "))
        .title_style(theme.title)
        .border_style(theme.secondary.add_modifier(Modifier::BOLD))
        .style(theme.panel_style());
    let inner = block.inner(dialog);
    frame.render_widget(block, dialog);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(2),
        ])
        .split(inner);

    let header = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("Project  ", theme.dim),
            Span::styled(
                format!("{} @ {}", plan.source_name, plan.source_version),
                theme.secondary.add_modifier(Modifier::BOLD),
            ),
            Span::styled("  — raise this project’s own version", theme.dim),
        ]),
        Line::from(Span::styled(
            format!(
                "kind: {} (1 major · 2 minor · 3 patch · Tab cycle) · y apply · n cancel",
                kind
            ),
            theme.dim,
        )),
        Line::from(Span::styled(
            "y rewrites the Gradle version line and git-commits the change.",
            theme.dim,
        )),
    ]);
    frame.render_widget(header, chunks[0]);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(
        format!(
            "  {:<3} {:<28} {:<14} {:<14} {}",
            "#", "PROJECT", "FROM", "TO", "FILE"
        ),
        theme.dim.add_modifier(Modifier::UNDERLINED),
    )));

    let viewport = chunks[1].height.saturating_sub(1) as usize;
    let total = plan.rows.len();
    let cursor = plan.cursor.min(total.saturating_sub(1));
    let start = if total <= viewport {
        0
    } else if cursor < viewport {
        0
    } else {
        (cursor + 1).saturating_sub(viewport)
    };
    let end = (start + viewport).min(total);

    for (i, row) in plan.rows.iter().enumerate().take(end).skip(start) {
        let selected = i == cursor;
        let style = if row.error.is_some() {
            theme.error
        } else if selected {
            theme.selected_row
        } else {
            theme.text
        };
        let mark = if selected { ">" } else { " " };
        let file_leaf = row
            .file
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| row.file.display().to_string());
        let to = if let Some(err) = &row.error {
            format!("ERR: {err}")
        } else {
            row.new_version.clone()
        };
        let text = format!(
            "{mark} {:<2} {:<28} {:<14} {:<14} {}",
            i + 1,
            truncate(&row.project_name, 28),
            truncate(&row.old_version, 14),
            truncate(&to, 14),
            file_leaf
        );
        lines.push(Line::from(Span::styled(text, style)));
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), chunks[1]);

    let kind_hint = match plan.kind {
        BumpKind::Major => "1.2.3 → 2.0.0",
        BumpKind::Minor => "1.2.3 → 1.3.0",
        BumpKind::Patch => "1.2.3 → 1.2.4",
    };
    let ok = plan.ok_count();
    let footer = Paragraph::new(Line::from(vec![
        Span::styled(
            format!(" {ok}/{} ready  ·  example {kind_hint}  ·  ", plan.rows.len()),
            theme.dim,
        ),
        Span::styled(
            "y/Enter apply   n/Esc cancel",
            theme.secondary.add_modifier(Modifier::BOLD),
        ),
    ]));
    frame.render_widget(footer, chunks[2]);
}

fn truncate(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    let take = max.saturating_sub(1);
    format!("{}…", s.chars().take(take).collect::<String>())
}
