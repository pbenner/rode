//! Aligned, horizontally scrollable GFM tables within the message's selectable text buffer.
use super::*;
use pulldown_cmark::Alignment;

const PADDING: f32 = 6.;

pub(super) struct Row {
    character: usize,
    pub cells: Vec<Range<usize>>,
}

pub(super) struct Table {
    pub range: Range<usize>,
    pub rows: Vec<Row>,
    alignments: Vec<Alignment>,
    starts: Vec<f32>,
    widths: Vec<f32>,
    separator: f32,
    width: f32,
}

impl Table {
    fn alignment(&self, column: usize) -> f32 {
        match self.alignments.get(column) {
            Some(Alignment::Right) => 1.,
            Some(Alignment::Center) => 0.5,
            _ => 0.,
        }
    }

    pub fn new(alignments: Vec<Alignment>) -> Self {
        Self {
            range: 0..0,
            rows: vec![],
            alignments,
            starts: vec![],
            widths: vec![],
            separator: 0.,
            width: 0.,
        }
    }

    pub fn row(&mut self, byte: usize, character: usize) {
        if self.rows.is_empty() {
            self.range.start = byte;
        }
        self.rows.push(Row {
            character,
            cells: vec![],
        });
    }

    pub fn cell(&mut self, bytes: Range<usize>) {
        self.rows.last_mut().unwrap().cells.push(bytes);
    }

    /// Measure styled content rather than character counts; padding never enters copied text.
    pub fn prepare(&mut self, ui: &egui::Ui, job: &mut LayoutJob) {
        let columns = self
            .rows
            .iter()
            .map(|row| row.cells.len())
            .max()
            .unwrap_or(0);
        self.widths = vec![16.; columns];
        let measure = |range: Range<usize>| {
            let mut cell = slice(job, range);
            cell.wrap.max_width = f32::INFINITY;
            ui.fonts_mut(|fonts| fonts.layout_job(cell)).size().x
        };
        let sizes: Vec<Vec<f32>> = self
            .rows
            .iter()
            .map(|row| {
                row.cells
                    .iter()
                    .enumerate()
                    .map(|(column, cell)| {
                        let width = measure(cell.clone());
                        self.widths[column] = self.widths[column].max(width);
                        width
                    })
                    .collect()
            })
            .collect();
        let separators: Vec<Vec<f32>> = self
            .rows
            .iter()
            .map(|row| {
                row.cells
                    .windows(2)
                    .map(|cells| measure(cells[0].end..cells[1].start))
                    .collect()
            })
            .collect();
        self.separator = separators.iter().flatten().copied().fold(0., f32::max);
        let mut next = PADDING;
        self.starts = self
            .widths
            .iter()
            .map(|width| {
                let start = next;
                next += width + PADDING * 2. + self.separator;
                start
            })
            .collect();
        self.width = self
            .starts
            .last()
            .zip(self.widths.last())
            .map_or(0., |(start, width)| start + width + PADDING);
        for (index, row) in self.rows.iter().enumerate() {
            let mut x = 0.;
            for (column, cell) in row.cells.iter().enumerate() {
                let width = sizes[index][column];
                let extra = (self.widths[column] - width).max(0.);
                let offset = extra * self.alignment(column);
                if !cell.is_empty() {
                    let start = self.starts[column] + offset;
                    leading(job, cell.start, (start - x).max(0.));
                    x = start + width;
                }
                if let Some(after) = row.cells.get(column + 1) {
                    let width = separators[index][column];
                    let start = self.starts[column]
                        + self.widths[column]
                        + PADDING
                        + (self.separator - width) / 2.;
                    leading(job, cell.end, (start - x).max(0.));
                    x = start + width;
                    let section = job
                        .sections
                        .partition_point(|section| section.byte_range.start < cell.end);
                    for section in &mut job.sections[section..] {
                        if section.byte_range.start >= after.start {
                            break;
                        }
                        section.format.color = egui::Color32::TRANSPARENT;
                    }
                }
            }
        }
        let begin = job
            .sections
            .partition_point(|section| section.byte_range.end <= self.range.start);
        for section in &mut job.sections[begin..] {
            if section.byte_range.start >= self.range.end {
                break;
            }
            let height = section
                .format
                .line_height
                .unwrap_or_else(|| ui.fonts_mut(|fonts| fonts.row_height(&section.format.font_id)));
            section.format.line_height = Some(height + 8.);
        }
        // Font rasterisation rounds glyph origins to pixels. Correct the
        // measured anchors after layout so that rounding cannot accumulate
        // across columns, especially with mixed proportional/monospace text.
        let mut table = slice(job, self.range.clone());
        table.wrap.max_width = f32::INFINITY;
        let laid_out = ui.fonts_mut(|fonts| fonts.layout_job(table));
        let mut adjustments = vec![];
        for (row, rendered) in self.rows.iter().zip(&laid_out.rows) {
            let mut byte = row
                .cells
                .first()
                .map_or(self.range.start, |cell| cell.start);
            let mut character = 0;
            let mut carried = 0.;
            for (column, cell) in row.cells.iter().enumerate() {
                character += job.text[byte..cell.start].chars().count();
                let length = job.text[cell.clone()].chars().count();
                if length > 0 {
                    let left = rendered.glyphs[character].pos.x;
                    let right = rendered.glyphs[character + length - 1].max_x();
                    let alignment = self.alignment(column);
                    let desired = self.starts[column] + self.widths[column] * alignment;
                    let actual = left + (right - left) * alignment;
                    let correction = desired - actual - carried;
                    adjustments.push((cell.start, correction));
                    carried += correction;
                }
                character += length;
                byte = cell.end;
            }
        }
        for (byte, correction) in adjustments {
            leading(job, byte, correction);
        }
    }

    pub fn paint(
        &self,
        ui: &egui::Ui,
        output: &egui::text_edit::TextEditOutput,
        palette: theme::Palette,
    ) {
        let Some(first) = self.rows.first() else {
            return;
        };
        let start = output
            .galley
            .layout_from_cursor(egui::text::CCursor {
                index: first.character,
                prefer_next_row: true,
            })
            .row;
        let rows =
            &output.galley.rows[start..(start + self.rows.len()).min(output.galley.rows.len())];
        let (Some(first), Some(last)) = (rows.first(), rows.last()) else {
            return;
        };
        let origin = output.galley_pos.to_vec2();
        let rect = egui::Rect::from_min_max(
            egui::pos2(0., first.rect().top()),
            egui::pos2(self.width, last.rect().bottom()),
        )
        .translate(origin);
        let stroke = Stroke::new(1_f32, palette.border);
        ui.painter()
            .rect_stroke(rect, 2., stroke, egui::StrokeKind::Inside);
        for row in rows.iter().take(rows.len().saturating_sub(1)) {
            let y = origin.y + row.rect().bottom();
            ui.painter().line_segment(
                [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                stroke,
            );
        }
        for column in 0..self.widths.len().saturating_sub(1) {
            let x = origin.x
                + self.starts[column]
                + self.widths[column]
                + PADDING
                + self.separator / 2.;
            ui.painter().line_segment(
                [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                stroke,
            );
        }
    }
}

fn leading(job: &mut LayoutJob, byte: usize, space: f32) {
    let index = job
        .sections
        .partition_point(|section| section.byte_range.start < byte);
    if let Some(section) = job.sections.get_mut(index) {
        debug_assert_eq!(section.byte_range.start, byte);
        section.leading_space += space;
    }
}

fn slice(job: &LayoutJob, range: Range<usize>) -> LayoutJob {
    let start = job
        .sections
        .partition_point(|section| section.byte_range.end <= range.start);
    let sections = job.sections[start..]
        .iter()
        .take_while(|section| section.byte_range.start < range.end)
        .map(|section| {
            let mut section = section.clone();
            section.byte_range = section.byte_range.start.max(range.start) - range.start
                ..section.byte_range.end.min(range.end) - range.start;
            section
        })
        .collect();
    LayoutJob {
        text: job.text[range].into(),
        sections,
        ..Default::default()
    }
}

/// Concatenate cached paragraph layouts without changing character order or selection indices.
/// Prose wraps at the viewport edge; table rows retain their measured column positions.
pub(super) fn galley(
    ui: &egui::Ui,
    job: &LayoutJob,
    tables: &[Table],
    width: f32,
    prose_offset: f32,
) -> Arc<egui::Galley> {
    let mut job = job.clone();
    job.wrap.max_width = width.max(1.);
    let base = ui.fonts_mut(|fonts| fonts.layout_job(job.clone()));
    if tables.is_empty() {
        return base;
    }
    let mut ranges = vec![];
    let mut cursor = 0;
    for table in tables {
        if cursor < table.range.start {
            ranges.push((cursor..table.range.start, width, false));
        }
        let mut end = table.range.end;
        if job.text[end..].starts_with('\n') {
            end += 1;
        }
        ranges.push((table.range.start..end, f32::INFINITY, true));
        cursor = end;
    }
    if cursor < job.text.len() {
        ranges.push((cursor..job.text.len(), width, false));
    }
    let mut galley = (*base).clone();
    galley.rows.clear();
    galley.mesh_bounds = egui::Rect::NOTHING;
    galley.num_vertices = 0;
    galley.num_indices = 0;
    let mut y = 0.;
    let mut right = tables.iter().map(|table| table.width).fold(0., f32::max);
    for (index, (range, wrap, table)) in ranges.iter().enumerate() {
        let mut part = slice(&job, range.clone());
        part.wrap.max_width = *wrap;
        let layout = ui.fonts_mut(|fonts| fonts.layout_job(part));
        let count = layout.rows.len()
            - usize::from(index + 1 < ranges.len() && job.text[range.clone()].ends_with('\n'));
        let mut bottom = y;
        for mut row in layout.rows[..count].iter().cloned() {
            row.pos.y += y;
            if !*table && prose_offset > 0. {
                // Cursor hit testing uses glyph coordinates, not PlacedRow.x.
                // Translate the visuals and those coordinates together.
                let text = Arc::make_mut(&mut row.row);
                for glyph in &mut text.glyphs {
                    glyph.pos.x += prose_offset;
                }
                text.size.x += prose_offset;
                let offset = egui::vec2(prose_offset, 0.);
                text.visuals.mesh.translate(offset);
                text.visuals.mesh_bounds = text.visuals.mesh_bounds.translate(offset);
            }
            bottom = bottom.max(row.rect().bottom());
            right = right.max(row.rect().right());
            galley.mesh_bounds = galley
                .mesh_bounds
                .union(row.visuals.mesh_bounds.translate(row.pos.to_vec2()));
            galley.num_vertices += row.visuals.mesh.vertices.len();
            galley.num_indices += row.visuals.mesh.indices.len();
            galley.rows.push(row);
        }
        y = bottom;
    }
    galley.rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(right, y));
    Arc::new(galley)
}

#[cfg(test)]
mod tests;
