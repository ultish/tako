//! Cascade plan confirm overlay (SPEC wireframe: publish_and_redeploy_consumers).

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::App;
use crate::exec::CascadeStepStatus;
use crate::ui::widgets::confirm_dialog::centered_rect;

/// Full-frame dimmed overlay with step table + warnings + y/n footer.
pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let Some(plan) = app.plan.as_ref() else {
        return;
    };
    if plan.executing {
        return;
    }

    let dialog = centered_rect(88, 78, area);
    frame.render_widget(Clear, dialog);

    let theme = &app.theme;
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" plan: {} ", plan.recipe))
        .title_style(theme.title)
        .border_style(theme.warning)
        .style(theme.panel_style());
    let inner = block.inner(dialog);
    frame.render_widget(block, dialog);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // source header
            Constraint::Min(4),    // steps
            Constraint::Length(if plan.warnings.is_empty() {
                0
            } else {
                (plan.warnings.len() as u16).saturating_add(1).min(5)
            }),
            Constraint::Length(1), // footer
        ])
        .split(inner);

    let header = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("source: ", theme.dim),
            Span::styled(
                format!("{} @ {}", plan.source_name, plan.source_version),
                theme.secondary.add_modifier(Modifier::BOLD),
            ),
            Span::styled("  →  publish to Nexus", theme.dim),
        ]),
        Line::from(Span::styled(
            format!(
                "{} step(s) · j/k scroll · y/Enter run · n/Esc cancel",
                plan.steps.len()
            ),
            theme.dim,
        )),
    ]);
    frame.render_widget(header, chunks[0]);

    // Table of steps with cursor highlight
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(
        format!(
            "  {:<3} {:<28} {:<18} {}",
            "#", "STEP", "PROJECT", "TASK"
        ),
        theme.dim.add_modifier(Modifier::UNDERLINED),
    )));

    let viewport_h = chunks[1].height.saturating_sub(1) as usize;
    let total = plan.steps.len();
    let cursor = plan.cursor.min(total.saturating_sub(1));
    let offset = if total <= viewport_h {
        0
    } else {
        cursor
            .saturating_sub(viewport_h / 2)
            .min(total.saturating_sub(viewport_h))
    };

    for (i, step) in plan.steps.iter().enumerate().skip(offset).take(viewport_h) {
        let selected = i == plan.cursor;
        let style = if selected {
            theme.selected_row
        } else {
            theme.text
        };
        let marker = if selected { ">" } else { " " };
        let status = match step.status {
            CascadeStepStatus::Pending => "",
            s => s.label(),
        };
        let row = format!(
            "{marker}{:<2} {:<28} {:<18} {:<20} {status}",
            i + 1,
            truncate(&step.step_label(), 28),
            truncate(&step.project_name, 18),
            truncate(&step.task_column(), 20),
        );
        lines.push(Line::from(Span::styled(row, style)));
    }

    frame.render_widget(
        Paragraph::new(lines)
            .style(theme.text)
            .wrap(Wrap { trim: false }),
        chunks[1],
    );

    if !plan.warnings.is_empty() && chunks[2].height > 0 {
        let warn_lines: Vec<Line> = plan
            .warnings
            .iter()
            .take(chunks[2].height as usize)
            .map(|w| {
                Line::from(Span::styled(
                    format!("warn: {w}"),
                    theme.error,
                ))
            })
            .collect();
        frame.render_widget(Paragraph::new(warn_lines), chunks[2]);
    }

    frame.render_widget(
        Paragraph::new("y/Enter run plan    n/Esc cancel    j/k scroll")
            .style(theme.secondary.add_modifier(Modifier::BOLD)),
        chunks[3],
    );
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}
