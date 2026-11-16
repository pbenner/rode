//! Deterministic native brush strokes on the shared icon grid. Pressure varies
//! along a continuous ribbon, with tapered ends and an angled nib. No randomness,
//! animation, textures or extra widgets are involved; the caller owns clipping,
//! tint, disabled opacity and interaction. Silhouettes keep their usual meanings.
use super::Canvas;
use eframe::egui::{self, Mesh, Pos2, Shape, Vec2};

/// Join sampled points into one pressure-sensitive stroke, without dark seams
/// from overlapping translucent segments. Work is bounded by the static grid paths.
pub(super) fn path(canvas: &Canvas<'_>, points: &[[f32; 2]]) {
    let mut samples = Vec::new();
    let mut distance = 0.;
    for segment in points.windows(2) {
        let start = canvas.point(segment[0]);
        let end = canvas.point(segment[1]);
        let length = start.distance(end);
        if length <= f32::EPSILON {
            continue;
        }
        if samples.is_empty() {
            samples.push((start, distance));
        }
        let steps = ((length / canvas.scale.max(0.01) / 2.).ceil() as usize).clamp(1, 16);
        for step in 1..=steps {
            let t = step as f32 / steps as f32;
            samples.push((start.lerp(end, t), distance + length * t));
        }
        distance += length;
    }
    if samples.len() < 2 {
        return;
    }
    let mut mesh = Mesh::default();
    for (index, (point, along)) in samples.iter().enumerate() {
        let previous = samples[index.saturating_sub(1)].0;
        let next = samples[(index + 1).min(samples.len() - 1)].0;
        let tangent = (next - previous).normalized();
        let normal = Vec2::new(-tangent.y, tangent.x);
        let t = along / distance;
        let pressure = 0.25 + 0.75 * (std::f32::consts::PI * t).sin().max(0.).sqrt();
        let nib = 0.8 + 0.2 * normal.x.abs();
        let half_width = canvas.stroke.width * 0.78 * pressure * nib;
        for side in [-1., 1.] {
            mesh.vertices.push(egui::epaint::Vertex {
                pos: *point + normal * (side * half_width),
                uv: egui::epaint::WHITE_UV,
                color: canvas.stroke.color,
            });
        }
        if index > 0 {
            let last = (index * 2) as u32;
            mesh.add_triangle(last - 2, last - 1, last);
            mesh.add_triangle(last - 1, last + 1, last);
        }
    }
    canvas.painter.add(Shape::mesh(mesh));
}

/// A nearly closed enso-like stroke retains circular icon semantics.
pub(super) fn circle(canvas: &Canvas<'_>, [x, y]: [f32; 2], radius: f32) {
    let points: Vec<_> = (0..=32)
        .map(|step| {
            let angle = -0.75 + step as f32 / 32. * (std::f32::consts::TAU - 0.38);
            [x + radius * angle.cos(), y + radius * angle.sin()]
        })
        .collect();
    path(canvas, &points);
}

/// A small ink dab, rather than a mechanically round punctuation mark.
pub(super) fn dot(canvas: &Canvas<'_>, [x, y]: [f32; 2]) {
    let radius = canvas.stroke.width * 0.9;
    let center = canvas.point([x, y]);
    let points: Vec<Pos2> = [
        [-0.3, -1.],
        [0.6, -0.6],
        [0.9, 0.2],
        [0.3, 1.],
        [-0.7, 0.7],
        [-0.9, -0.1],
    ]
    .into_iter()
    .map(|[x, y]| center + egui::vec2(x, y) * radius)
    .collect();
    canvas.painter.add(Shape::convex_polygon(
        points,
        canvas.stroke.color,
        egui::Stroke::NONE,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::icons::Icon,
        theme::{IconStyle, Palette},
    };
    use egui::{Color32, Context, Rect, pos2, vec2};

    #[test]
    fn icons_switch_treatment_without_leaking_styles_or_changing_button_geometry() {
        let ctx = Context::default();
        let mut outline_shapes = None;
        let mut button_rect = None;
        for style in [IconStyle::Outline, IconStyle::SumiE, IconStyle::Outline] {
            crate::theme::install(
                &ctx,
                Palette {
                    icons: style,
                    ..Default::default()
                },
            );
            assert_eq!(IconStyle::current(&ctx), style);
            let output = ctx.run(Default::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let response = ui.add(super::super::IconButton::icon(Icon::Files, "Explorer"));
                    assert_eq!(*button_rect.get_or_insert(response.rect), response.rect);
                });
            });
            if style == IconStyle::Outline {
                assert_eq!(
                    *outline_shapes.get_or_insert_with(|| output.shapes.clone()),
                    output.shapes
                );
            } else {
                assert_ne!(outline_shapes.as_ref().unwrap(), &output.shapes);
            }
        }
    }

    #[test]
    fn ink_stays_in_each_icon_slot_at_small_sizes_and_fractional_scaling() {
        let ctx = Context::default();
        crate::theme::install(
            &ctx,
            Palette {
                icons: IconStyle::SumiE,
                ..Default::default()
            },
        );
        for scale in [0.75, 1., 1.25, 2.] {
            ctx.set_pixels_per_point(scale);
            let output = ctx.run(Default::default(), |ctx| {
                let painter = ctx.layer_painter(egui::LayerId::background());
                for size in [14., 16., 22., 32.] {
                    let rect = Rect::from_min_size(pos2(20., 20.), vec2(size, size));
                    for icon in [
                        Icon::Files,
                        Icon::Chat,
                        Icon::Palette,
                        Icon::Search,
                        Icon::Git,
                        Icon::Settings,
                        Icon::Folder,
                        Icon::Close,
                    ] {
                        icon.paint(&painter, rect, Color32::WHITE);
                    }
                }
            });
            assert!(!output.shapes.is_empty());
            for shape in output.shapes {
                let bounds = shape.shape.visual_bounding_rect();
                assert!(bounds.is_finite());
                assert!(shape.clip_rect.expand(0.5).contains_rect(bounds));
                assert!(shape.clip_rect.width() <= 32.);
                if let Shape::Mesh(mesh) = shape.shape {
                    assert!(mesh.is_valid());
                }
            }
        }
    }
}
