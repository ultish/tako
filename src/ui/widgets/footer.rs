use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::App;
use crate::ui::theme::Theme;

/// Split `area` into (main content, status bar, keybind footer).
///
/// Status + keybinds always reserve two bottom rows so job feedback never
/// overlays the main content (replaces the old top-centered toast).
pub fn split_with_footer(area: Rect) -> (Rect, Rect, Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(1), // status bar
            Constraint::Length(1), // keybinds
        ])
        .split(area);
    (chunks[0], chunks[1], chunks[2])
}

/// One-line keybind / help strip.
pub fn render_keybind_footer(frame: &mut Frame, area: Rect, theme: &Theme, text: &str) {
    frame.render_widget(Paragraph::new(text).style(theme.status), area);
}

/// Bottom status bar: shows `app.status_message` with success / error / info
/// coloring. Empty when there is no status (layout stays reserved).
pub fn render_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let Some(status) = app.status_message.as_deref() else {
        // Keep the row painted so the chrome doesn't flicker.
        frame.render_widget(Paragraph::new("").style(app.theme.root_style()), area);
        return;
    };

    let lower = status.to_ascii_lowercase();
    let success = is_success_status(&lower);
    let error = lower.contains("fail") || lower.contains("error");

    let style = if success && !error {
        Style::new()
            .fg(app.theme.success.fg.unwrap_or(Color::Green))
            .add_modifier(Modifier::BOLD)
    } else if error {
        app.theme.error.add_modifier(Modifier::BOLD)
    } else {
        // In-flight / neutral (scanning…, build…, cascade…)
        app.theme.secondary.add_modifier(Modifier::BOLD)
    };

    let line = Line::from(vec![
        Span::styled(" ", app.theme.root_style()),
        Span::styled(status, style),
    ]);
    frame.render_widget(
        Paragraph::new(line).style(app.theme.root_style()),
        area,
    );
}

fn is_success_status(lower: &str) -> bool {
    if lower.contains("fail") || lower.contains("error") {
        return false;
    }
    lower.starts_with("copied")
        || lower.starts_with("pasted")
        || (lower.starts_with("pull") && lower.contains("ok"))
        || (lower.starts_with("build")
            && (lower.contains("ok") || lower.contains("done") || lower.contains("finished")))
        || (lower.starts_with("publish")
            && (lower.contains("ok") || lower.contains("done") || lower.contains("finished")))
        || (lower.starts_with("scan")
            && (lower.contains("ok")
                || lower.contains("done")
                || lower.contains("finished")
                || lower.contains("complete")))
        || (lower.starts_with("cascade")
            && (lower.contains("ok") || lower.contains("done") || lower.contains("finished")))
        || (lower.starts_with("skaffold")
            && (lower.contains("started") || lower.contains("ok") || lower.contains("stopped")))
        || lower.starts_with("added root")
        || lower.starts_with("updated root")
        || lower.starts_with("deleted root")
}
