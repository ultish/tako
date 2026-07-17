//! Global help overlay (`?`) built from a shared per-screen keybind table.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, Screen};
use crate::ui::widgets::confirm_dialog::centered_rect;

pub struct KeybindEntry {
    pub keys: &'static str,
    pub description: &'static str,
}

pub struct KeybindSection {
    pub title: &'static str,
    pub entries: &'static [KeybindEntry],
}

const GLOBAL: KeybindSection = KeybindSection {
    title: "Global",
    entries: &[
        KeybindEntry {
            keys: "?",
            description: "Toggle this help",
        },
        KeybindEntry {
            keys: "q",
            description: "Quit (confirm)",
        },
        KeybindEntry {
            keys: "Ctrl-c",
            description: "Force quit",
        },
        KeybindEntry {
            keys: "A",
            description: "Cycle banner (wave → ms/frame → fps → off); saved to config",
        },
        KeybindEntry {
            keys: "T",
            description: "Cycle theme (dark ↔ light); saved to config",
        },
        KeybindEntry {
            keys: "j/k · ↑/↓",
            description: "Move selection",
        },
        KeybindEntry {
            keys: "Enter",
            description: "Confirm / open",
        },
        KeybindEntry {
            keys: "Esc",
            description: "Back / cancel",
        },
        KeybindEntry {
            keys: "1/2/3/4",
            description: "Jump Projects / Jobs / Workspace / Settings",
        },
    ],
};

const SETTINGS: KeybindSection = KeybindSection {
    title: "Settings",
    entries: &[
        KeybindEntry {
            keys: "j/k · ↑/↓",
            description: "Move between settings",
        },
        KeybindEntry {
            keys: "Enter · Space",
            description: "Toggle bool / cycle / open editor",
        },
        KeybindEntry {
            keys: "← / →",
            description: "Nudge integer settings",
        },
        KeybindEntry {
            keys: "kube.enabled",
            description: "Toggle cluster version probing (then K on Projects)",
        },
    ],
};

const PROJECT_BROWSER: KeybindSection = KeybindSection {
    title: "Project browser",
    entries: &[
        KeybindEntry {
            keys: "Space",
            description: "Toggle multi-select on cursor (* mark); Esc clears set",
        },
        KeybindEntry {
            keys: "r",
            description: "Refresh / rescan workspace roots",
        },
        KeybindEntry {
            keys: "b",
            description: "Gradle build — cursor, or all multi-selected",
        },
        KeybindEntry {
            keys: "B",
            description: "Publish → rebuild dependents (confirm plan; SNAPSHOT refresh)",
        },
        KeybindEntry {
            keys: "c",
            description: "Gradle clean — cursor, or all multi-selected",
        },
        KeybindEntry {
            keys: "p",
            description: "Gradle publish (cursor project only)",
        },
        KeybindEntry {
            keys: "G",
            description: "Git pull (ff-only) — cursor, or unique git roots among multi",
        },
        KeybindEntry {
            keys: "d / D",
            description: "Skaffold dev / debug (-f skaffold file; cursor only)",
        },
        KeybindEntry {
            keys: "x / u",
            description: "Skaffold delete / run (cursor only)",
        },
        KeybindEntry {
            keys: "P",
            description: "Cascade publish → rebuild + skaffold redeploy consumers",
        },
        KeybindEntry {
            keys: "K",
            description: "Probe cluster Deployments for deployed versions ([kube] config)",
        },
        KeybindEntry {
            keys: "f",
            description: "Toggle drift-only filter (local▲ / cluster▲ / unknown)",
        },
        KeybindEntry {
            keys: "-",
            description: "Exclude cursor (or multi-selected) from inventory → [scan].exclude",
        },
        KeybindEntry {
            keys: "Enter",
            description: "Open full-screen project detail (deps + actions)",
        },
        KeybindEntry {
            keys: "2",
            description: "Open Jobs console (live logs)",
        },
        KeybindEntry {
            keys: "/",
            description: "Filter projects — later milestone",
        },
    ],
};

const JOBS: KeybindSection = KeybindSection {
    title: "Jobs",
    entries: &[
        KeybindEntry {
            keys: "Esc",
            description: "Cancel focused job (when running)",
        },
        KeybindEntry {
            keys: "Tab",
            description: "Focus list vs log",
        },
    ],
};

const WORKSPACE: KeybindSection = KeybindSection {
    title: "Workspace",
    entries: &[
        KeybindEntry {
            keys: "Tab",
            description: "Focus scan roots vs project excludes",
        },
        KeybindEntry {
            keys: "n",
            description: "Add root or exclude (depends on focus)",
        },
        KeybindEntry {
            keys: "e",
            description: "Edit selected root / exclude pattern",
        },
        KeybindEntry {
            keys: "z",
            description: "Delete selected root / exclude (confirm)",
        },
        KeybindEntry {
            keys: "j/k · ↑/↓",
            description: "Move selection in focused list",
        },
        KeybindEntry {
            keys: "r",
            description: "Rescan configured roots (applies excludes)",
        },
        KeybindEntry {
            keys: "Enter",
            description: "Open project browser (or save while editing)",
        },
        KeybindEntry {
            keys: "Esc",
            description: "Cancel editor / back to projects",
        },
        KeybindEntry {
            keys: "1",
            description: "Jump to project browser",
        },
    ],
};

/// Sections shown for the current screen (global always first).
pub fn sections_for(app: &App) -> Vec<&'static KeybindSection> {
    let mut sections = vec![&GLOBAL];
    match app.screen {
        Screen::ProjectBrowser => sections.push(&PROJECT_BROWSER),
        Screen::Jobs => sections.push(&JOBS),
        Screen::Workspace => sections.push(&WORKSPACE),
        Screen::Settings => sections.push(&SETTINGS),
    }
    sections
}

/// Full-screen dimmed overlay listing keybinds for the active screen.
pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let dialog = centered_rect(78, 80, area);
    frame.render_widget(Clear, dialog);

    let theme = &app.theme;
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(
            " Help — {}  (theme: {}) ",
            screen_label(app.screen),
            theme.name.label()
        ))
        .title_style(theme.title)
        .border_style(theme.border)
        .style(theme.root_style().bg(theme.bg_panel));
    let inner = block.inner(dialog);
    frame.render_widget(block, dialog);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    let mut lines: Vec<Line> = Vec::new();
    for section in sections_for(app) {
        lines.push(Line::from(Span::styled(
            section.title,
            theme.title.add_modifier(Modifier::UNDERLINED),
        )));
        for entry in section.entries {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {:<18}", entry.keys),
                    theme.secondary.add_modifier(Modifier::BOLD),
                ),
                Span::styled(entry.description, theme.dim),
            ]));
        }
        lines.push(Line::from(""));
    }

    frame.render_widget(
        Paragraph::new(lines)
            .style(theme.text)
            .wrap(Wrap { trim: false }),
        chunks[0],
    );
    frame.render_widget(
        Paragraph::new("?: close   Esc: close").style(theme.secondary),
        chunks[1],
    );
}

fn screen_label(screen: Screen) -> &'static str {
    match screen {
        Screen::ProjectBrowser => "Projects",
        Screen::Jobs => "Jobs",
        Screen::Workspace => "Workspace",
        Screen::Settings => "Settings",
    }
}
