//! Static instrument-panel brackets and edge hatching, confined to frame margins.
use super::Palette;
use eframe::egui::{Color32, Rect, Shape, Stroke, pos2};

/// Paint an open frame with strong corner brackets and subdued vertical rails.
pub(super) fn surface(palette: Palette, rect: Rect, fill: Option<Color32>) -> Vec<Shape> {
    let mut shapes = Vec::with_capacity(21);
    if let Some(fill) = fill {
        shapes.push(Shape::rect_filled(rect, 0, fill));
    }
    for x in [rect.left(), rect.right()] {
        shapes.push(Shape::line_segment(
            [pos2(x, rect.top()), pos2(x, rect.bottom())],
            Stroke::new(0.8_f32, palette.frame_accent.gamma_multiply(0.3)),
        ));
    }
    shapes.extend(corners(palette, rect));
    // A small hatched rail echoes the reference's instruments. It is purely
    // frame decoration; its length and appearance never imply task progress.
    let length = bracket_length(rect);
    let start = rect.left() + length + 8.;
    if start + 52. < rect.right() - length - 8. {
        for index in 0..6 {
            let x = start + index as f32 * 8.;
            shapes.push(Shape::line_segment(
                [pos2(x, rect.bottom()), pos2(x + 4., rect.bottom() - 3.)],
                Stroke::new(1_f32, palette.frame_accent.gamma_multiply(0.6)),
            ));
        }
    }
    shapes
}

/// Four illuminated corner brackets, also used on selected tabs.
pub(super) fn corners(palette: Palette, rect: Rect) -> Vec<Shape> {
    let length = bracket_length(rect);
    let corners = [
        vec![
            pos2(rect.left(), rect.top() + length),
            rect.left_top(),
            pos2(rect.left() + length, rect.top()),
        ],
        vec![
            pos2(rect.right() - length, rect.top()),
            rect.right_top(),
            pos2(rect.right(), rect.top() + length),
        ],
        vec![
            pos2(rect.right(), rect.bottom() - length),
            rect.right_bottom(),
            pos2(rect.right() - length, rect.bottom()),
        ],
        vec![
            pos2(rect.left() + length, rect.bottom()),
            rect.left_bottom(),
            pos2(rect.left(), rect.bottom() - length),
        ],
    ];
    let mut shapes = Vec::with_capacity(12);
    for points in corners {
        for (width, opacity) in [(6_f32, 0.045), (3., 0.14), (1.5, 0.9)] {
            shapes.push(Shape::line(
                points.clone(),
                Stroke::new(width, palette.frame_accent.gamma_multiply(opacity)),
            ));
        }
    }
    shapes
}

/// Keep opposing brackets apart even on a narrow tab or compact card.
fn bracket_length(rect: Rect) -> f32 {
    24_f32.min(rect.width() / 3.).min(rect.height() / 3.)
}
