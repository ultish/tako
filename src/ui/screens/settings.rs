//! Settings screen — edit all config.toml fields (except roots/excludes on Workspace).

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::settings::{SettingId, SettingKind, SettingsEditorState};
use crate::app::App;
use crate::events::Action;
use crate::ui::widgets::confirm_dialog::centered_rect;
use crate::ui::widgets::footer::{
    render_keybind_footer, render_status_bar, split_with_footer,
};
use crate::ui::widgets::table_nav::render_selectable_list;

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let (main, status_bar, footer) = split_with_footer(area);

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
        .split(main);

    render_settings_list(frame, app, chunks[0]);
    render_settings_detail(frame, app, chunks[1]);

    let footer_text = if app.settings_editor.is_some() {
        "type value   Enter: save   Esc: cancel"
    } else {
        "j/k: move   Enter/Space: edit/toggle   ←/→: nudge ints   1–3: other tabs   ?: help   q: quit"
    };
    render_status_bar(frame, app, status_bar);
    render_keybind_footer(frame, footer, &app.theme, footer_text);

    if let Some(ed) = app.settings_editor.as_ref() {
        render_settings_editor(frame, app, area, ed);
    }
}

fn render_settings_list(frame: &mut Frame, app: &App, area: Rect) {
    let mut items: Vec<Vec<String>> = Vec::new();
    let mut last_section = "";
    for id in SettingId::ALL {
        let section = id.section();
        if section != last_section {
            // Section header as a non-selectable visual row — still in list for simplicity;
            // we only select real settings via selected_setting index into ALL.
            last_section = section;
        }
        items.push(vec![
            format!("[{section}]"),
            id.key().to_string(),
            app.setting_display_value(*id),
        ]);
    }

    let selected = app.selected_setting.min(items.len().saturating_sub(1));
    render_selectable_list(
        frame,
        app,
        area,
        " Settings · config.toml ",
        &items,
        Some(&["Section", "Key", "Value"]),
        selected,
        true,
    );

    // Click rows
    register_setting_clicks(app, area, items.len());
}

fn register_setting_clicks(app: &App, list_area: Rect, count: usize) {
    if count == 0 || list_area.height < 3 {
        return;
    }
    let header_offset = 2u16;
    let data_top = list_area.y.saturating_add(header_offset);
    let data_bottom = list_area
        .y
        .saturating_add(list_area.height)
        .saturating_sub(1);
    for i in 0..count {
        let y = data_top.saturating_add(i as u16);
        if y >= data_bottom {
            break;
        }
        app.register_click(
            list_area.x,
            y,
            list_area.width,
            1,
            Action::SelectSettingRow(i),
        );
    }
}

fn render_settings_detail(frame: &mut Frame, app: &App, area: Rect) {
    let Some(&id) = SettingId::ALL.get(app.selected_setting) else {
        return;
    };
    let value = app.setting_display_value(id);
    let action = match id.kind() {
        SettingKind::Bool => "Enter/Space: toggle",
        SettingKind::Cycle => "Enter/Space: cycle",
        SettingKind::Int => "Enter: edit · ←/→ nudge",
        SettingKind::Text => "Enter: edit text",
    };

    let lines = vec![
        Line::from(Span::styled(
            format!("[{}] {}", id.section(), id.key()),
            app.theme.accent.add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("Value  ", app.theme.dim),
            Span::styled(value, app.theme.text.add_modifier(Modifier::BOLD)),
        ]),
        Line::from(""),
        Line::from(Span::styled(id.help(), app.theme.status)),
        Line::from(""),
        Line::from(Span::styled(action, app.theme.secondary)),
        Line::from(""),
        Line::from(Span::styled(
            "Roots & project excludes live on Workspace (3).",
            app.theme.dim,
        )),
        Line::from(Span::styled(
            "Saves immediately to config.toml.",
            app.theme.dim,
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("File: {}", app.config_path.display()),
            app.theme.dim,
        )),
    ];

    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Detail ")
                    .title_style(app.theme.title)
                    .border_style(app.theme.border)
                    .style(app.theme.root_style()),
            ),
        area,
    );
}

fn render_settings_editor(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    ed: &SettingsEditorState,
) {
    let dialog = centered_rect(72, 42, area);
    frame.render_widget(Clear, dialog);

    let title = format!(" Edit {}.{} ", ed.id.section(), ed.id.key());
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
            Constraint::Length(2),
            Constraint::Length(3),
            Constraint::Min(2),
            Constraint::Length(1),
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new(ed.id.help())
            .style(app.theme.status)
            .wrap(Wrap { trim: true }),
        chunks[0],
    );

    frame.render_widget(
        Paragraph::new(ed.display_with_cursor())
            .style(app.theme.text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Value ")
                    .title_style(app.theme.focus_title(true))
                    .border_style(app.theme.focus_border(true)),
            ),
        chunks[1],
    );

    if let Some(err) = &ed.error {
        frame.render_widget(
            Paragraph::new(err.as_str())
                .style(app.theme.error.add_modifier(Modifier::BOLD)),
            chunks[2],
        );
    } else {
        frame.render_widget(
            Paragraph::new("Lists: comma-separated (e.g. dev, staging). Empty clears optional fields.")
                .style(app.theme.dim)
                .wrap(Wrap { trim: true }),
            chunks[2],
        );
    }

    frame.render_widget(
        Paragraph::new("Enter: save     Esc: cancel")
            .style(app.theme.secondary.add_modifier(Modifier::BOLD))
            .alignment(Alignment::Center),
        chunks[3],
    );
}
