//! Logical source-line geometry from placed text rows. A wrapped line can span
//! several galley rows; only hard newlines advance file/debugger/Git line numbers.
//! Font metrics never substitute for these positions, including after scrolling
//! and changes to wrapping, font size, DPI or interface scale.
use super::*;

#[cfg(test)]
mod tests;

struct Line {
    first_row: usize,
    bounds: egui::Rect,
}

/// One projection per editor viewport. Retain only a weak layout reference and
/// rebuild when the cached galley changes; idle frames do not rescan the source.
#[derive(Default)]
pub(in crate::app) struct Layout {
    galley: std::sync::Weak<egui::Galley>,
    lines: Option<Arc<[Line]>>,
}

impl Layout {
    pub(in crate::app) fn rows<'a>(
        &mut self,
        galley: &'a Arc<egui::Galley>,
        origin: egui::Pos2,
    ) -> Rows<'a> {
        if self.galley.as_ptr() != Arc::as_ptr(galley) {
            self.lines = galley.job.wrap.max_width.is_finite().then(|| {
                let mut lines = Vec::new();
                let mut first_row = 0;
                let mut bounds = egui::Rect::NOTHING;
                for (index, row) in galley.rows.iter().enumerate() {
                    bounds = bounds.union(row.rect());
                    if row.ends_with_newline || index + 1 == galley.rows.len() {
                        lines.push(Line { first_row, bounds });
                        first_row = index + 1;
                        bounds = egui::Rect::NOTHING;
                    }
                }
                lines.into()
            });
            self.galley = Arc::downgrade(galley);
        }
        Rows {
            galley,
            origin,
            lines: self.lines.clone(),
        }
    }
}

/// Placed logical lines shared by line numbers, breakpoints, diagnostics and Git
/// decorations. Unwrapped source retains its direct, allocation-free row lookup.
pub(in crate::app) struct Rows<'a> {
    galley: &'a egui::Galley,
    origin: egui::Pos2,
    lines: Option<Arc<[Line]>>,
}

impl Rows<'_> {
    pub(in crate::app) fn bounds(&self) -> egui::Rect {
        self.galley.rect.translate(self.origin.to_vec2())
    }

    fn len(&self) -> usize {
        self.lines
            .as_ref()
            .map_or(self.galley.rows.len(), |lines| lines.len())
    }

    /// Full extent of a zero-based source line, including its continuation rows.
    pub(in crate::app) fn line(&self, line: usize) -> Option<egui::Rect> {
        let rect = if let Some(lines) = &self.lines {
            lines.get(line)?.bounds
        } else {
            self.galley.rows.get(line)?.rect()
        };
        Some(rect.translate(self.origin.to_vec2()))
    }

    /// Numbers and breakpoint icons belong to the first display row only.
    pub(in crate::app) fn first_row(&self, line: usize) -> Option<egui::Rect> {
        let row = self.lines.as_ref().map_or(Some(line), |lines| {
            lines.get(line).map(|line| line.first_row)
        })?;
        self.galley
            .rows
            .get(row)
            .map(|row| row.rect().translate(self.origin.to_vec2()))
    }

    /// Include partially visible logical lines, even when their first row has
    /// scrolled away. Their numbers remain clipped rather than being repeated.
    pub(in crate::app) fn visible(&self, clip: egui::Rect) -> std::ops::Range<usize> {
        if let Some(lines) = &self.lines {
            let first =
                lines.partition_point(|line| line.bounds.bottom() + self.origin.y < clip.top());
            let last =
                lines.partition_point(|line| line.bounds.top() + self.origin.y <= clip.bottom());
            first..last
        } else {
            let first = self
                .galley
                .rows
                .partition_point(|row| row.rect().bottom() + self.origin.y < clip.top());
            let last = self
                .galley
                .rows
                .partition_point(|row| row.rect().top() + self.origin.y <= clip.bottom());
            first..last
        }
    }

    /// Span source lines, clamping the end at EOF and retaining all wrapped rows.
    pub(in crate::app) fn span(&self, line: usize, count: usize) -> Option<egui::Rect> {
        let first = self.line(line)?;
        let last = self.line(
            line.saturating_add(count.saturating_sub(1))
                .min(self.len().saturating_sub(1)),
        )?;
        Some(first.union(last))
    }

    /// Deletions lie between logical lines; EOF is below the final display row.
    pub(in crate::app) fn boundary(&self, line: usize) -> f32 {
        self.line(line)
            .map_or_else(|| self.bounds().bottom(), |row| row.top())
    }

    /// Split a character range into its visible display rows. This keeps
    /// diagnostic squiggles attached to characters across soft wrap boundaries.
    pub(in crate::app) fn character_range(
        &self,
        start: usize,
        end: usize,
        clip: egui::Rect,
    ) -> impl Iterator<Item = egui::Rect> + '_ {
        let start = self.galley.layout_from_cursor(egui::text::CCursor {
            index: start,
            prefer_next_row: true,
        });
        let end = self.galley.layout_from_cursor(egui::text::CCursor {
            index: end,
            prefer_next_row: false,
        });
        let first = self
            .galley
            .rows
            .partition_point(|row| row.rect().bottom() + self.origin.y < clip.top());
        let last = self
            .galley
            .rows
            .partition_point(|row| row.rect().top() + self.origin.y <= clip.bottom());
        (start.row.max(first)..end.row.saturating_add(1).min(last)).map(move |index| {
            let row = &self.galley.rows[index];
            let left = row.x_offset(if index == start.row { start.column } else { 0 });
            let right = row.x_offset(if index == end.row {
                end.column
            } else {
                row.char_count_excluding_newline()
            });
            egui::Rect::from_min_max(
                egui::pos2(left, row.rect().top()),
                egui::pos2(right.max(left + 5.), row.rect().bottom()),
            )
            .translate(self.origin.to_vec2())
        })
    }
}
