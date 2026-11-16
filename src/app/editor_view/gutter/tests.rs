use super::*;
use egui::Color32;

#[test]
fn wrapped_line_geometry_matches_character_positions_at_every_scale() {
    let ctx = egui::Context::default();
    let source = "let 日本語 = café; ".repeat(16) + "\nsecond line\n\nlast line";
    let mut layout = Layout::default();
    for scale in [0.75, 1., 1.25, 2.] {
        ctx.set_pixels_per_point(scale);
        let _ = ctx.run(Default::default(), |ctx| {
            for width in [120., 260., f32::INFINITY] {
                let galley = ctx.fonts_mut(|fonts| {
                    fonts.layout_job(egui::text::LayoutJob::simple(
                        source.clone(),
                        FontId::monospace(15.),
                        Color32::WHITE,
                        width,
                    ))
                });
                let origin = egui::pos2(80., -140.);
                let rows = layout.rows(&galley, origin);
                assert_eq!(rows.len(), source.split('\n').count());
                let mut index = 0;
                for (line, text) in source.split('\n').enumerate() {
                    let start = galley
                        .pos_from_cursor(egui::text::CCursor {
                            index,
                            prefer_next_row: true,
                        })
                        .translate(origin.to_vec2());
                    let end = galley
                        .pos_from_cursor(egui::text::CCursor::new(index + text.chars().count()))
                        .translate(origin.to_vec2());
                    assert_eq!(rows.first_row(line).unwrap().top(), start.top());
                    assert_eq!(rows.line(line).unwrap().top(), start.top());
                    assert_eq!(rows.line(line).unwrap().bottom(), end.bottom());
                    assert_eq!(rows.boundary(line), start.top());
                    index += text.chars().count() + 1;
                }
                assert_eq!(
                    rows.span(0, 999).unwrap().bottom(),
                    galley
                        .rows
                        .last()
                        .unwrap()
                        .rect()
                        .translate(origin.to_vec2())
                        .bottom()
                );
                assert_eq!(rows.boundary(rows.len()), rows.bounds().bottom());
                assert!(rows.line(rows.len()).is_none());
                if width.is_finite() {
                    assert!(galley.rows.len() > rows.len());
                    let continuation = galley.rows[1]
                        .rect()
                        .translate(origin.to_vec2())
                        .shrink(0.1);
                    assert_eq!(rows.visible(continuation), 0..1);
                    assert!(rows.first_row(0).unwrap().bottom() < continuation.top());
                    assert!(rows.line(0).unwrap().contains_rect(continuation));
                    // A scroll only moves the origin; it must retain the projection.
                    let scrolled = layout.rows(&galley, origin + egui::vec2(0., -20.));
                    assert!(Arc::ptr_eq(
                        rows.lines.as_ref().unwrap(),
                        scrolled.lines.as_ref().unwrap()
                    ));
                } else {
                    assert!(rows.lines.is_none());
                    assert_eq!(galley.rows.len(), rows.len());
                }
            }
        });
    }
}

#[test]
fn diagnostic_ranges_split_at_soft_wraps_and_ignore_offscreen_rows() {
    let ctx = egui::Context::default();
    let _ = ctx.run(Default::default(), |ctx| {
        let source = "alpha βeta 日本語 repeated text ".repeat(16);
        let galley = ctx.fonts_mut(|fonts| {
            fonts.layout_job(egui::text::LayoutJob::simple(
                source.clone(),
                FontId::monospace(16.),
                Color32::WHITE,
                160.,
            ))
        });
        let origin = egui::pos2(50., 80.);
        let rows = Layout::default().rows(&galley, origin);
        let all: Vec<_> = rows
            .character_range(3, source.chars().count(), egui::Rect::EVERYTHING)
            .collect();
        assert_eq!(all.len(), galley.rows.len());
        assert!(all.windows(2).all(|pair| pair[0].bottom() <= pair[1].top()));
        let clip = galley.rows[2]
            .rect()
            .translate(origin.to_vec2())
            .shrink(0.1);
        let visible: Vec<_> = rows
            .character_range(3, source.chars().count(), clip)
            .collect();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0], all[2]);
        let end: Vec<_> = rows
            .character_range(usize::MAX, usize::MAX, egui::Rect::EVERYTHING)
            .collect();
        assert_eq!(end.len(), 1);
        assert!(end[0].is_finite());
    });
}
