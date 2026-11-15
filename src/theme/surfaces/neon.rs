//! Static native equivalents of SynthWave's editor wash and active-tab rails.
use super::*;

fn gradient(painter: &egui::Painter, rect: Rect, start: Color32, end: Color32, vertical: bool) {
    if !rect.is_positive() || !rect.is_finite() {
        return;
    }
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), start);
    mesh.colored_vertex(rect.right_top(), if vertical { start } else { end });
    mesh.colored_vertex(rect.right_bottom(), end);
    mesh.colored_vertex(rect.left_bottom(), if vertical { end } else { start });
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(Shape::mesh(mesh));
}

pub(super) fn tab(
    palette: Palette,
    painter: &egui::Painter,
    rect: Rect,
    fill: Color32,
    active: bool,
) {
    painter.rect_filled(rect, 0, if active { palette.background } else { fill });
    if active {
        gradient(
            painter,
            rect,
            palette.background,
            palette.background.lerp_to_gamma(palette.frame_accent, 0.10),
            true,
        );
        let rail = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 3.), rect.right_bottom());
        gradient(painter, rail, palette.frame_accent, palette.heading, false);
    } else {
        painter.line_segment(
            [rect.left_bottom(), rect.right_bottom()],
            Stroke::new(1_f32, palette.panel),
        );
    }
}

pub(super) fn editor(palette: Palette, ui: &egui::Ui) {
    let rect = ui.max_rect().intersect(ui.clip_rect());
    // Upstream keeps the upper 75% flat and washes only the bottom quarter.
    let bottom = Rect::from_min_max(
        pos2(rect.left(), rect.top() + rect.height() * 0.75),
        rect.right_bottom(),
    );
    gradient(
        ui.painter(),
        bottom,
        palette.background,
        palette.editor_gradient.unwrap_or(palette.background),
        true,
    );
}

pub(super) fn activity(palette: Palette, painter: &egui::Painter, rect: Rect) {
    gradient(painter, rect, palette.frame_accent, palette.heading, true);
}
