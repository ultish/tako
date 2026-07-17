//! Slim top banner: app name + a braille strip that's either a decorative wave, a
//! live frame-cost graph (`ms/frame`), or off (`A` cycles wave → ms → fps → off).
//! Detailed octopus art is splash-only.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::{App, BannerMode};
use crate::ring_buffer::RingBuffer;

/// One content line + bottom border.
pub const BANNER_HEIGHT: u16 = 3;

/// Short strip — same scale as the original animation.
const WAVE_WIDTH: usize = 9;

/// Density ladder (height): empty-ish → full.
const LEVELS: &[char] = &['⡀', '⣀', '⣄', '⣤', '⣦', '⣶', '⣷', '⣿'];

/// Columns between wave peak/trough keyframes.
const WAVE_PERIOD: usize = 6;

fn keyframe_height(k: usize) -> f64 {
    let mut h = k.wrapping_mul(0x9E37_79B9);
    h ^= h >> 16;
    h = h.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 13;
    (h % 1000) as f64 / 999.0 * (LEVELS.len() - 1) as f64
}

fn smoothstep(t: f64) -> f64 {
    t * t * (3.0 - 2.0 * t)
}

fn height_at(x: usize) -> usize {
    let k0 = x / WAVE_PERIOD;
    let k1 = k0 + 1;
    let t = (x % WAVE_PERIOD) as f64 / WAVE_PERIOD as f64;
    let h0 = keyframe_height(k0);
    let h1 = keyframe_height(k1);
    let h = h0 + (h1 - h0) * smoothstep(t);
    (h.round() as usize).min(LEVELS.len() - 1)
}

/// Visible strip at scroll phase `phase` (cells `phase .. phase+WAVE_WIDTH`).
pub fn stream_wave(phase: usize) -> String {
    let mut s = String::with_capacity(WAVE_WIDTH);
    for i in 0..WAVE_WIDTH {
        s.push(LEVELS[height_at(phase.wrapping_add(i))]);
    }
    s
}

fn frame_ms_graph(samples: &RingBuffer<f64>) -> String {
    density_graph(samples.iter().copied())
}

fn frame_fps_graph(samples: &RingBuffer<f64>) -> String {
    density_graph(samples.iter().copied().map(ms_to_fps))
}

fn ms_to_fps(ms: f64) -> f64 {
    if ms > 0.0 {
        1000.0 / ms
    } else {
        0.0
    }
}

fn density_graph(values: impl IntoIterator<Item = f64>) -> String {
    let values: Vec<f64> = values.into_iter().collect();
    if values.is_empty() {
        return " ".repeat(WAVE_WIDTH);
    }
    let max = values.iter().cloned().fold(0.0_f64, f64::max).max(1.0);
    let recent = &values[values.len().saturating_sub(WAVE_WIDTH)..];
    let pad = WAVE_WIDTH.saturating_sub(recent.len());

    let mut s = String::with_capacity(WAVE_WIDTH);
    s.extend(std::iter::repeat_n(' ', pad));
    for &v in recent {
        let level = ((v / max) * (LEVELS.len() - 1) as f64).round() as usize;
        s.push(LEVELS[level.min(LEVELS.len() - 1)]);
    }
    s
}

fn recent_frame_ms(samples: &RingBuffer<f64>) -> f64 {
    let values: Vec<f64> = samples.iter().copied().collect();
    let recent = &values[values.len().saturating_sub(5)..];
    if recent.is_empty() {
        return 0.0;
    }
    recent.iter().sum::<f64>() / recent.len() as f64
}

fn recent_frame_fps(samples: &RingBuffer<f64>) -> f64 {
    let ms = recent_frame_ms(samples);
    ms_to_fps(ms)
}

/// Renders the top banner into `area` (should be `BANNER_HEIGHT` tall).
pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let _ = area;

    let (glyphs, glyph_style, mode_label, next_hint) = match app.banner_mode {
        BannerMode::Wave => (
            stream_wave(app.banner_frame),
            app.theme.secondary,
            "stream".to_string(),
            "A: ms",
        ),
        BannerMode::Ms => (
            frame_ms_graph(&app.frame_ms_samples),
            app.theme.success,
            format!("{:.0} ms/frame", recent_frame_ms(&app.frame_ms_samples)),
            "A: fps",
        ),
        BannerMode::Fps => (
            frame_fps_graph(&app.frame_ms_samples),
            app.theme.success,
            format!("{:.0} fps", recent_frame_fps(&app.frame_ms_samples)),
            "A: off",
        ),
        BannerMode::Off => (
            stream_wave(0),
            app.theme.dim,
            "paused".to_string(),
            "A: wave",
        ),
    };

    // One purple touch: the word “tako”. Everything else cyan/grey.
    let line = Line::from(vec![
        Span::styled(" tako ", app.theme.accent),
        Span::styled(glyphs, glyph_style),
        Span::styled(" 蛸", app.theme.secondary),
        Span::styled(format!("  {mode_label}"), app.theme.dim),
        Span::styled(format!("  [{next_hint}]"), app.theme.secondary),
    ]);

    frame.render_widget(
        Paragraph::new(line).block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(app.theme.border)
                .style(app.theme.root_style())
                .title("tako")
                .title_style(app.theme.accent),
        ),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_stays_small() {
        assert_eq!(stream_wave(0).chars().count(), WAVE_WIDTH);
        assert_eq!(stream_wave(100).chars().count(), WAVE_WIDTH);
    }

    #[test]
    fn scroll_is_continuous() {
        let a: Vec<char> = stream_wave(0).chars().collect();
        let b: Vec<char> = stream_wave(1).chars().collect();
        assert_eq!(b[..WAVE_WIDTH - 1], a[1..]);
    }

    #[test]
    fn heights_use_level_glyphs() {
        let s = stream_wave(42);
        assert!(s.chars().all(|c| LEVELS.contains(&c)));
    }

    #[test]
    fn adjacent_columns_never_jump_more_than_a_couple_levels() {
        for x in 0..200 {
            let a = height_at(x) as i64;
            let b = height_at(x + 1) as i64;
            assert!(
                (a - b).abs() <= 2,
                "adjacent columns at x={x} jumped from level {a} to {b}"
            );
        }
    }

    #[test]
    fn frame_ms_graph_is_blank_when_no_samples_yet() {
        let samples = RingBuffer::new(30);
        let s = frame_ms_graph(&samples);
        assert_eq!(s.chars().count(), WAVE_WIDTH);
        assert!(s.chars().all(|c| c == ' '));
    }

    #[test]
    fn frame_ms_graph_uses_level_glyphs_once_samples_exist() {
        let mut samples = RingBuffer::new(30);
        for v in [10.0, 20.0, 5.0, 60.0, 60.0, 60.0, 60.0, 60.0, 60.0, 60.0] {
            samples.push(v);
        }
        let s = frame_ms_graph(&samples);
        assert_eq!(s.chars().count(), WAVE_WIDTH);
        assert!(s.chars().all(|c| LEVELS.contains(&c)));
        assert!(s.ends_with('⣿'));
    }

    #[test]
    fn recent_frame_fps_is_inverse_of_ms() {
        let mut samples = RingBuffer::new(30);
        for _ in 0..5 {
            samples.push(10.0);
        }
        assert!((recent_frame_fps(&samples) - 100.0).abs() < 1e-9);
    }
}
