//! Layout and text helpers used by every screen in both applications.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    text::{Line, Span},
    widgets::{Block, Borders},
};

/// Offset that keeps `selected` on screen without storing scroll position
/// between frames: the selection rides the bottom edge once the list is
/// longer than the viewport.
pub fn scroll_offset(selected: usize, height: usize, total: usize) -> usize {
    if height == 0 || total <= height {
        return 0;
    }
    selected.saturating_sub(height - 1).min(total - height)
}

/// A bordered pane with its keys along the bottom edge.
pub fn panel(title: String, hint: &'static str) -> Block<'static> {
    Block::default()
        .title(title)
        .title_bottom(hint)
        .borders(Borders::ALL)
}

/// Pads a line with spaces so a selected row's background spans the pane
/// instead of stopping at the end of the text.
pub fn pad_to_width(line: Line<'_>, width: u16) -> Line<'_> {
    let used = line.width();
    let mut line = line;
    if used < width as usize {
        line.push_span(Span::raw(" ".repeat(width as usize - used)));
    }
    line
}

/// Right-aligns into a fixed-width column, so decimal points line up. Never
/// truncates: a clipped number is worse than a misaligned one.
pub fn pad_left(s: &str, width: usize) -> String {
    let used = s.chars().count();
    if used >= width {
        s.to_string()
    } else {
        format!("{}{s}", " ".repeat(width - used))
    }
}

/// Truncates on a character boundary; slicing by byte index would panic on
/// the multi-byte punctuation that headlines and player names are full of.
pub fn truncate(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        return s.to_string();
    }
    let mut out: String = s.chars().take(width.saturating_sub(1)).collect();
    out.push('\u{2026}');
    out
}

/// A centred rect of an absolute size, clamped to the area.
pub fn centered_size(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

/// A centred rect as a percentage of the area.
pub fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

/// Steps through a fixed list of variants, wrapping at both ends.
pub fn cycle<T: Copy + PartialEq>(all: &[T], current: T, step: isize) -> T {
    let n = all.len() as isize;
    let at = all.iter().position(|x| *x == current).unwrap_or(0) as isize;
    all[((at + step).rem_euclid(n)) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_lists_never_scroll() {
        assert_eq!(scroll_offset(0, 10, 5), 0);
        assert_eq!(scroll_offset(4, 10, 5), 0);
    }

    #[test]
    fn the_selection_stays_visible_in_a_long_list() {
        assert_eq!(scroll_offset(0, 10, 40), 0);
        assert_eq!(scroll_offset(9, 10, 40), 0);
        assert_eq!(scroll_offset(10, 10, 40), 1);
        assert_eq!(scroll_offset(39, 10, 40), 30);
    }

    #[test]
    fn a_zero_height_viewport_does_not_underflow() {
        assert_eq!(scroll_offset(5, 0, 40), 0);
    }

    #[test]
    fn truncate_adds_an_ellipsis_and_respects_character_boundaries() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("abcdefghij", 5), "abcd\u{2026}");
        assert_eq!(truncate("Japan\u{2019}s yen", 7), "Japan\u{2019}\u{2026}");
    }

    #[test]
    fn pad_left_right_aligns_and_never_truncates() {
        assert_eq!(pad_left("1.5", 6), "   1.5");
        assert_eq!(pad_left("1234567", 3), "1234567");
    }

    #[test]
    fn centered_size_clamps_to_an_area_smaller_than_the_overlay() {
        let rect = centered_size(66, 30, Rect::new(0, 0, 20, 10));
        assert_eq!((rect.width, rect.height), (20, 10));
    }

    #[test]
    fn cycle_wraps_in_both_directions() {
        let all = [1, 2, 3];
        assert_eq!(cycle(&all, 3, 1), 1);
        assert_eq!(cycle(&all, 1, -1), 3);
    }
}
