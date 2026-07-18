//! Global help overlay (`?`): per-screen keybind tables + workflow flowcharts.
//!
//! Opens on **Keys**. **Tab** (or ←/→) switches to **Workflows** and back.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, HelpPage, Screen};
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
            description: "Toggle this help (opens on Keys)",
        },
        KeybindEntry {
            keys: "Tab",
            description: "Switch Keys ↔ Workflows (while help open)",
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
            keys: "A / T",
            description: "Cycle banner / theme",
        },
        KeybindEntry {
            keys: "j/k · Enter · Esc",
            description: "Move / open / back",
        },
        KeybindEntry {
            keys: "1/2/3/4",
            description: "Projects / Jobs / Workspace / Settings",
        },
        KeybindEntry {
            keys: "w",
            description: "Rescan workspace roots",
        },
    ],
};

const SETTINGS: KeybindSection = KeybindSection {
    title: "Settings",
    entries: &[
        KeybindEntry {
            keys: "j/k · Enter · ←/→",
            description: "Move / edit / nudge ints",
        },
    ],
};

const PROJECT_BROWSER: KeybindSection = KeybindSection {
    title: "Project browser",
    entries: &[
        KeybindEntry {
            keys: "— Shared —",
            description: "",
        },
        KeybindEntry {
            keys: "b / B",
            description: "Build / build with latest SNAPSHOT (no stale cache)",
        },
        KeybindEntry {
            keys: "c",
            description: "Clean",
        },
        KeybindEntry {
            keys: "p",
            description: "Publish to Nexus",
        },
        KeybindEntry {
            keys: "v / V",
            description: "Bump this version / bump all dependent versions",
        },
        KeybindEntry {
            keys: "G",
            description: "Git pull --ff-only",
        },
        KeybindEntry {
            keys: "r",
            description: "Refresh stats: git fetch+lag, Nexus versions, kube if enabled",
        },
        KeybindEntry {
            keys: "w",
            description: "Rescan workspace",
        },
        KeybindEntry {
            keys: "— Lib —",
            description: "",
        },
        KeybindEntry {
            keys: "U",
            description: "Update dependents — Nexus check libs, then services: pull/clean/B/delete→run",
        },
        KeybindEntry {
            keys: "i",
            description: "Who needs this? (dependent tree)",
        },
        KeybindEntry {
            keys: "— Service —",
            description: "",
        },
        KeybindEntry {
            keys: "u / x",
            description: "Skaffold delete→run / delete only",
        },
        KeybindEntry {
            keys: "K",
            description: "Probe cluster deployed versions",
        },
        KeybindEntry {
            keys: "— List —",
            description: "",
        },
        KeybindEntry {
            keys: "Space / m",
            description: "Toggle multi-select on cursor (● mark) · then b/B/c/G bulk",
        },
        KeybindEntry {
            keys: "- / F / /",
            description: "Hide · favorite · filter",
        },
    ],
};

const JOBS: KeybindSection = KeybindSection {
    title: "Jobs",
    entries: &[
        KeybindEntry {
            keys: "Esc",
            description: "Cancel focused job",
        },
        KeybindEntry {
            keys: "Tab · click",
            description: "Focus list vs log",
        },
    ],
};

const WORKSPACE: KeybindSection = KeybindSection {
    title: "Workspace",
    entries: &[
        KeybindEntry {
            keys: "Tab · click",
            description: "Focus roots vs excludes",
        },
        KeybindEntry {
            keys: "n / e / z",
            description: "Add / edit / delete",
        },
        KeybindEntry {
            keys: "w",
            description: "Rescan roots",
        },
    ],
};

/// Sections for the Keys page (global + current screen).
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

/// Full-screen dimmed overlay: workflows flowchart or keybind tables.
pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let dialog = centered_rect(90, 88, area);
    frame.render_widget(Clear, dialog);

    let theme = &app.theme;
    let page = app.help_page.label();
    let other = app.help_page.other().label();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(
            " Help · {page}  (Tab → {other}) · theme: {} ",
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

    let lines = match app.help_page {
        HelpPage::Workflows => workflow_lines(app),
        HelpPage::Keys => keybind_lines(app),
    };

    frame.render_widget(
        Paragraph::new(lines)
            .style(theme.text)
            .wrap(Wrap { trim: false }),
        chunks[0],
    );
    frame.render_widget(
        Paragraph::new("Tab: switch page   ?/Esc: close").style(theme.secondary),
        chunks[1],
    );
}

fn keybind_lines(app: &App) -> Vec<Line<'static>> {
    let theme_secondary = app.theme.secondary;
    let theme_title = app.theme.title;
    let theme_dim = app.theme.dim;

    let mut lines: Vec<Line> = Vec::new();
    for section in sections_for(app) {
        lines.push(Line::from(Span::styled(
            section.title,
            theme_title.add_modifier(Modifier::UNDERLINED),
        )));
        for entry in section.entries {
            if entry.description.is_empty() {
                lines.push(Line::from(Span::styled(
                    format!("  {}", entry.keys),
                    theme_dim.add_modifier(Modifier::BOLD),
                )));
                continue;
            }
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {:<20}", entry.keys),
                    theme_secondary.add_modifier(Modifier::BOLD),
                ),
                Span::styled(entry.description, theme_dim),
            ]));
        }
        lines.push(Line::from(""));
    }
    lines
}

fn workflow_lines(app: &App) -> Vec<Line<'static>> {
    let t = app.theme.title.add_modifier(Modifier::BOLD);
    let s = app.theme.secondary.add_modifier(Modifier::BOLD);
    let d = app.theme.dim;
    let w = app.theme.warning;
    let ok = app.theme.success;

    let mut lines = Vec::new();

    lines.push(Line::from(Span::styled(
        "Libs publish to Nexus. Services resolve from Nexus, then skaffold.",
        d,
    )));
    lines.push(Line::from(""));

    // ── Someone else updated ───────────────────────────────────────────
    lines.push(Line::from(Span::styled(
        "1) Someone else already published (catch-up)",
        t,
    )));
    lines.push(Line::from(Span::styled(
        "   Parent lib + child libs are on Nexus. You only refresh services.",
        d,
    )));
    lines.push(Line::from(""));
    for row in [
        "   select lib",
        "       │",
        "       ▼",
        "   ┌─────────────────────────────────────────┐",
        "   │  U  Update dependents                   │",
        "   │  ① libs in tree → check Nexus vs cache  │",
        "   │     (warn if missing / not newer)       │",
        "   │  ② each SERVICE:                        │",
        "   │     git pull → clean → B (no cache)     │",
        "   │     → skaffold delete → run             │",
        "   │     (skip skaffold if Argo)             │",
        "   └─────────────────────────────────────────┘",
    ] {
        let style = if row.contains('U') {
            s
        } else if row.contains('①') || row.contains('②') {
            w
        } else {
            d
        };
        lines.push(Line::from(Span::styled(row, style)));
    }
    lines.push(Line::from(""));

    // ── You own the change ─────────────────────────────────────────────
    lines.push(Line::from(Span::styled(
        "2) You are changing the libs (you publish)",
        t,
    )));
    lines.push(Line::from(""));
    for row in [
        "   select root lib",
        "       │",
        "       ▼",
        "   V  bump versions on child libs + services  (manual plan, y)",
        "       │",
        "       ▼",
        "   on each lib you own (parent, then children):",
        "       b/B build  →  p publish to Nexus",
        "       │",
        "       ▼",
        "   U  same as (1) — Nexus check, then services pull/clean/B/delete→run",
    ] {
        let style = if row.starts_with("   V") || row.contains(" U ") || row.starts_with("   U")
        {
            s
        } else if row.contains("p publish") || row.contains("b/B") {
            ok
        } else {
            d
        };
        lines.push(Line::from(Span::styled(row, style)));
    }
    lines.push(Line::from(""));

    // ── Single service ─────────────────────────────────────────────────
    lines.push(Line::from(Span::styled(
        "3) Just this service",
        t,
    )));
    lines.push(Line::from(""));
    for row in [
        "   select service",
        "       │",
        "       ▼",
        "   B  build no-cache (optional)   →   u  skaffold delete→run",
        "   v  bump this version (optional, before deploy)",
        "   p  publish only if you ship the service jar (rare)",
    ] {
        let style = if row.contains(" B ") || row.contains(" u ") {
            s
        } else {
            d
        };
        lines.push(Line::from(Span::styled(row, style)));
    }
    lines.push(Line::from(""));

    // ── Cheat sheet ────────────────────────────────────────────────────
    lines.push(Line::from(Span::styled("Quick map", t)));
    lines.push(Line::from(vec![
        Span::styled("   U  ", s),
        Span::styled("catch-up services (Nexus already right)", d),
    ]));
    lines.push(Line::from(vec![
        Span::styled("   V  ", s),
        Span::styled("bump dependent versions in-repo", d),
    ]));
    lines.push(Line::from(vec![
        Span::styled("   p  ", s),
        Span::styled("publish this project to Nexus", d),
    ]));
    lines.push(Line::from(vec![
        Span::styled("   B  ", s),
        Span::styled("force latest SNAPSHOT resolve + build", d),
    ]));
    lines.push(Line::from(vec![
        Span::styled("   u  ", s),
        Span::styled("this service only: delete → run", d),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "U on a service with nothing depending on it → status only (use B then u).",
        w,
    )));

    lines
}

