//! Bounded, static native decoration. These shapes never allocate interaction
//! regions, change widget IDs, load assets, or request animation frames.
use super::Palette;
use eframe::egui::{self, Color32, Rect, Shape, Stroke, pos2};
use serde::Deserialize;

/// Optional native frame treatment inherited by custom themes.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Chrome {
    /// Standard flat frames used by the original themes.
    #[default]
    Plain,
    /// Cut corners, accent rails and a static edge glow.
    Chamfered,
}

impl Palette {
    /// Section title colour; plain themes keep their original foreground.
    pub fn heading_color(self) -> Color32 {
        match self.chrome {
            Chrome::Plain => self.text,
            Chrome::Chamfered => self.heading,
        }
    }

    /// Paint inside the existing panel margin, before any of its children.
    pub fn paint_panel(self, ui: &egui::Ui) {
        if self.chrome == Chrome::Chamfered {
            let rect = ui
                .max_rect()
                .expand(8.)
                .intersect(ui.clip_rect())
                .shrink(3.);
            ui.painter().extend(self.surface_shapes(rect, None));
        }
    }

    /// A card's fill and trim can be inserted behind children once its final
    /// bounds are known. At most eight outline points and a fixed set of strokes.
    pub(crate) fn surface_shapes(self, rect: Rect, fill: Option<Color32>) -> Vec<Shape> {
        if self.chrome == Chrome::Plain
            || !rect.is_finite()
            || rect.width() < 16.
            || rect.height() < 16.
        {
            return Vec::new();
        }
        let cut = 10_f32.min(rect.width() / 6.).min(rect.height() / 4.);
        let points = vec![
            pos2(rect.left() + cut, rect.top()),
            pos2(rect.right() - cut, rect.top()),
            pos2(rect.right(), rect.top() + cut),
            pos2(rect.right(), rect.bottom() - cut),
            pos2(rect.right() - cut, rect.bottom()),
            pos2(rect.left() + cut, rect.bottom()),
            pos2(rect.left(), rect.bottom() - cut),
            pos2(rect.left(), rect.top() + cut),
        ];
        let mut shapes = Vec::with_capacity(7);
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
        let start = rect.left() + cut + 8.;
        let end = start + (rect.width() * 0.22).clamp(12., 96.);
        if end + cut < rect.right() {
            shapes.push(Shape::convex_polygon(
                vec![
                    pos2(start, rect.top()),
                    pos2(end, rect.top()),
                    pos2(end - 3., rect.top() + 3.),
                    pos2(start - 3., rect.top() + 3.),
                ],
                self.frame_accent,
                Stroke::NONE,
            ));
        }
        shapes
    }

    /// Existing tab hit regions and text remain unchanged.
    pub fn paint_tab(self, painter: &egui::Painter, rect: Rect, fill: Color32, active: bool) {
        painter.rect_filled(rect, 0, fill);
        if self.chrome == Chrome::Chamfered {
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
