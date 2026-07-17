use std::collections::HashSet;

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::widgets::{Block, Borders, Cell, Row, Table, TableState};
use ratatui::Frame;

use crate::app::App;
use crate::events::Action;

/// Optional per-row style tints (by list / display index).
///
/// Applied to non-selected rows after paint; selection and hover still win.
#[derive(Debug, Clone, Default)]
pub struct RowHighlights {
    /// Dependency rows — typically theme `secondary` (cyan).
    pub secondary: HashSet<usize>,
    /// Dependent / consumer rows — typically theme `warning`.
    pub warning: HashSet<usize>,
}

/// Matches `Table` default spacing between columns.
const COLUMN_SPACING: u16 = 1;
/// Matches our highlight symbol (`"> "`).
const HIGHLIGHT_SYMBOL: &str = "> ";

/// How many data rows fit in `area` for a table built by [`render_selectable_list`]
/// (borders + optional header).
pub fn selectable_list_viewport_rows(area: Rect, has_header: bool) -> usize {
    let inner = Block::default().borders(Borders::ALL).inner(area);
    let header_h = u16::from(has_header);
    inner.height.saturating_sub(header_h) as usize
}

/// Scroll offset ratatui's `Table` derives when `TableState` starts at offset 0
/// and then keeps `selected` in view.
pub fn selectable_list_offset(selected: usize, item_count: usize, viewport_rows: usize) -> usize {
    if viewport_rows == 0 || item_count == 0 {
        return 0;
    }
    let max_offset = item_count.saturating_sub(viewport_rows);
    if selected < viewport_rows {
        0
    } else {
        (selected + 1).saturating_sub(viewport_rows).min(max_offset)
    }
}

/// Renders `items` as a selectable table with the row at `selected` highlighted.
///
/// Column widths are content-aware for metadata columns; one primary column
/// uses `Fill` so it expands with the terminal/area width.
#[allow(clippy::too_many_arguments)]
pub fn render_selectable_list(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    title: &str,
    items: &[Vec<String>],
    header: Option<&[&str]>,
    selected: usize,
    selectable: bool,
) {
    render_selectable_list_with_highlights(
        frame,
        app,
        area,
        title,
        items,
        header,
        selected,
        selectable,
        None,
    );
}

/// Like [`render_selectable_list`], with optional dep/dependent row tints.
#[allow(clippy::too_many_arguments)]
pub fn render_selectable_list_with_highlights(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    title: &str,
    items: &[Vec<String>],
    header: Option<&[&str]>,
    selected: usize,
    selectable: bool,
    highlights: Option<&RowHighlights>,
) {
    let column_count = header
        .map(<[&str]>::len)
        .or_else(|| items.first().map(Vec::len))
        .unwrap_or(1)
        .max(1);

    let constraints = auto_column_widths(column_count, header, items);
    let highlight_w = HIGHLIGHT_SYMBOL.chars().count() as u16;
    let available = area
        .width
        .saturating_sub(2) // block borders
        .saturating_sub(highlight_w);
    let col_widths = resolve_column_widths(&constraints, available, COLUMN_SPACING);

    let rows = items.iter().map(|row| {
        let cells: Vec<Cell> = (0..column_count)
            .map(|i| {
                let raw = row.get(i).map(String::as_str).unwrap_or("");
                let max = col_widths.get(i).copied().unwrap_or(0) as usize;
                Cell::new(truncate_to_width(raw, max))
            })
            .collect();
        Row::new(cells)
    });

    let header_row = header.map(|header_cells| {
        let cells: Vec<Cell> = header_cells
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let max = col_widths.get(i).copied().unwrap_or(0) as usize;
                Cell::new(truncate_to_width(name, max))
            })
            .collect();
        Row::new(cells).style(app.theme.title)
    });

    let mut table = Table::new(rows, constraints)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .title_style(app.theme.title)
                .border_style(app.theme.border)
                .style(app.theme.root_style()),
        )
        .column_spacing(COLUMN_SPACING)
        .style(app.theme.text)
        .row_highlight_style(app.theme.selected_row)
        .highlight_symbol(HIGHLIGHT_SYMBOL);

    if let Some(header_row) = header_row {
        table = table.header(header_row);
    }

    let mut state = TableState::default();
    let clamped_selected = (!items.is_empty()).then(|| selected.min(items.len() - 1));
    state.select(clamped_selected);

    frame.render_stateful_widget(table, area, &mut state);

    if selectable && !items.is_empty() {
        register_row_interactions(
            frame,
            app,
            area,
            header.is_some(),
            items.len(),
            state.offset(),
            clamped_selected.unwrap_or(usize::MAX),
            highlights,
        );
    }
}

fn register_row_interactions(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    has_header: bool,
    item_count: usize,
    offset: usize,
    selected: usize,
    highlights: Option<&RowHighlights>,
) {
    let inner = Block::default().borders(Borders::ALL).inner(area);
    let header_h = u16::from(has_header);
    let rows_y = inner.y + header_h;
    let rows_h = inner.height.saturating_sub(header_h);
    let visible = (rows_h as usize).min(item_count.saturating_sub(offset));
    for i in 0..visible {
        let row_index = offset + i;
        let y = rows_y + i as u16;
        app.register_click(inner.x, y, inner.width, 1, Action::SelectRow(row_index));

        let row_rect = Rect {
            x: inner.x,
            y,
            width: inner.width,
            height: 1,
        };

        // Graph tints first; hover and selection override.
        if row_index != selected {
            if let Some(h) = highlights {
                let style: Option<Style> = if h.warning.contains(&row_index) {
                    Some(app.theme.warning)
                } else if h.secondary.contains(&row_index) {
                    Some(app.theme.secondary)
                } else {
                    None
                };
                if let Some(style) = style {
                    frame.buffer_mut().set_style(row_rect, style);
                }
            }
        }

        if row_index != selected && app.is_hovered(inner.x, y, inner.width, 1) {
            frame.buffer_mut().set_style(row_rect, app.theme.hover_row);
        }
    }
}

fn resolve_column_widths(
    constraints: &[Constraint],
    available: u16,
    spacing: u16,
) -> Vec<u16> {
    let n = constraints.len();
    if n == 0 || available == 0 {
        return vec![0; n];
    }
    let spacing_total = spacing.saturating_mul((n as u16).saturating_sub(1));
    let inner = available.saturating_sub(spacing_total).max(1);
    Layout::horizontal(constraints.to_vec())
        .split(Rect {
            x: 0,
            y: 0,
            width: inner,
            height: 1,
        })
        .iter()
        .map(|r| r.width)
        .collect()
}

fn truncate_to_width(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    if max == 1 {
        return "…".to_string();
    }
    let truncated: String = text.chars().take(max - 1).collect();
    format!("{truncated}…")
}

fn auto_column_widths(
    column_count: usize,
    header: Option<&[&str]>,
    items: &[Vec<String>],
) -> Vec<Constraint> {
    if column_count == 1 {
        return vec![Constraint::Fill(1)];
    }

    let mut max_w = vec![0usize; column_count];
    if let Some(headers) = header {
        for (i, name) in headers.iter().enumerate().take(column_count) {
            max_w[i] = max_w[i].max(name.chars().count());
        }
    }
    for row in items {
        for (i, cell) in row.iter().enumerate().take(column_count) {
            let sample = cell.chars().count().min(48);
            max_w[i] = max_w[i].max(sample);
        }
    }

    let fill_idx = fill_column_index(column_count, header);

    let mut constraints = Vec::with_capacity(column_count);
    for (i, &content_w) in max_w.iter().enumerate() {
        if i == fill_idx {
            constraints.push(Constraint::Fill(1));
            continue;
        }
        let cap = column_cap(i, header);
        let width = (content_w + 1).clamp(3, cap) as u16;
        constraints.push(Constraint::Length(width));
    }
    constraints
}

fn fill_column_index(column_count: usize, header: Option<&[&str]>) -> usize {
    const PREFERENCE: &[&str] = &["name", "group / name", "project", "path", "step", "status"];
    if let Some(headers) = header {
        for pref in PREFERENCE {
            if let Some(i) = headers
                .iter()
                .position(|h| h.eq_ignore_ascii_case(pref))
            {
                if i < column_count {
                    return i;
                }
            }
        }
    }
    0
}

fn column_cap(index: usize, header: Option<&[&str]>) -> usize {
    if let Some(headers) = header {
        if let Some(name) = headers.get(index) {
            let lower = name.to_ascii_lowercase();
            return match lower.as_str() {
                "kind" => 10,
                "ver" | "version" | "local" | "deployed" => 12,
                "branch" => 18,
                "sk" | "git" => 4,
                "status" => 20,
                "drift" => 10,
                _ => 24,
            };
        }
    }
    24
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selectable_list_offset_stays_zero_while_selection_fits() {
        assert_eq!(selectable_list_offset(0, 100, 20), 0);
        assert_eq!(selectable_list_offset(19, 100, 20), 0);
    }

    #[test]
    fn selectable_list_offset_scrolls_to_keep_selection_visible() {
        assert_eq!(selectable_list_offset(20, 100, 20), 1);
        assert_eq!(selectable_list_offset(50, 100, 20), 31);
        assert_eq!(selectable_list_offset(99, 100, 20), 80);
    }

    #[test]
    fn project_list_name_column_fills() {
        let header = ["Name", "Kind", "Ver", "Branch", "Skaffold", "Git", "Status"];
        let items = vec![vec![
            "payments-api".into(),
            "service".into(),
            "3.2.0".into(),
            "feature/x".into(),
            "●".into(),
            "·".into(),
            "idle".into(),
        ]];
        let widths = auto_column_widths(7, Some(&header), &items);
        assert!(matches!(widths[0], Constraint::Fill(1)));
        assert!(matches!(widths[1], Constraint::Length(_)));
    }

    #[test]
    fn truncate_uses_full_width_and_ellipsis() {
        assert_eq!(truncate_to_width("hello", 10), "hello");
        assert_eq!(truncate_to_width("hello world", 8), "hello w…");
        assert_eq!(truncate_to_width("ab", 1), "…");
    }
}
