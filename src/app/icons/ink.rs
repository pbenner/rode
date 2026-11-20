//! Deterministic native brush strokes on the shared icon grid. Pressure varies
//! along an anti-aliased ribbon, with tapered ends and an angled nib. No randomness,
//! animation, textures or extra widgets are involved; the caller owns clipping,
//! tint, disabled opacity and interaction. Silhouettes keep their usual meanings.
use super::Canvas;
use eframe::egui::{self, Color32, Mesh, Pos2, Shape, Stroke, Vec2};

/// Join sampled points into one pressure-sensitive stroke. Raw meshes bypass
/// egui's tessellator, so the ribbon supplies its own one-physical-pixel feather
/// on both sides and both caps. A single mesh avoids translucent overlap seams.
pub(super) fn path(canvas: &Canvas<'_>, points: &[[f32; 2]]) {
    let pixels_per_point = canvas.painter.ctx().pixels_per_point();
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
        // Pressure varies along the stroke as well as at corners. Sample in
        // physical pixels, with a fixed bound for every static grid segment.
        let steps = ((length * pixels_per_point).ceil() as usize).clamp(2, 64);
        for step in 1..=steps {
            let t = step as f32 / steps as f32;
            samples.push((start.lerp(end, t), distance + length * t));
        }
        distance += length;
    }
    if samples.len() < 2 {
        return;
    }
    let mesh = ribbon(&samples, canvas.stroke, 1. / pixels_per_point);
    canvas.painter.add(Shape::mesh(mesh));
}

fn ribbon(samples: &[(Pos2, f32)], stroke: Stroke, feather: f32) -> Mesh {
    let mut mesh = Mesh::default();
    let distance = samples.last().unwrap().1;
    for (index, (point, along)) in samples.iter().enumerate() {
        let before = if index > 0 {
            (*point - samples[index - 1].0).normalized()
        } else {
            (samples[1].0 - *point).normalized()
        };
        let after = if index + 1 < samples.len() {
            (samples[index + 1].0 - *point).normalized()
        } else {
            before
        };
        let tangent = (before + after).normalized();
        let normal = if tangent == Vec2::ZERO {
            before.rot90()
        } else {
            tangent.rot90()
        };
        // Preserve full-width right-angle joins. Sharper turns taper rather
        // than producing spikes that hit the icon clip at small sizes.
        let joined_normal = normal
            / normal
                .dot(after.rot90())
                .max(std::f32::consts::FRAC_1_SQRT_2);
        let t = along / distance;
        let pressure = 0.25 + 0.75 * (std::f32::consts::PI * t).sin().max(0.).sqrt();
        let nib = 0.8 + 0.2 * normal.x.abs();
        let half_width = stroke.width * 0.78 * pressure * nib;
        if index == 0 {
            section(
                &mut mesh,
                *point - before * feather,
                joined_normal,
                half_width,
                feather,
                Color32::TRANSPARENT,
            );
        }
        section(
            &mut mesh,
            *point,
            joined_normal,
            half_width,
            feather,
            stroke.color,
        );
        if index + 1 == samples.len() {
            section(
                &mut mesh,
                *point + after * feather,
                joined_normal,
                half_width,
                feather,
                Color32::TRANSPARENT,
            );
        }
    }
    mesh
}

/// Four edges form a solid core and two coverage ramps. Below one physical
/// pixel, collapse the core to a ridge and reduce opacity instead of losing the
/// thin tip between pixels. This conserves ink coverage as display scale changes.
fn section(
    mesh: &mut Mesh,
    point: Pos2,
    normal: Vec2,
    half_width: f32,
    feather: f32,
    color: Color32,
) {
    let inner = (half_width - feather * 0.5).max(0.);
    let outer = (half_width + feather * 0.5).max(feather);
    let ink = color.gamma_multiply((2. * half_width / feather).min(1.));
    let start = mesh.vertices.len() as u32;
    for (offset, color) in [
        (-outer, Color32::TRANSPARENT),
        (-inner, ink),
        (inner, ink),
        (outer, Color32::TRANSPARENT),
    ] {
        mesh.vertices.push(egui::epaint::Vertex {
            pos: point + normal * offset,
            uv: egui::epaint::WHITE_UV,
            color,
        });
    }
    if start > 0 {
        for band in 0..3 {
            let previous = start - 4 + band;
            let current = start + band;
            mesh.add_triangle(previous, previous + 1, current);
            mesh.add_triangle(previous + 1, current + 1, current);
        }
    }
}

/// A nearly closed enso-like stroke retains circular icon semantics.
pub(super) fn circle(canvas: &Canvas<'_>, [x, y]: [f32; 2], radius: f32) {
    let sweep = std::f32::consts::TAU - 0.38;
    let physical_radius = radius * canvas.scale * canvas.painter.ctx().pixels_per_point();
    let steps = ((sweep * physical_radius).ceil() as usize).clamp(24, 192);
    let points: Vec<_> = (0..=steps)
        .map(|step| {
            let angle = -0.75 + step as f32 / steps as f32 * sweep;
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
    fn feathering_tracks_physical_pixels_and_preserves_thin_ink_coverage() {
        let samples = [
            (pos2(10., 20.), 0.),
            (pos2(20., 20.), 10.),
            (pos2(30., 20.), 20.),
        ];
        for color in [
            Color32::WHITE,
            Color32::from_rgba_premultiplied(44, 63, 71, 128),
        ] {
            let mut reference: Option<Vec<f32>> = None;
            for pixels_per_point in [1., 0.75, 1.25, 1.5, 2., 3., 4.] {
                let mesh = ribbon(&samples, Stroke::new(1.2_f32, color), 1. / pixels_per_point);
                assert!(mesh.is_valid());
                let sections: Vec<_> = mesh.vertices.chunks_exact(4).collect();
                // Both caps fade to zero as well as the two side boundaries.
                for cap in [sections.first().unwrap(), sections.last().unwrap()] {
                    assert!(
                        cap.iter()
                            .all(|vertex| vertex.color == Color32::TRANSPARENT)
                    );
                }
                assert!(
                    ((sections[1][0].pos.x - sections[0][0].pos.x) * pixels_per_point - 1.).abs()
                        < 0.001
                );
                let mut coverage = Vec::new();
                for cross in &sections[1..sections.len() - 1] {
                    assert_eq!(cross[0].color, Color32::TRANSPARENT);
                    assert_eq!(cross[3].color, Color32::TRANSPARENT);
                    assert!(cross[1].color.a() > 0);
                    assert_eq!(cross[1].color, cross[2].color);
                    let gap = cross[0].pos.distance(cross[1].pos) * pixels_per_point;
                    assert!((gap - 1.).abs() < 0.001);
                    if cross[1].color.a() < color.a() {
                        assert_eq!(
                            cross[1].pos, cross[2].pos,
                            "Subpixel ink must form a coverage ridge"
                        );
                    }
                    // Integrate the linear coverage across the stroke. It must
                    // remain the same in logical points at every display scale,
                    // including translucent ink and tips thinner than a pixel.
                    coverage.push(
                        cross
                            .windows(2)
                            .map(|pair| {
                                pair[0].pos.distance(pair[1].pos)
                                    * (f32::from(pair[0].color.a()) + f32::from(pair[1].color.a()))
                                    / (2. * f32::from(color.a()))
                            })
                            .sum::<f32>(),
                    );
                }
                if let Some(reference) = &reference {
                    for (actual, expected) in coverage.iter().zip(reference) {
                        assert!(
                            (actual - expected).abs() < 0.015,
                            "Ink coverage changed at {pixels_per_point} pixels per point"
                        );
                    }
                } else {
                    reference = Some(coverage);
                }
            }
        }
    }

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
        for scale in [0.75, 1., 1.25, 1.5, 2., 3., 4.] {
            ctx.set_pixels_per_point(scale);
            let output = ctx.run(Default::default(), |ctx| {
                let painter = ctx.layer_painter(egui::LayerId::background());
                for size in [14., 16., 22., 32.] {
                    let rect = Rect::from_min_size(pos2(20., 20.), vec2(size, size));
                    for icon in [
                        Icon::Files,
                        Icon::Workspace,
                        Icon::List,
                        Icon::Grid,
                        Icon::Bookmark,
                        Icon::Search,
                        Icon::Remote,
                        Icon::Terminal,
                        Icon::Git,
                        Icon::Monitor,
                        Icon::Problems,
                        Icon::Environment,
                        Icon::Settings,
                        Icon::Extensions,
                        Icon::Help,
                        Icon::NewFile,
                        Icon::Folder,
                        Icon::Save,
                        Icon::SaveAs,
                        Icon::SaveAll,
                        Icon::Exit,
                        Icon::Format,
                        Icon::Check,
                        Icon::Fix,
                        Icon::Play,
                        Icon::Palette,
                        Icon::Refresh,
                        Icon::Close,
                        Icon::ChevronLeft,
                        Icon::ChevronRight,
                        Icon::ChevronUp,
                        Icon::ChevronDown,
                        Icon::More,
                        Icon::Copy,
                        Icon::Chat,
                        Icon::Debug,
                        Icon::Pause,
                        Icon::Stop,
                        Icon::StepOver,
                        Icon::StepIn,
                        Icon::StepOut,
                    ] {
                        icon.paint(&painter, rect, Color32::WHITE);
                    }
                }
            });
            assert!(!output.shapes.is_empty());
            for shape in output.shapes {
                let bounds = shape.shape.visual_bounding_rect();
                assert!(bounds.is_finite());
                assert!(
                    shape.clip_rect.expand(1.5 / scale).contains_rect(bounds),
                    "Anti-aliasing extended beyond the physical-pixel margin at {scale}x: {bounds:?} outside {:?}",
                    shape.clip_rect
                );
                assert!(shape.clip_rect.width() <= 32.);
                if let Shape::Mesh(mesh) = shape.shape {
                    assert!(mesh.is_valid());
                    assert!(
                        mesh.vertices
                            .iter()
                            .filter(|vertex| vertex.color.a() > 0)
                            .all(|vertex| shape.clip_rect.contains(vertex.pos)),
                        "Visible ink was clipped at {scale}x: {bounds:?} outside {:?}",
                        shape.clip_rect
                    );
                    assert!(
                        mesh.vertices.iter().any(|vertex| vertex.color.a() == 0),
                        "Brush meshes must include anti-aliased edges"
                    );
                    assert!(mesh.vertices.iter().any(|vertex| vertex.color.a() > 0));
                }
            }
        }
    }
}
