//! Cell-boundary selections over the currently displayed terminal viewport.

use eframe::egui::{Pos2, Vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Point {
    pub row: u16,
    pub col: u16,
}

pub(super) struct Selection {
    pub anchor: Point,
    pub head: Point,
    pub dragging: bool,
    size: (u16, u16),
    scrollback: usize,
    alternate: bool,
    contents: String,
}

impl Selection {
    pub fn new(screen: &vt100::Screen, anchor: Point) -> Self {
        Self {
            anchor,
            head: anchor,
            dragging: true,
            size: screen.size(),
            scrollback: screen.scrollback(),
            alternate: screen.alternate_screen(),
            contents: screen.contents(),
        }
    }

    /// Invalidate a range when its underlying text moves or changes, so copying
    /// can never silently pick up different output at the same cell coordinates.
    pub fn valid_for(&self, screen: &vt100::Screen) -> bool {
        self.size == screen.size()
            && self.scrollback == screen.scrollback()
            && self.alternate == screen.alternate_screen()
            && self.contents == screen.contents()
    }

    pub fn bounds(&self) -> (Point, Point) {
        (self.anchor.min(self.head), self.anchor.max(self.head))
    }

    pub fn text(&self, screen: &vt100::Screen) -> Option<String> {
        let (start, end) = self.bounds();
        (start != end && self.valid_for(screen))
            .then(|| screen.contents_between(start.row, start.col, end.row, end.col))
    }

    pub fn contains(&self, row: u16, col: u16) -> bool {
        let point = Point { row, col };
        let (start, end) = self.bounds();
        start <= point && point < end
    }
}

/// Snap to the nearest boundary, keeping wide glyphs and combining characters
/// whole. Points outside the terminal clamp to the visible grid while dragging.
pub(super) fn point(
    screen: &vt100::Screen,
    origin: Pos2,
    cell: Vec2,
    position: Pos2,
    visible: (u16, u16),
) -> Point {
    let (rows, cols) = screen.size();
    let rows = rows.min(visible.0).max(1);
    let cols = cols.min(visible.1);
    let row = ((position.y - origin.y) / cell.y)
        .floor()
        .clamp(0., f32::from(rows - 1)) as u16;
    let x = ((position.x - origin.x) / cell.x).clamp(0., f32::from(cols));
    let mut col = x.round() as u16;
    if screen
        .cell(row, col)
        .is_some_and(|cell| cell.is_wide_continuation())
    {
        col = if x < f32::from(col) {
            col.saturating_sub(1)
        } else {
            (col + 1).min(cols)
        };
    }
    Point { row, col }
}
