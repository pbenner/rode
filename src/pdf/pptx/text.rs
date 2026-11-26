//! Read-only Unicode cells from the same BetterOffice display list as the
//! raster. Layout supplies shaping clusters, advances, wrapping and transforms;
//! no OCR, guessed character widths or second font/layout engine is involved.
use super::*;
use betteroffice_pptx::{PositionedTextLine, PositionedTextRun, Primitive, SurfaceDisplayList};
use hayro::kurbo::{Affine, Point, Rect};
use std::collections::BTreeMap;

pub(super) fn collect(list: &SurfaceDisplayList) -> Result<PageText> {
    ensure!(
        list.width.is_finite() && list.height.is_finite() && list.width > 0. && list.height > 0.,
        "Invalid slide text dimensions."
    );
    let mut collector = Collector {
        page: PageText::default(),
        size: [f64::from(list.width), f64::from(list.height)],
        clips: vec![corners(
            Affine::IDENTITY,
            Rect::new(0., 0., list.width.into(), list.height.into()),
        )],
    };
    collector.visit(&list.primitives, Affine::IDENTITY, 0)?;
    collector.page.validate()?;
    Ok(collector.page)
}

struct Collector {
    page: PageText,
    size: [f64; 2],
    clips: Vec<[Point; 4]>,
}
impl Collector {
    fn visit(&mut self, primitives: &[Primitive], parent: Affine, depth: usize) -> Result<()> {
        ensure!(depth <= 64, "Slide text nesting exceeds its limit.");
        for primitive in primitives {
            let (x, y, w, h, transform, clipped) = match primitive {
                Primitive::TextBox {
                    x,
                    y,
                    w,
                    h,
                    transform,
                    overflow,
                    ..
                } => (*x, *y, *w, *h, transform, !overflow),
                Primitive::Table {
                    x,
                    y,
                    w,
                    h,
                    transform,
                    ..
                }
                | Primitive::Chart {
                    x,
                    y,
                    w,
                    h,
                    transform,
                    ..
                } => (*x, *y, *w, *h, transform, true),
                _ => continue,
            };
            let rect = Rect::new(
                x.into(),
                y.into(),
                f64::from(x) + f64::from(w),
                f64::from(y) + f64::from(h),
            );
            ensure!(
                rect.is_finite() && transform.rotation_deg.is_finite(),
                "Invalid slide text transform."
            );
            let center = rect.center();
            let matrix = parent
                * Affine::translate(center.to_vec2())
                * Affine::rotate(f64::from(transform.rotation_deg).to_radians())
                * Affine::scale_non_uniform(
                    if transform.flip_h { -1. } else { 1. },
                    if transform.flip_v { -1. } else { 1. },
                )
                * Affine::translate(-center.to_vec2());
            if clipped {
                if w <= 0. || h <= 0. {
                    continue;
                }
                self.clips.push(corners(matrix, rect));
            }
            match primitive {
                Primitive::TextBox { lines, .. } => {
                    for line in lines {
                        self.line(line, matrix)?;
                    }
                }
                Primitive::Table { primitives, .. } | Primitive::Chart { primitives, .. } => {
                    self.visit(primitives, matrix, depth + 1)?
                }
                _ => unreachable!(),
            }
            if clipped {
                self.clips.pop();
            }
        }
        Ok(())
    }
    fn line(&mut self, line: &PositionedTextLine, matrix: Affine) -> Result<()> {
        let carets: BTreeMap<_, _> = line
            .caret_stops
            .iter()
            .map(|c| (c.position, f64::from(c.x)))
            .collect();
        let mut separator = "\n";
        for run in &line.runs {
            ensure!(
                run.text.len() <= super::super::positioned::TEXT_LIMIT,
                "Slide text exceeds its limit."
            );
            // Bullet markers live outside the story's UTF-16 character space.
            if run.start == run.end {
                if self.cell(
                    &run.text,
                    f64::from(run.x),
                    f64::from(run.x + run.width),
                    line,
                    matrix,
                    separator,
                )? {
                    separator = " ";
                }
                continue;
            }
            let mut utf16 = run.start;
            let mut offsets = BTreeMap::new();
            for (byte, ch) in run.text.char_indices() {
                offsets.insert(utf16, byte);
                utf16 = utf16
                    .checked_add(ch.len_utf16() as u32)
                    .context("Slide text offset overflow")?;
            }
            offsets.insert(utf16, run.text.len());
            ensure!(utf16 == run.end, "Invalid slide text offsets.");
            let glyphs = cluster_bounds(run);
            let mut boundaries = vec![run.start, run.end];
            boundaries.extend(glyphs.keys().copied());
            boundaries.extend(carets.range(run.start..=run.end).map(|(p, _)| *p));
            boundaries.sort_unstable();
            boundaries.dedup();
            let direction = glyphs
                .first_key_value()
                .zip(glyphs.last_key_value())
                .map_or(1., |((_, a), (_, b))| if a.0 > b.0 { -1. } else { 1. });
            for pair in boundaries.windows(2) {
                let (Some(&start), Some(&end)) = (offsets.get(&pair[0]), offsets.get(&pair[1]))
                else {
                    continue;
                };
                let (a, b) = if let Some(&(left, right)) = glyphs.get(&pair[0]) {
                    if direction > 0. {
                        (left, right)
                    } else {
                        (right, left)
                    }
                } else if let (Some(&a), Some(&b)) = (carets.get(&pair[0]), carets.get(&pair[1])) {
                    (a, b)
                } else {
                    continue;
                };
                if self.cell(&run.text[start..end], a, b, line, matrix, separator)? {
                    separator = "";
                }
            }
        }
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    fn cell(
        &mut self,
        value: &str,
        a: f64,
        b: f64,
        line: &PositionedTextLine,
        matrix: Affine,
        separator: &str,
    ) -> Result<bool> {
        if value.is_empty() || !a.is_finite() || !b.is_finite() || a == b || line.height <= 0. {
            return Ok(false);
        }
        let rect = Rect::new(
            a.min(b),
            line.y.into(),
            a.max(b),
            f64::from(line.y) + f64::from(line.height),
        );
        let mut polygon = corners(matrix, rect).to_vec();
        for clip in &self.clips {
            polygon = clipped(&polygon, clip);
            if polygon.is_empty() {
                return Ok(false);
            }
        }
        let bounds = polygon.iter().fold(
            Rect::new(
                f64::INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
            ),
            |r, p| Rect::new(r.x0.min(p.x), r.y0.min(p.y), r.x1.max(p.x), r.y1.max(p.y)),
        );
        let rect = [
            bounds.x0 / self.size[0],
            bounds.y0 / self.size[1],
            bounds.x1 / self.size[0],
            bounds.y1 / self.size[1],
        ]
        .map(|v| v.clamp(0., 1.) as f32);
        if rect[0] >= rect[2] || rect[1] >= rect[3] {
            return Ok(false);
        }
        let direction = (matrix * Point::new(b, 0.)) - (matrix * Point::new(a, 0.));
        let direction = [direction.x / self.size[0], direction.y / self.size[1]];
        let length = direction[0].hypot(direction[1]);
        ensure!(
            length.is_finite() && length > 0.,
            "Invalid slide text direction."
        );
        let separator = if self.page.text.is_empty()
            || (separator == "\n" && self.page.text.ends_with('\n'))
            || (separator == " "
                && (self.page.text.ends_with(char::is_whitespace)
                    || value.starts_with(char::is_whitespace)))
        {
            ""
        } else {
            separator
        };
        ensure!(
            self.page.text.len() + separator.len() + value.len()
                <= super::super::positioned::TEXT_LIMIT
                && self.page.cells.len() < super::super::positioned::GLYPH_LIMIT,
            "Slide text exceeds its limit."
        );
        self.page.text.push_str(separator);
        let start = self.page.text.len();
        self.page.text.push_str(value);
        self.page.cells.push(TextCell {
            range: [start, self.page.text.len()],
            rect,
            direction: [
                (direction[0] / length) as f32,
                (direction[1] / length) as f32,
            ],
        });
        Ok(true)
    }
}

/// Glyph clusters carry story-global UTF-16 offsets, not UTF-8 byte indexes.
/// Their placed advances also preserve RTL ordering, ligatures and combining marks.
fn cluster_bounds(run: &PositionedTextRun) -> BTreeMap<u32, (f64, f64)> {
    let mut bounds = BTreeMap::new();
    for glyph in &run.glyphs {
        if !(run.start..run.end).contains(&glyph.cluster) {
            continue;
        }
        let a = f64::from(glyph.x + glyph.x_offset);
        let b = a + f64::from(glyph.advance);
        bounds
            .entry(glyph.cluster)
            .and_modify(|r: &mut (f64, f64)| {
                r.0 = r.0.min(a.min(b));
                r.1 = r.1.max(a.max(b));
            })
            .or_insert((a.min(b), a.max(b)));
    }
    bounds
}
fn corners(matrix: Affine, rect: Rect) -> [Point; 4] {
    [
        (rect.x0, rect.y0),
        (rect.x1, rect.y0),
        (rect.x1, rect.y1),
        (rect.x0, rect.y1),
    ]
    .map(|p| matrix * Point::from(p))
}
/// Intersect convex text cells with the transformed page/container/text-box
/// clips, including flipped boxes. Invisible overflow never enters the index.
fn clipped(polygon: &[Point], clip: &[Point; 4]) -> Vec<Point> {
    let mut result = polygon.to_vec();
    let sign = (clip[1] - clip[0]).cross(clip[2] - clip[1]).signum();
    if sign == 0. {
        return Vec::new();
    }
    for i in 0..4 {
        let edge = clip[(i + 1) % 4] - clip[i];
        let distance = |p: Point| edge.cross(p - clip[i]) * sign;
        let input = std::mem::take(&mut result);
        let Some(&last) = input.last() else {
            break;
        };
        let mut previous = last;
        let mut before = distance(previous);
        for current in input {
            let after = distance(current);
            if (before >= 0.) != (after >= 0.) {
                result.push(previous + (current - previous) * (before / (before - after)));
            }
            if after >= 0. {
                result.push(current);
            }
            previous = current;
            before = after;
        }
    }
    result
}

#[cfg(test)]
mod tests;
