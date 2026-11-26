use super::*;
use betteroffice_pptx::{CaretStop, PositionedGlyph, TextAnchor, Transform};

fn line() -> PositionedTextLine {
    // The emoji occupies two UTF-16 units; "fi" occupies one shaping cluster.
    let stops = [(10, 10.), (11, 20.), (13, 32.), (15, 40.), (16, 50.)];
    PositionedTextLine {
        column_offset: 0.,
        x: 10.,
        y: 20.,
        width: 40.,
        height: 10.,
        baseline: 28.,
        start: 10,
        end: 16,
        runs: vec![PositionedTextRun {
            text: "A😀fiB".into(),
            start: 10,
            end: 16,
            x: 10.,
            width: 40.,
            font_id: 0,
            font_family: "Inter".into(),
            font_size_px: 10.,
            bold: false,
            italic: false,
            underline: false,
            color: "#000000".into(),
            letter_spacing_px: 0.,
            baseline_offset_px: 0.,
            glyphs: stops
                .windows(2)
                .map(|p| PositionedGlyph {
                    glyph_id: 1,
                    cluster: p[0].0,
                    x: p[0].1,
                    advance: p[1].1 - p[0].1,
                    x_offset: 0.,
                    y_offset: 28.,
                })
                .collect(),
        }],
        caret_stops: stops
            .into_iter()
            .map(|(position, x)| CaretStop { position, x })
            .collect(),
    }
}
fn text_box(width: f32, overflow: bool, transform: Transform) -> Primitive {
    Primitive::TextBox {
        text_shadow: None,
        object_id: 1,
        shape_id: None,
        story_id: None,
        x: 0.,
        y: 0.,
        w: width,
        h: 100.,
        anchor: TextAnchor::Top,
        paragraphs: vec![],
        lines: vec![line()],
        overflow,
        transform,
    }
}
fn list(primitives: Vec<Primitive>) -> SurfaceDisplayList {
    SurfaceDisplayList {
        contract_version: betteroffice_pptx::CONTRACT_VERSION,
        width: 100.,
        height: 100.,
        background: None,
        primitives,
    }
}
fn near(actual: [f32; 4], expected: [f32; 4]) {
    for (a, b) in actual.into_iter().zip(expected) {
        assert!((a - b).abs() < 0.0001, "{actual:?} != {expected:?}");
    }
}

#[test]
fn unicode_clusters_follow_native_rotation_and_flips() {
    for (rotation, flip_h, rect, direction) in [
        (0., false, [0.1, 0.2, 0.2, 0.3], [1., 0.]),
        (90., false, [0.7, 0.1, 0.8, 0.2], [0., 1.]),
        (180., false, [0.8, 0.7, 0.9, 0.8], [-1., 0.]),
        (270., false, [0.2, 0.8, 0.3, 0.9], [0., -1.]),
        (90., true, [0.7, 0.8, 0.8, 0.9], [0., -1.]),
    ] {
        let page = collect(&list(vec![text_box(
            100.,
            false,
            Transform {
                rotation_deg: rotation,
                flip_h,
                flip_v: false,
            },
        )]))
        .unwrap();
        assert_eq!(page.text, "A😀fiB");
        assert_eq!(
            page.cells
                .iter()
                .map(|c| &page.text[c.range[0]..c.range[1]])
                .collect::<Vec<_>>(),
            ["A", "😀", "fi", "B"]
        );
        near(page.cells[0].rect, rect);
        for (a, b) in page.cells[0].direction.into_iter().zip(direction) {
            assert!((a - b).abs() < 0.0001);
        }
    }
}

#[test]
fn copy_order_keeps_bullets_line_breaks_and_right_to_left_advances() {
    let mut primitive = text_box(100., false, Default::default());
    if let Primitive::TextBox { lines, .. } = &mut primitive {
        let mut bullet = lines[0].runs[0].clone();
        bullet.text = "•".into();
        bullet.end = bullet.start;
        bullet.x = 0.;
        bullet.width = 5.;
        bullet.glyphs.clear();
        lines[0].runs.insert(0, bullet);
        let mut second = lines[0].clone();
        second.y = 40.;
        lines.push(second);
    }
    let page = collect(&list(vec![primitive])).unwrap();
    assert_eq!(page.text, "• A😀fiB\n• A😀fiB");

    let mut primitive = text_box(100., false, Default::default());
    if let Primitive::TextBox { lines, .. } = &mut primitive {
        for glyph in &mut lines[0].runs[0].glyphs {
            glyph.x = 60. - glyph.x - glyph.advance;
        }
        for stop in &mut lines[0].caret_stops {
            stop.x = 60. - stop.x;
        }
    }
    let page = collect(&list(vec![primitive])).unwrap();
    assert_eq!(page.text, "A😀fiB");
    near(page.cells[0].rect, [0.4, 0.2, 0.5, 0.3]);
    assert!(page.cells.iter().all(|cell| cell.direction == [-1., 0.]));
}

#[test]
fn container_clips_and_overflow_match_the_raster_and_invisible_text_is_omitted() {
    let page = collect(&list(vec![text_box(35., false, Default::default())])).unwrap();
    assert_eq!(page.text, "A😀fi");
    assert!(page.cells.iter().all(|cell| cell.rect[2] <= 0.35));
    let page = collect(&list(vec![text_box(35., true, Default::default())])).unwrap();
    assert_eq!(page.text, "A😀fiB");
    let table = Primitive::Table {
        object_id: 1,
        shape_id: None,
        name: "table".into(),
        x: 0.,
        y: 0.,
        w: 35.,
        h: 100.,
        label: String::new(),
        primitives: vec![text_box(100., true, Default::default())],
        transform: Default::default(),
    };
    let page = collect(&list(vec![table])).unwrap();
    assert_eq!(page.text, "A😀fi");
    near(page.cells.last().unwrap().rect, [0.32, 0.2, 0.35, 0.3]);
    assert!(collect(&list(vec![])).unwrap().text.is_empty());
}

#[test]
fn native_slides_expose_the_same_text_and_geometry_without_decoding_images() {
    let presentation =
        super::super::Presentation::open(include_bytes!("../../../../tests/fixtures/slides.pptx"))
            .unwrap();
    for page in 1..=3 {
        let layer = presentation.page_text(page).unwrap();
        assert_eq!(layer.text, format!("RODE — PowerPoint slide {page}"));
        assert!(layer.cells.len() > 20);
        assert!(layer.cells[0].rect[0] > 0.04 && layer.cells[0].rect[0] < 0.1);
        layer.validate().unwrap();
    }
    assert!(presentation.page_text(0).is_err());
    assert!(presentation.page_text(4).is_err());
}

#[test]
fn oversized_runs_and_excessive_nesting_are_bounded() {
    let mut primitive = text_box(100., false, Default::default());
    if let Primitive::TextBox { lines, .. } = &mut primitive {
        lines[0].runs[0].text = "x".repeat(super::super::super::positioned::TEXT_LIMIT + 1);
    }
    assert!(
        collect(&list(vec![primitive]))
            .unwrap_err()
            .to_string()
            .contains("limit")
    );
    let mut primitive = text_box(100., false, Default::default());
    for _ in 0..66 {
        primitive = Primitive::Chart {
            object_id: 1,
            shape_id: None,
            name: String::new(),
            x: 0.,
            y: 0.,
            w: 100.,
            h: 100.,
            label: String::new(),
            primitives: vec![primitive],
            transform: Default::default(),
        };
    }
    assert!(
        collect(&list(vec![primitive]))
            .unwrap_err()
            .to_string()
            .contains("nesting")
    );
}
