pub mod banner;
pub mod help;
pub mod screens;
pub mod splash;
pub mod theme;
pub mod widgets;

use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::widgets::{Block, Clear};
use ratatui::Frame;

use crate::app::{App, Screen};
use crate::ui::widgets::confirm_dialog::render_confirm_dialog;
use crate::ui::widgets::view_switcher;

/// Draws splash (first paint) or banner + active screen.
pub fn draw(frame: &mut Frame, app: &App) {
    app.clear_click_regions();
    let area = frame.area();

    // Paint theme surface so the TUI owns the background.
    frame.render_widget(Clear, area);
    frame.render_widget(Block::default().style(app.theme.root_style()), area);

    if app.show_splash {
        splash::render(frame, app, area);
        if app.quit_confirm {
            render_quit_confirm(frame, app, area);
        }
        return;
    }

    let show_switcher = view_switcher::is_visible(app.screen);
    let mut constraints = vec![Constraint::Length(banner::BANNER_HEIGHT)];
    if show_switcher {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Min(1));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    banner::render(frame, app, chunks[0]);
    let content = if show_switcher {
        view_switcher::render(frame, app, chunks[1]);
        chunks[2]
    } else {
        chunks[1]
    };

    match app.screen {
        Screen::ProjectBrowser
        | Screen::Jobs
        | Screen::Workspace
        | Screen::Settings => {
            screens::project_browser::render(frame, app, content);
        }
    }

    // Cascade plan confirm sits above screens, below help/quit.
    if app.plan_confirming() {
        screens::cascade_plan::render(frame, app, area);
    }

    if app.help_visible {
        help::render(frame, app, area);
    }

    if app.quit_confirm {
        render_quit_confirm(frame, app, area);
    }
}

fn render_quit_confirm(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    render_confirm_dialog(
        frame,
        area,
        &app.theme,
        "Quit tako?",
        "Exit the TUI?\n\ny/Enter: quit   n/Esc: cancel",
        None,
    );
}
