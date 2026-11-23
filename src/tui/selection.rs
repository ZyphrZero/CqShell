//! Border-free, pane-clamped in-TUI text drag selection and extraction.
//!
//! Selections are strictly bounded to a pane's inner content rect so dragging
//! can never leak into `│` box-drawing borders or adjacent columns. Soft-wrapped
//! visual rows belonging to the same logical line are rejoined cleanly without
//! inserting synthetic newlines.

use crate::theme::{Role, Theme};
use crate::tui::render::tokens;
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Modifier, Style},
};
use unicode_width::UnicodeWidthChar;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TextPane {
    Inspector,
    LogDrawer,
    LogModal,
    Notice,
    Confirm,
}

#[derive(Debug, Clone)]
pub(super) struct SelectableRow {
    pub text: String,
    /// True when this visual row is a soft-wrap continuation of the preceding row
    /// from the same logical line (so spanning both rows preserves the unbroken line).
    pub soft_wrap: bool,
}

#[derive(Debug, Clone)]
pub(super) struct SelectablePane {
    pub pane: TextPane,
    pub bounds: Rect,
    pub rows: Vec<SelectableRow>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DragSelection {
    pub pane: TextPane,
    pub bounds: Rect,
    pub anchor: Position,
    pub focus: Position,
    pub dragging: bool,
}

impl DragSelection {
    pub fn start(pane: TextPane, bounds: Rect, point: Position) -> Self {
        let clamped = Self::clamp(bounds, point);
        Self {
            pane,
            bounds,
            anchor: clamped,
            focus: clamped,
            dragging: true,
        }
    }

    pub fn clamp(bounds: Rect, point: Position) -> Position {
        if bounds.width == 0 || bounds.height == 0 {
            return Position::new(bounds.x, bounds.y);
        }
        Position::new(
            point.x.clamp(bounds.x, bounds.right().saturating_sub(1)),
            point.y.clamp(bounds.y, bounds.bottom().saturating_sub(1)),
        )
    }

    pub fn has_range(&self) -> bool {
        self.anchor != self.focus
    }

    /// Returns `(start, end)` sorted in reading order `(y, x)`.
    pub fn ordered(&self) -> (Position, Position) {
        if (self.anchor.y, self.anchor.x) <= (self.focus.y, self.focus.x) {
            (self.anchor, self.focus)
        } else {
            (self.focus, self.anchor)
        }
    }

    /// Inclusive screen-space column range `[x0, x1]` selected on row `y`.
    pub fn row_range(&self, y: u16) -> Option<(u16, u16)> {
        if self.bounds.width == 0 || self.bounds.height == 0 {
            return None;
        }
        let (start, end) = self.ordered();
        if y < start.y || y > end.y {
            return None;
        }
        let left = self.bounds.x;
        let right = self.bounds.right().saturating_sub(1);
        let x0 = if y == start.y { start.x } else { left };
        let x1 = if y == end.y { end.x } else { right };
        Some((x0.min(x1), x0.max(x1)))
    }
}

/// Extract clean, border-free text from `pane` covering `sel`, respecting CJK
/// double-width columns and rejoining soft-wrapped logical lines.
pub(super) fn extract_text(pane: &SelectablePane, sel: &DragSelection) -> String {
    if !sel.has_range() || pane.bounds.width == 0 || pane.bounds.height == 0 {
        return String::new();
    }
    let (start, end) = sel.ordered();
    let mut out = String::new();
    let mut has_prev = false;
    let mut prev_reached_end = false;

    for y in start.y..=end.y {
        let Some((x0, x1)) = sel.row_range(y) else {
            continue;
        };
        let row_idx = y.saturating_sub(pane.bounds.y) as usize;
        let Some(row) = pane.rows.get(row_idx) else {
            continue;
        };
        let rel_start = x0.saturating_sub(pane.bounds.x) as usize;
        let rel_end = x1.saturating_sub(pane.bounds.x) as usize;
        let (slice, reached_end) = slice_by_columns(&row.text, rel_start, rel_end);

        // Trim synthetic trailing cell padding when selection swept past text end
        // unless this row continues via soft-wrap into the next row.
        let cleaned = if y == end.y || !reached_end {
            slice.trim_end()
        } else {
            slice
        };

        if has_prev && !(row.soft_wrap && prev_reached_end && rel_start == 0) {
            out.push('\n');
        }
        out.push_str(cleaned);
        has_prev = true;
        prev_reached_end = reached_end;
    }

    out.trim_matches('\n').to_owned()
}

/// Slice `text` to characters overlapping inclusive visual columns `[c0, c1]`.
/// Also returns whether the slice reached the last display character of `text`.
fn slice_by_columns(text: &str, c0: usize, c1: usize) -> (&str, bool) {
    let mut col = 0usize;
    let mut byte_start: Option<usize> = None;
    let mut byte_end = 0usize;
    let mut total_cols = 0usize;

    for (idx, ch) in text.char_indices() {
        let cw = ch.width().unwrap_or(0);
        let next_col = col + cw;
        let overlaps = if cw == 0 {
            byte_start.is_some() && col <= c1
        } else {
            next_col > c0 && col <= c1
        };
        if overlaps {
            if byte_start.is_none() {
                byte_start = Some(idx);
            }
            byte_end = idx + ch.len_utf8();
        }
        col = next_col;
        total_cols = col;
    }

    let Some(start) = byte_start else {
        return ("", false);
    };
    let reached_end = total_cols == 0 || c1 + 1 >= total_cols;
    (&text[start..byte_end], reached_end)
}

/// Paint high-contrast selection highlight strictly inside `sel.bounds`.
pub(super) fn highlight_selection(buf: &mut Buffer, sel: &DragSelection, theme: &Theme) {
    if !sel.has_range() {
        return;
    }
    let area = buf.area;
    let (start, end) = sel.ordered();
    let style = if theme.flags.ascii {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
            .fg(tokens::color(theme, Role::OnAccent))
            .bg(tokens::color(theme, Role::Accent))
    };

    for y in start.y..=end.y {
        if y < area.y || y >= area.bottom() {
            continue;
        }
        let Some((x0, x1)) = sel.row_range(y) else {
            continue;
        };
        let right_bound = sel.bounds.right().min(area.right());
        let mut col = x0.max(area.x);
        // If selection starts on the trailing cell of a wide CJK glyph, include its leading cell.
        if col > sel.bounds.x && col < right_bound && buf[(col, y)].symbol().is_empty() {
            col -= 1;
        }
        let mut last = x1.min(right_bound.saturating_sub(1));
        // If selection ends on the leading cell of a wide CJK glyph, include its trailing cell.
        if last + 1 < right_bound && buf[(last + 1, y)].symbol().is_empty() {
            last += 1;
        }
        for x in col..=last {
            buf[(x, y)].set_style(style);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_selection_clamps_strictly_inside_bounds_and_extracts_cjk_and_soft_wraps() {
        let bounds = Rect::new(10, 5, 20, 4);
        let pane = SelectablePane {
            pane: TextPane::LogModal,
            bounds,
            rows: vec![
                SelectableRow {
                    text: "http://203.0.113.2".into(),
                    soft_wrap: false,
                },
                SelectableRow {
                    text: "8:9090/ui/".into(),
                    soft_wrap: true,
                },
                SelectableRow {
                    text: "订阅：示例.net".into(),
                    soft_wrap: false,
                },
            ],
        };

        // Dragging past left/right bounds clamps cleanly inside bounds (never grabs borders).
        let mut sel = DragSelection::start(TextPane::LogModal, bounds, Position::new(2, 5));
        sel.focus = DragSelection::clamp(bounds, Position::new(99, 6));
        assert_eq!(sel.anchor, Position::new(10, 5));
        assert_eq!(sel.focus, Position::new(29, 6));
        // Soft-wrapped URL across rows 0..=1 is rejoined seamlessly with no synthetic `\n`.
        assert_eq!(extract_text(&pane, &sel), "http://203.0.113.28:9090/ui/");

        // Dragging across CJK text extracts exact characters without corrupting UTF-8.
        let mut cjk = DragSelection::start(TextPane::LogModal, bounds, Position::new(16, 7));
        cjk.focus = Position::new(23, 7);
        assert_eq!(extract_text(&pane, &cjk), "示例.net");

        // Dragging across distinct logical lines preserves real `\n`.
        let mut multi = DragSelection::start(TextPane::LogModal, bounds, Position::new(10, 5));
        multi.focus = Position::new(29, 7);
        assert_eq!(
            extract_text(&pane, &multi),
            "http://203.0.113.28:9090/ui/\n订阅：示例.net"
        );
    }
}
