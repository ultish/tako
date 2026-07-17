//! Jobs console — list of background jobs + live log pane for the selection.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, JobsFocus};
use crate::events::Action;
use crate::jobs::JobStatus;
use crate::ui::widgets::footer::{
    render_keybind_footer, render_status_bar, split_with_footer,
};
use crate::ui::widgets::table_nav::render_selectable_list;

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let (main, status_bar, footer) = split_with_footer(area);

    if app.jobs.is_empty() {
        render_empty(frame, app, main);
        render_status_bar(frame, app, status_bar);
        render_keybind_footer(
            frame,
            footer,
            &app.theme,
            "1: projects   b/c/p: gradle   d/D/x/u: skaffold   G: pull   ?: help   q: quit",
        );
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
        .split(main);

    render_job_list(frame, app, chunks[0]);
    render_log_pane(frame, app, chunks[1]);

    let focus_hint = match app.jobs_focus {
        JobsFocus::List => "focus: list",
        JobsFocus::Log => "focus: log",
    };
    render_status_bar(frame, app, status_bar);
    render_keybind_footer(
        frame,
        footer,
        &app.theme,
        &format!(
            "j/k: move   Tab: list/log ({focus_hint})   Esc: cancel job   1: projects   ?: help   q: quit"
        ),
    );
}

fn render_empty(frame: &mut Frame, app: &App, area: Rect) {
    let body = "No jobs yet.\n\n\
         From Projects (1), run:\n\
           b build · c clean · p publish\n\
           d dev · D debug · x delete · u run\n\
           G git pull\n\n\
         Live logs appear here; Esc cancels the focused running job.";
    let message = Paragraph::new(body)
        .style(app.theme.status)
        .wrap(Wrap { trim: false })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Jobs")
                .title_style(app.theme.title)
                .border_style(app.theme.border)
                .style(app.theme.root_style()),
        );
    frame.render_widget(message, area);
}

fn render_job_list(frame: &mut Frame, app: &App, area: Rect) {
    let items: Vec<Vec<String>> = app
        .jobs
        .iter()
        .map(|j| {
            vec![
                format!("#{}", j.id),
                j.project_name.clone(),
                j.kind.label(),
                j.status.label().to_string(),
                format!("{}s", j.elapsed_secs()),
            ]
        })
        .collect();

    let title = match app.jobs_focus {
        JobsFocus::List => "Jobs ▶",
        JobsFocus::Log => "Jobs",
    };

    render_selectable_list(
        frame,
        app,
        area,
        title,
        &items,
        Some(&["Id", "Project", "Kind", "St", "Time"]),
        app.selected_job,
        true,
    );

    // Click regions for job rows (SelectRow index = job index).
    remap_job_row_clicks(app, area, items.len());
}

fn remap_job_row_clicks(app: &App, area: Rect, count: usize) {
    if count == 0 {
        return;
    }
    use crate::ui::widgets::table_nav::{
        selectable_list_offset, selectable_list_viewport_rows,
    };

    let has_header = true;
    let viewport = selectable_list_viewport_rows(area, has_header);
    let offset = selectable_list_offset(app.selected_job, count, viewport);
    let inner = Block::default().borders(Borders::ALL).inner(area);
    let header_h = 1u16;
    let rows_y = inner.y + header_h;
    let rows_h = inner.height.saturating_sub(header_h);
    let visible = (rows_h as usize).min(count.saturating_sub(offset));
    for i in 0..visible {
        let idx = offset + i;
        let y = rows_y + i as u16;
        app.register_click(inner.x, y, inner.width, 1, Action::SelectRow(idx));
    }
}

fn render_log_pane(frame: &mut Frame, app: &App, area: Rect) {
    let Some(job) = app.jobs.get(app.selected_job) else {
        return;
    };

    let focus_mark = match app.jobs_focus {
        JobsFocus::Log => " ▶",
        JobsFocus::List => "",
    };
    let status_style = match job.status {
        JobStatus::Ok => app.theme.success,
        JobStatus::Failed => app.theme.error,
        JobStatus::Cancelled => app.theme.warning,
        JobStatus::Running | JobStatus::Pending => app.theme.secondary,
    };

    let summary = job
        .summary
        .as_deref()
        .map(|s| format!(" — {s}"))
        .unwrap_or_default();
    let title = format!(
        "Log{focus_mark} · {} · {}{summary}",
        job.kind.label(),
        job.status.label()
    );

    let lines: Vec<&str> = job.log.iter().map(String::as_str).collect();
    let total = lines.len();
    let inner_h = Block::default()
        .borders(Borders::ALL)
        .inner(area)
        .height as usize;
    let viewport = inner_h.max(1);

    // job_log_scroll is an offset from the start (0 = oldest visible first).
    let max_scroll = total.saturating_sub(viewport);
    let scroll = app.job_log_scroll.min(max_scroll);
    let end = (scroll + viewport).min(total);
    let slice = if total == 0 {
        String::from("(no output yet)")
    } else {
        lines[scroll..end].join("\n")
    };

    let border = if app.jobs_focus == JobsFocus::Log {
        app.theme.secondary.add_modifier(Modifier::BOLD)
    } else {
        app.theme.border
    };

    let message = Paragraph::new(slice)
        .style(app.theme.status)
        .wrap(Wrap { trim: false })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .title_style(status_style.add_modifier(Modifier::BOLD))
                .border_style(border)
                .style(app.theme.root_style()),
        );
    frame.render_widget(message, area);
}
