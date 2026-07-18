//! Confirm overlay for multi-step plans (U / u / etc.).

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::App;
use crate::exec::{recipe_summary, recipe_title, CascadeStepStatus};
use crate::ui::widgets::confirm_dialog::centered_rect;

/// Full-frame dimmed overlay with lib Nexus checks + service step table.
pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let Some(plan) = app.plan.as_ref() else {
        return;
    };
    if plan.executing {
        return;
    }

    let dialog = centered_rect(96, 84, area);
    frame.render_widget(Clear, dialog);

    let theme = &app.theme;
    let title = recipe_title(&plan.recipe);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {title} "))
        .title_style(theme.title)
        .border_style(theme.warning)
        .style(theme.panel_style());
    let inner = block.inner(dialog);
    frame.render_widget(block, dialog);

    let lib_h = if plan.lib_checks.is_empty() {
        0
    } else {
        (plan.lib_checks.len() as u16)
            .saturating_add(2) // section title + blank
            .min(12)
    };
    let warn_h = if plan.warnings.is_empty() {
        0
    } else {
        (plan.warnings.len() as u16).saturating_add(1).min(5)
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // header
            Constraint::Length(lib_h),
            Constraint::Min(4), // service steps
            Constraint::Length(warn_h),
            Constraint::Length(1), // footer
        ])
        .split(inner);

    let header = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("Starting from  ", theme.dim),
            Span::styled(
                format!("{} @ {}", plan.source_name, plan.source_version),
                theme.secondary.add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(Span::styled(recipe_summary(&plan.recipe), theme.dim)),
        Line::from(Span::styled(
            format!(
                "{} lib check(s) · {} service step(s) · j/k · y run · n cancel",
                plan.lib_checks.len(),
                plan.steps.len()
            ),
            theme.dim,
        )),
    ]);
    frame.render_widget(header, chunks[0]);

    // ── Child libs: Nexus status (not executed) ───────────────────────────
    if lib_h > 0 {
        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::from(Span::styled(
            "LIBS — expect on Nexus (not built locally)",
            theme.warning.add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )));
        for row in plan.lib_checks.iter().take(lib_h.saturating_sub(1) as usize) {
            let glyph = row.status_glyph();
            let style = match row.status.as_str() {
                "ok" => theme.success,
                "warn" => theme.error,
                _ => theme.dim,
            };
            lines.push(Line::from(Span::styled(
                format!(
                    "  {glyph} {:<28} v{:<12} {}",
                    truncate(&row.project_name, 28),
                    truncate(&row.local_version, 12),
                    truncate(&row.detail, 48),
                ),
                style,
            )));
        }
        frame.render_widget(
            Paragraph::new(lines).wrap(Wrap { trim: false }),
            chunks[1],
        );
    }

    // ── Service steps ─────────────────────────────────────────────────────
    let step_area = chunks[2];
    let cols = step_table_widths(step_area.width);
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(
        if plan.steps.is_empty() {
            "SERVICES — (none to run)".to_string()
        } else {
            "SERVICES — will run".to_string()
        },
        theme.secondary.add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
    )));
    lines.push(Line::from(Span::styled(
        format!(
            "  {:<3} {:<w_step$} {:<w_proj$} {}",
            "#",
            "STEP",
            "PROJECT",
            "TASK",
            w_step = cols.step,
            w_proj = cols.project,
        ),
        theme.dim.add_modifier(Modifier::UNDERLINED),
    )));

    let viewport_h = step_area.height.saturating_sub(2) as usize;
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
        let task = step.task_column();
        let row = format!(
            "{marker}{:<2} {:<w_step$} {:<w_proj$} {:<w_task$} {status}",
            i + 1,
            truncate(&step.step_label(), cols.step),
            truncate(&step.project_name, cols.project),
            truncate(&task, cols.task),
            w_step = cols.step,
            w_proj = cols.project,
            w_task = cols.task,
        );
        lines.push(Line::from(Span::styled(row, style)));
    }

    frame.render_widget(
        Paragraph::new(lines)
            .style(theme.text)
            .wrap(Wrap { trim: false }),
        step_area,
    );

    if warn_h > 0 {
        let warn_lines: Vec<Line> = plan
            .warnings
            .iter()
            .take(warn_h as usize)
            .map(|w| Line::from(Span::styled(format!("warn: {w}"), theme.error)))
            .collect();
        frame.render_widget(Paragraph::new(warn_lines), chunks[3]);
    }

    frame.render_widget(
        Paragraph::new("y/Enter run services    n/Esc cancel    j/k scroll")
            .style(theme.secondary.add_modifier(Modifier::BOLD)),
        chunks[4],
    );
}

struct ColWidths {
    step: usize,
    project: usize,
    task: usize,
}

fn step_table_widths(area_width: u16) -> ColWidths {
    let fixed = 3usize + 4 + 8;
    let avail = (area_width as usize).saturating_sub(fixed).max(40);
    let mut step = (avail * 42 / 100).max(32);
    let mut project = (avail * 38 / 100).max(24);
    let mut task = avail.saturating_sub(step + project).max(12);
    let total = step + project + task;
    if total > avail {
        let mut over = total - avail;
        let shrink = |col: &mut usize, floor: usize, over: &mut usize| {
            let can = col.saturating_sub(floor).min(*over);
            *col -= can;
            *over -= can;
        };
        shrink(&mut task, 10, &mut over);
        shrink(&mut project, 20, &mut over);
        shrink(&mut step, 24, &mut over);
    }
    ColWidths {
        step,
        project,
        task,
    }
}

fn truncate(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}
