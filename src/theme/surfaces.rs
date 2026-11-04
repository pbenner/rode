//! Bounded, static native decoration. These shapes never allocate interaction
//! regions, change widget IDs, load assets, or request animation frames.
use super::Palette;
use eframe::egui::{self, Color32, Rect, Shape, Stroke, pos2};
use serde::Deserialize;
mod brackets;

/// Optional native frame treatment inherited by custom themes.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Chrome {
    /// Standard flat frames used by the original themes.
    #[default]
    Plain,
    /// Cut corners, accent rails and a static edge glow.
    Chamfered,
    /// Double cut-corner outlines, segmented rails and a static edge glow.
    Holographic,
    /// Open instrument frames with illuminated corner brackets and edge hatching.
    Bracketed,
}

impl Chrome {
    /// Whether native surfaces need space and styling for decorative frames.
    pub fn is_decorated(self) -> bool {
        self != Self::Plain
    }
}

impl Palette {
    /// Section title colour; plain themes keep their original foreground.
    pub fn heading_color(self) -> Color32 {
        match self.chrome {
            Chrome::Plain => self.text,
            Chrome::Chamfered | Chrome::Holographic | Chrome::Bracketed => self.heading,
        }
    }

    /// Paint inside the existing panel margin, before any of its children.
    pub fn paint_panel(self, ui: &egui::Ui) {
        if self.chrome.is_decorated() {
            let rect = ui
                .max_rect()
                .expand(8.)
                .intersect(ui.clip_rect())
                .shrink(3.);
            ui.painter().extend(self.surface_shapes(rect, None));
        }
    }

    /// A card's fill and trim can be inserted behind children once its final
    /// bounds are known. Geometry stays bounded regardless of the surface size.
    pub(crate) fn surface_shapes(self, rect: Rect, fill: Option<Color32>) -> Vec<Shape> {
        if self.chrome == Chrome::Plain
            || !rect.is_finite()
            || rect.width() < 16.
            || rect.height() < 16.
        {
            return Vec::new();
        }
        if self.chrome == Chrome::Bracketed {
            return brackets::surface(self, rect, fill);
        }
        let cut = 10_f32.min(rect.width() / 6.).min(rect.height() / 4.);
        let points = outline(rect, cut);
        let mut shapes = Vec::with_capacity(16);
        if let Some(fill) = fill {
            let mut mesh = egui::Mesh::default();
            // A very slight vertical tint, confined to cards rather than code.
            for point in &points {
                let tint = (1. - (point.y - rect.top()) / rect.height()) * 0.035;
                mesh.colored_vertex(*point, fill.lerp_to_gamma(self.frame_accent, tint));
            }
            for index in 1..7 {
                mesh.add_triangle(0, index, index + 1);
            }
            shapes.push(Shape::mesh(mesh));
        }
        for (width, opacity) in [(6_f32, 0.035), (3., 0.08), (1., 0.55)] {
            shapes.push(Shape::closed_line(
                points.clone(),
                Stroke::new(width, self.frame_accent.gamma_multiply(opacity)),
            ));
        }
        if self.chrome == Chrome::Holographic {
            // Keep the second outline and short illuminated corners inside the
            // frame margin; no decoration crosses code or terminal text.
            shapes.push(Shape::closed_line(
                outline(rect.shrink(3.), (cut - 1.).max(1.)),
                Stroke::new(0.7_f32, self.frame_accent.gamma_multiply(0.24)),
            ));
            let highlight = Stroke::new(1.2_f32, self.heading.gamma_multiply(0.8));
            let reach = (rect.width().min(rect.height()) - 2. * cut).min(15.);
            shapes.push(Shape::line(
                vec![
                    pos2(rect.left(), rect.top() + cut + reach),
                    pos2(rect.left(), rect.top() + cut),
                    pos2(rect.left() + cut, rect.top()),
                    pos2(rect.left() + cut + reach, rect.top()),
                ],
                highlight,
            ));
            shapes.push(Shape::line(
                vec![
                    pos2(rect.right(), rect.bottom() - cut - reach),
                    pos2(rect.right(), rect.bottom() - cut),
                    pos2(rect.right() - cut, rect.bottom()),
                    pos2(rect.right() - cut - reach, rect.bottom()),
                ],
                highlight,
            ));
        }
        let start = rect.left() + cut + 8.;
        let end = start + (rect.width() * 0.22).clamp(12., 96.);
        if end + cut < rect.right() {
            let segments = if self.chrome == Chrome::Holographic {
                ((end - start) / 12.).clamp(1., 6.) as usize
            } else {
                1
            };
            let step = (end - start) / segments as f32;
            for segment in 0..segments {
                let left = start + segment as f32 * step;
                let right = left + step - if segments > 1 { 2. } else { 0. };
                shapes.push(Shape::convex_polygon(
                    vec![
                        pos2(left, rect.top()),
                        pos2(right, rect.top()),
                        pos2(right - 3., rect.top() + 3.),
                        pos2(left - 3., rect.top() + 3.),
                    ],
                    self.frame_accent,
                    Stroke::NONE,
                ));
            }
        }
        shapes
    }

    /// Existing tab hit regions and text remain unchanged.
    pub fn paint_tab(self, painter: &egui::Painter, rect: Rect, fill: Color32, active: bool) {
        painter.rect_filled(rect, 0, fill);
        if self.chrome == Chrome::Bracketed {
            if active {
                let frame = rect.shrink(2.);
                painter.rect_filled(frame, 0, self.active);
                painter.extend(brackets::corners(self, frame));
            } else {
                painter.line_segment(
                    [rect.left_bottom(), rect.right_bottom()],
                    Stroke::new(1_f32, self.border),
                );
            }
        } else if self.chrome.is_decorated() {
            if active {
                let frame = rect.shrink(1.);
                painter.add(Shape::convex_polygon(
                    vec![
                        pos2(frame.left() + 7., frame.top()),
                        frame.right_top(),
                        pos2(frame.right(), frame.bottom() - 7.),
                        pos2(frame.right() - 7., frame.bottom()),
                        frame.left_bottom(),
                        pos2(frame.left(), frame.top() + 7.),
                    ],
                    self.active,
                    Stroke::new(1_f32, self.accent.gamma_multiply(0.65)),
                ));
                painter.line_segment(
                    [
                        pos2(frame.left() + 8., frame.top()),
                        pos2(frame.left() + 40., frame.top()),
                    ],
                    Stroke::new(2_f32, self.accent),
                );
            } else {
                painter.line_segment(
                    [rect.left_bottom(), rect.right_bottom()],
                    Stroke::new(1_f32, self.border),
                );
            }
        } else {
            painter.line_segment(
                [rect.right_top(), rect.right_bottom()],
                Stroke::new(1_f32, self.border),
            );
            if active {
                painter.rect_filled(
                    Rect::from_min_size(rect.min, egui::vec2(rect.width(), 2.)),
                    0,
                    self.accent,
                );
            } else {
                painter.line_segment(
                    [rect.left_bottom(), rect.right_bottom()],
                    Stroke::new(1_f32, self.border),
                );
            }
        }
    }
}

/// A convex cut-corner outline shared by the outer and inset panel frames.
fn outline(rect: Rect, cut: f32) -> Vec<egui::Pos2> {
    vec![
        pos2(rect.left() + cut, rect.top()),
        pos2(rect.right() - cut, rect.top()),
        pos2(rect.right(), rect.top() + cut),
        pos2(rect.right(), rect.bottom() - cut),
        pos2(rect.right() - cut, rect.bottom()),
        pos2(rect.left() + cut, rect.bottom()),
        pos2(rect.left(), rect.bottom() - cut),
        pos2(rect.left(), rect.top() + cut),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decorated_frames_remain_bounded_on_small_and_wide_surfaces() {
        for chrome in [Chrome::Chamfered, Chrome::Holographic, Chrome::Bracketed] {
            let palette = Palette {
                chrome,
                ..Default::default()
            };
            for size in [
                egui::vec2(16., 16.),
                egui::vec2(32., 200.),
                egui::vec2(180., 32.),
                egui::vec2(1200., 800.),
            ] {
                let rect = Rect::from_min_size(pos2(20., 30.), size);
                let shapes = palette.surface_shapes(rect, Some(palette.panel));
                assert!(!shapes.is_empty());
                assert!(shapes.len() <= 21);
                for shape in shapes {
                    // The widest glow stroke extends three points beyond the frame.
                    assert!(rect.expand(3.1).contains_rect(shape.visual_bounding_rect()));
                }
            }
        }
    }
}
