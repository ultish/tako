//! Persistent top-level view switcher bar (`1`/`2`/`3`).

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::{App, Screen};
use crate::events::Action;

const ENTRIES: [(&str, &str, Action); 4] = [
    ("1", "Projects", Action::SwitchToProjects),
    ("2", "Jobs", Action::SwitchToJobs),
    ("3", "Workspace", Action::SwitchToWorkspace),
    ("4", "Settings", Action::SwitchToSettings),
];

/// Column gap between switcher entries.
const GAP: u16 = 3;

fn active_index(screen: Screen) -> Option<usize> {
    match screen {
        Screen::ProjectBrowser => Some(0),
        Screen::Jobs => Some(1),
        Screen::Workspace => Some(2),
        Screen::Settings => Some(3),
    }
}

/// Whether `screen` shows the switcher bar at all.
pub fn is_visible(screen: Screen) -> bool {
    active_index(screen).is_some()
}

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let Some(active) = active_index(app.screen) else {
        return;
    };

    let mut spans = Vec::new();
    let mut x = area.x;
    for (i, (key, label, action)) in ENTRIES.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" ".repeat(GAP as usize)));
            x += GAP;
        }
        let style = if i == active {
            app.theme.selected_row
        } else {
            app.theme.secondary
        };
        let text = format!("{key} {label}");
        let width = text.chars().count() as u16;
        spans.push(Span::styled(text, style));
        app.register_click(x, area.y, width, 1, action.clone());
        x += width;
    }
    frame.render_widget(Paragraph::new(Line::from(spans)).style(Style::default()), area);
}
