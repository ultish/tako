//! Startup splash: truecolor half-block octopus when the terminal supports it,
//! otherwise a braille silhouette. Dismiss with any key.
//!
//! Art is the full `assets/tako.jpeg` (no face crop). Prefer filling the art
//! pane — scale **up** to available width (soft cap ~120 cols), not a modest
//! fixed comfort width like rakko's 72.

use std::sync::OnceLock;

use crossterm::style::available_color_count;
use image::imageops::FilterType;
use image::RgbaImage;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

/// Soft upper bound on half-block width. Prefer filling the art pane; this only
/// caps extremely wide terminals so decode/resize stays cheap.
const TRUECOLOR_MAX_COLS: u32 = 120;

/// Embedded square octopus art (`assets/tako.jpeg`, ~447×447).
const TAKO_JPEG: &[u8] = include_bytes!("../../assets/tako.jpeg");

/// Truecolor support: env heuristic via crossterm (COLORTERM / TERM).
pub fn supports_truecolor() -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    if let Ok(v) = std::env::var("TAKO_TRUECOLOR") {
        match v.as_str() {
            "0" | "false" | "off" | "no" => return false,
            "1" | "true" | "on" | "yes" => return true,
            _ => {}
        }
    }
    available_color_count() == u16::MAX
}

/// Cached decoded **full-frame** image (no otter-style face crop).
fn tako_rgba() -> &'static RgbaImage {
    static IMG: OnceLock<RgbaImage> = OnceLock::new();
    IMG.get_or_init(|| {
        image::load_from_memory(TAKO_JPEG)
            .expect("embedded tako.jpeg must decode")
            .to_rgba8()
    })
}

/// Build half-block lines: each `▀` cell has fg = top pixel, bg = bottom pixel.
///
/// Scaling: prefer `max_cols` (the inner art pane width), clamped to
/// [`TRUECOLOR_MAX_COLS`]. Scale down only when height does not fit.
fn halfblock_lines(max_cols: u16, max_rows: u16) -> Vec<Line<'static>> {
    let src = tako_rgba();
    let (sw, sh) = src.dimensions();

    // Prefer filling the splash art pane — large by default.
    let mut cols = (max_cols as u32).clamp(8, TRUECOLOR_MAX_COLS) as u16;
    cols = cols.min(max_cols.max(8));

    // height in pixels must be even (two per cell row)
    let mut pix_h = ((sh as f32) * (cols as f32) / (sw as f32)).round() as u32;
    if pix_h < 2 {
        pix_h = 2;
    }
    if pix_h % 2 == 1 {
        pix_h += 1;
    }
    let mut rows = (pix_h / 2) as u16;
    if rows > max_rows && max_rows >= 4 {
        // Scale down to fit terminal height.
        rows = max_rows;
        pix_h = rows as u32 * 2;
        cols = ((sw as f32) * (pix_h as f32) / (sh as f32)).round() as u16;
        cols = cols.clamp(8, max_cols.max(8));
        // recompute height for new width to keep aspect
        pix_h = ((sh as f32) * (cols as f32) / (sw as f32)).round() as u32;
        if pix_h % 2 == 1 {
            pix_h += 1;
        }
        if (pix_h / 2) as u16 > max_rows {
            pix_h = max_rows as u32 * 2;
        }
        rows = (pix_h / 2) as u16;
    }

    let resized = image::imageops::resize(src, cols as u32, pix_h, FilterType::Triangle);

    let mut lines = Vec::with_capacity(rows as usize);
    for r in 0..rows {
        let mut spans = Vec::with_capacity(cols as usize);
        for c in 0..cols {
            let top = resized.get_pixel(c as u32, r as u32 * 2).0;
            let bot = resized.get_pixel(c as u32, r as u32 * 2 + 1).0;
            let fg = Color::Rgb(top[0], top[1], top[2]);
            let bg = Color::Rgb(bot[0], bot[1], bot[2]);
            spans.push(Span::styled("▀", Style::default().fg(fg).bg(bg)));
        }
        lines.push(Line::from(spans));
    }
    lines
}

pub fn render(frame: &mut Frame, app: &crate::app::App, area: Rect) {
    // Do **not** Clear here — `ui::draw` already filled `area` with the theme
    // background. `Clear` resets cells to the terminal default and undoes that.

    let root = app.theme.root_style();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(3),
        ])
        .split(area);

    let mode = if supports_truecolor() {
        "truecolor"
    } else {
        "braille"
    };

    let title = Paragraph::new(Line::from(vec![
        Span::styled(" tako ", app.theme.accent),
        Span::styled("蛸", app.theme.secondary),
        Span::styled(
            " — multi-service control plane",
            app.theme.dim,
        ),
        Span::styled(format!("  [{mode}]"), app.theme.dim),
    ]))
    .style(root)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title("welcome")
            .title_style(app.theme.title)
            .border_style(app.theme.border)
            .style(root),
    );
    frame.render_widget(title, chunks[0]);

    let art_block = Block::default()
        .borders(Borders::ALL)
        .border_style(app.theme.border)
        .style(root);
    let inner = art_block.inner(chunks[1]);
    frame.render_widget(art_block, chunks[1]);

    if supports_truecolor() && inner.width >= 16 && inner.height >= 6 {
        let lines = halfblock_lines(inner.width, inner.height);
        // Center horizontally by padding if narrower than area
        let art_w = lines.first().map(|l| l.width() as u16).unwrap_or(0);
        let pad = inner.width.saturating_sub(art_w) / 2;
        let art_area = Rect {
            x: inner.x + pad,
            y: inner.y,
            width: art_w.min(inner.width),
            height: (lines.len() as u16).min(inner.height),
        };
        // Fill the art pane with theme bg so letterboxing stays near-black.
        frame.render_widget(Block::default().style(root), inner);
        frame.render_widget(Paragraph::new(lines), art_area);
    } else {
        frame.render_widget(
            Paragraph::new(TAKO_BRAILLE)
                .alignment(Alignment::Center)
                .style(
                    app.theme
                        .secondary
                        .add_modifier(Modifier::BOLD)
                        .bg(app.theme.bg),
                ),
            inner,
        );
    }

    let hint = Paragraph::new("Press Enter / Space / Esc to continue")
        .alignment(Alignment::Center)
        .style(app.theme.status)
        .wrap(Wrap { trim: true })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(app.theme.border)
                .style(root),
        );
    frame.render_widget(hint, chunks[2]);
}

/// Braille fallback octopus silhouette + brand label.
const TAKO_BRAILLE: &str = "\
⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢀⣀⣀⡀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀
⠀⠀⠀⠀⠀⠀⠀⢀⣴⣾⣿⣿⣿⣿⣷⣦⡀⠀⠀⠀⠀⠀⠀⠀
⠀⠀⠀⠀⠀⠀⣰⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣆⠀⠀⠀⠀⠀⠀
⠀⠀⠀⠀⠀⢰⣿⣿⣿⡿⠟⠛⠛⠻⢿⣿⣿⣿⡆⠀⠀⠀⠀⠀
⠀⠀⠀⠀⠀⣿⣿⣿⡏⠀⢀⣤⣤⡀⠀⢹⣿⣿⣿⠀⠀⠀⠀⠀
⠀⠀⠀⠀⠀⣿⣿⣿⡇⠀⠘⠿⠿⠃⠀⢸⣿⣿⣿⠀⠀⠀⠀⠀
⠀⠀⠀⠀⠀⠸⣿⣿⣷⣄⠀⠀⠀⢀⣠⣾⣿⣿⠇⠀⠀⠀⠀⠀
⠀⠀⢀⣀⣀⣀⣈⣿⣿⣿⣿⣶⣾⣿⣿⣿⣿⣁⣀⣀⣀⡀⠀⠀
⠀⣰⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣆⠀
⢠⣿⣿⡿⠋⠁⠈⠙⢿⣿⣿⣿⣿⣿⡿⠋⠁⠈⠙⢿⣿⣿⡄
⣿⣿⡟⠀⠀⠀⠀⠀⠀⠹⣿⣿⣿⠏⠀⠀⠀⠀⠀⠀⢻⣿⣿
⠈⠻⠀⠀⠀⠀⠀⠀⠀⠀⠈⠻⠟⠁⠀⠀⠀⠀⠀⠀⠀⠀⠟⠁

          ~ tako 蛸 ~";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_tako_decodes() {
        assert!(tako_rgba().width() > 0);
        assert!(tako_rgba().height() > 0);
        // Square source; full frame, not a face crop.
        assert_eq!(tako_rgba().width(), tako_rgba().height());
    }

    #[test]
    fn halfblock_prefers_large_width() {
        // On a wide pane, use most of the width (not a hard 72-col comfort size).
        let lines = halfblock_lines(100, 50);
        assert!(!lines.is_empty());
        let w = lines.first().map(|l| l.width()).unwrap_or(0);
        assert!(
            w >= 90,
            "expected large art width (>=90), got {w}"
        );
    }

    #[test]
    fn halfblock_respects_height() {
        let lines = halfblock_lines(120, 10);
        assert!(!lines.is_empty());
        assert!(lines.len() <= 10);
    }

    #[test]
    fn braille_fallback_has_label() {
        assert!(TAKO_BRAILLE.contains("tako 蛸"));
    }
}
