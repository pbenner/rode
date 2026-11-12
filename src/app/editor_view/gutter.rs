//! Source gutter geometry from the editor's placed text rows.
//!
//! Source text is unwrapped, so every galley row is one logical source line.
//! Font metrics are not a row pitch: layout rounds successive rows to physical
//! pixels and fallback glyphs can change their heights. All gutter decorations
//! share these actual positions, including after scrolling or changing DPI.
use super::*;

#[derive(Clone, Copy)]
pub(in crate::app) struct Rows<'a> {
    galley: &'a egui::Galley,
    origin: egui::Pos2,
}

impl<'a> Rows<'a> {
    pub(in crate::app) fn new(galley: &'a egui::Galley, origin: egui::Pos2) -> Self {
        Self { galley, origin }
    }

    pub(in crate::app) fn bounds(self) -> egui::Rect {
        self.galley.rect.translate(self.origin.to_vec2())
    }

    pub(in crate::app) fn line(self, line: usize) -> Option<egui::Rect> {
        self.galley
            .rows
            .get(line)
            .map(|row| row.rect().translate(self.origin.to_vec2()))
    }

    /// Include partially clipped rows; partition on placed bounds, not estimates.
    pub(in crate::app) fn visible(self, clip: egui::Rect) -> std::ops::Range<usize> {
        let first = self
            .galley
            .rows
            .partition_point(|r| r.rect().bottom() + self.origin.y < clip.top());
        let last = self
            .galley
            .rows
            .partition_point(|r| r.pos.y + self.origin.y <= clip.bottom());
        first..last
    }

    pub(in crate::app) fn span(self, line: usize, count: usize) -> Option<egui::Rect> {
        let first = self.line(line)?;
        let last = self.line(
            line.saturating_add(count.saturating_sub(1))
                .min(self.galley.rows.len().saturating_sub(1)),
        )?;
        Some(egui::Rect::from_min_max(
            first.left_top(),
            last.right_bottom(),
        ))
    }

    /// Deletions lie between rows; EOF is the bottom of the final rendered row.
    pub(in crate::app) fn boundary(self, line: usize) -> f32 {
        self.line(line)
            .map_or_else(|| self.bounds().bottom(), |row| row.top())
    }
}
