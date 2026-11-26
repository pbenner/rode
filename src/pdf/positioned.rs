//! Text cells use the renderer's crop/rotation transform, normalized to the page.
//! Byte ranges preserve Unicode and ligatures; font advances include whitespace.
use anyhow::{Result, ensure};
use hayro::{
    hayro_interpret::{util::TransformExt, *},
    hayro_syntax::Pdf,
    kurbo::{BezPath, Point, Rect},
};
use serde::{Deserialize, Serialize};

pub(super) const FRAME_LIMIT: usize = 8 * 1024 * 1024;
pub(super) const GLYPH_LIMIT: usize = 100_000;
pub(super) const TEXT_LIMIT: usize = 1024 * 1024;

/// One Unicode glyph or ligature and its cell in normalized, rotated page coordinates.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextCell {
    /// Half-open UTF-8 byte range in the page's text.
    pub range: [usize; 2],
    /// Left, top, right, bottom in the displayed page, each between zero and one.
    pub rect: [f32; 4],
    /// Normalized advance direction, including rotation and shear.
    pub direction: [f32; 2],
}

/// Immutable text layer, independent of raster resolution, theme and zoom.
#[derive(Default, Debug, Serialize, Deserialize)]
pub struct PageText {
    /// Text in document drawing order, with spaces and line breaks between runs.
    pub text: String,
    /// Cells in the same order as their UTF-8 ranges.
    pub cells: Vec<TextCell>,
}
impl PageText {
    pub(super) fn validate(&self) -> Result<()> {
        ensure!(
            self.text.len() <= TEXT_LIMIT && self.cells.len() <= GLYPH_LIMIT,
            "Document page text exceeds its limit."
        );
        let mut end = 0;
        for cell in &self.cells {
            let [start, next] = cell.range;
            let [x0, y0, x1, y1] = cell.rect;
            ensure!(
                start >= end
                    && start < next
                    && next <= self.text.len()
                    && self.text.is_char_boundary(start)
                    && self.text.is_char_boundary(next)
                    && cell
                        .rect
                        .iter()
                        .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
                    && x0 < x1
                    && y0 < y1,
                "Invalid document text geometry."
            );
            ensure!(
                cell.direction
                    .iter()
                    .all(|v| v.is_finite() && (-1.0..=1.0).contains(v))
                    && cell.direction != [0., 0.],
                "Invalid document text direction."
            );
            end = next;
        }
        Ok(())
    }
    /// Approximate retained allocation, used to bound the reader's text index.
    pub fn memory_size(&self) -> usize {
        self.text.capacity() + self.cells.capacity() * std::mem::size_of::<TextCell>()
    }
}

struct Collector {
    page: PageText,
    size: [f64; 2],
    previous: Option<(Point, Point, f64)>,
    overflow: bool,
}
impl<'a> Device<'a> for Collector {
    fn draw_path(&mut self, _: &BezPath, _: DrawProps<'a>, _: &DrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_image(&mut self, _: Image<'a, '_>, _: ImageDrawProps<'a>) {}
    fn pop_clip(&mut self) {}
    fn pop_transparency_group(&mut self) {}
    fn draw_glyph_run(&mut self, run: &font::GlyphRun<'_, 'a>, props: DrawProps<'a>, _: &DrawMode) {
        for glyph in run.glyphs() {
            if self.overflow {
                return;
            }
            let value = match glyph.as_unicode() {
                Some(hayro_cmap::BfString::Char(ch)) => ch.to_string(),
                Some(hayro_cmap::BfString::String(value)) => value,
                None => continue,
            };
            if value.is_empty() {
                continue;
            }
            let advance = match &**glyph {
                font::Glyph::Outline(glyph) => glyph.advance_width(),
                font::Glyph::Type3(glyph) => glyph.advance_width(),
            }
            .unwrap_or(500.)
            .abs()
            .max(1.) as f64;
            let transform = props.transform * glyph.transform();
            let bounds = transform.transform_rect_bbox(Rect::new(0., -200., advance, 800.));
            let bounds = bounds.intersect(Rect::new(0., 0., self.size[0], self.size[1]));
            if bounds.area() <= 0. || !bounds.is_finite() {
                continue;
            }
            let origin = transform * Point::ZERO;
            let end = transform * Point::new(advance, 0.);
            let height = ((transform * Point::new(0., 1000.)) - origin)
                .hypot()
                .max(0.1);
            if let Some((previous_origin, previous_end, previous_height)) = self.previous {
                let direction = previous_end - previous_origin;
                let direction = direction / direction.hypot().max(0.1);
                let gap = origin - previous_end;
                let height = height.min(previous_height);
                let separator = if gap.cross(direction).abs() > height * 0.5
                    || gap.dot(direction) < -height * 0.5
                {
                    Some('\n')
                } else if gap.dot(direction) > height * 0.15 {
                    Some(' ')
                } else {
                    None
                };
                if let Some(separator) = separator
                    && !self.page.text.ends_with(char::is_whitespace)
                {
                    self.page.text.push(separator);
                }
            }
            self.previous = Some((origin, end, height));
            if self.page.text.len() + value.len() > TEXT_LIMIT
                || self.page.cells.len() >= GLYPH_LIMIT
            {
                self.overflow = true;
                return;
            }
            let start = self.page.text.len();
            self.page.text.push_str(&value);
            let direction = [
                (end.x - origin.x) / self.size[0],
                (end.y - origin.y) / self.size[1],
            ];
            let length = direction[0].hypot(direction[1]).max(f64::MIN_POSITIVE);
            self.page.cells.push(TextCell {
                range: [start, self.page.text.len()],
                rect: [
                    (bounds.x0 / self.size[0]) as f32,
                    (bounds.y0 / self.size[1]) as f32,
                    (bounds.x1 / self.size[0]) as f32,
                    (bounds.y1 / self.size[1]) as f32,
                ],
                direction: [
                    (direction[0] / length) as f32,
                    (direction[1] / length) as f32,
                ],
            });
        }
    }
}

pub(super) fn page(document: &Pdf, number: usize) -> Result<PageText> {
    let page = document
        .pages()
        .get(number.wrapping_sub(1))
        .ok_or_else(|| anyhow::anyhow!("Invalid PDF text page."))?;
    let (width, height) = page.render_dimensions();
    let cache = InterpreterCache::new();
    let mut collector = Collector {
        page: PageText::default(),
        size: [width as f64, height as f64],
        previous: None,
        overflow: false,
    };
    let mut context = Context::new(
        page.initial_transform(true).to_kurbo(),
        Rect::new(0., 0., width as f64, height as f64),
        &cache,
        page.xref(),
        InterpreterSettings::default(),
    );
    interpret_page(page, &mut context, &mut collector);
    ensure!(!collector.overflow, "PDF page text exceeds its limit.");
    collector.page.validate()?;
    Ok(collector.page)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_cells_follow_portrait_landscape_rotation_and_crop_without_changing_unicode_ranges() {
        let document = Pdf::new(include_bytes!("../../tests/fixtures/pages.pdf").to_vec()).unwrap();
        for (number, expected) in [(1, "RODE - Page One"), (2, "RODE - Page Two")] {
            let text = page(&document, number).unwrap();
            assert_eq!(text.text, expected);
            assert_eq!(text.cells.len(), expected.len());
            assert!((text.cells[0].rect[0] - if number == 1 { 0.125 } else { 0.1 }).abs() < 0.001);
            assert!(
                (text.cells[0].rect[1] - if number == 1 { 0.0816 } else { 0.102 }).abs() < 0.002
            );
        }
        // A valid, standalone cropped/rotated page with two separate text runs.
        for rotation in [0, 90, 180, 270] {
            let content = "BT /F1 20 Tf 60 340 Td (First) Tj 0 -30 Td (Second) Tj ET";
            let objects = [
                "<< /Type /Catalog /Pages 2 0 R >>".into(),
                "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
                format!(
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 500] /CropBox [20 40 380 460] /Rotate {rotation} /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>"
                ),
                "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
                format!(
                    "<< /Length {} >>\nstream\n{content}\nendstream",
                    content.len()
                ),
            ];
            let mut bytes = "%PDF-1.4\n".to_string();
            let mut offsets = vec![];
            for (index, object) in objects.iter().enumerate() {
                offsets.push(bytes.len());
                bytes.push_str(&format!("{} 0 obj\n{object}\nendobj\n", index + 1));
            }
            let xref = bytes.len();
            bytes.push_str("xref\n0 6\n0000000000 65535 f \n");
            for offset in offsets {
                bytes.push_str(&format!("{offset:010} 00000 n \n"));
            }
            bytes.push_str(&format!(
                "trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF"
            ));
            let document = Pdf::new(bytes.into_bytes()).unwrap();
            let text = page(&document, 1).unwrap();
            assert_eq!(text.text, "First\nSecond", "rotation {rotation}");
            text.validate().unwrap();
            let rect = text.cells[0].rect;
            let direction = text.cells[0].direction;
            match rotation {
                0 => assert!(rect[0] < 0.2 && rect[1] < 0.4 && direction[0] > 0.9),
                90 => assert!(rect[0] > 0.6 && rect[1] < 0.2 && direction[1] > 0.9),
                180 => assert!(rect[0] > 0.7 && rect[1] > 0.6 && direction[0] < -0.9),
                270 => assert!(rect[0] < 0.4 && rect[1] > 0.7 && direction[1] < -0.9),
                _ => unreachable!(),
            }
        }
    }
}
