use super::super::super::{Images, Preview, SCHEME};
use super::*;
use crate::app::{EditorApp, Startup};
use std::sync::Arc;

const TITLE: &str = "<h1 align=\"center\">Machine Learning for Bioinformatics</h1>\n<h3 align=\"center\">Freie Universität Berlin</h3>\n<h5 align=\"center\">Exercise Notebook - Model Selection</h5>\n<hr style=\"border:2px solid gray\">";

#[test]
fn title_styles_accept_only_supported_literal_values() {
    assert_eq!(alignment(" CENTER ", ""), Some(Align::Center));
    assert_eq!(
        alignment("center", "text-align: right; unknown: url(secret)"),
        Some(Align::Max)
    );
    assert_eq!(
        alignment("center", "text-align: invalid"),
        Some(Align::Center)
    );
    assert!(
        matches!(rule("border:2px solid gray"), Some(Kind::Rule { width: 2., color: Some(color) }) if color == Color32::from_gray(128))
    );
    assert!(matches!(
        rule("border-top: #abc SOLID 3px"),
        Some(Kind::Rule {
            width: 3.,
            color: Some(_)
        })
    ));
    for value in [
        "border: -2px solid gray",
        "border: 10000px solid gray",
        "border: NaNpx solid gray",
        "border: 2em solid gray",
        "border:2px solid url(secret)",
        "border: 2px dashed gray",
    ] {
        assert!(rule(value).is_none(), "{value}");
    }
    let prepared = super::super::prepare(TITLE).unwrap();
    assert_eq!(prepared.decorations.len(), 4);
    // Styled-looking examples within a code fence remain ordinary code.
    assert!(
        super::super::prepare(&format!("```html\n{TITLE}\n```"))
            .unwrap()
            .decorations
            .is_empty()
    );
    let source = format!(
        "{}\n\n{}",
        "<h1 align='center'>x</h1>\n".repeat(1000),
        "z".repeat(225_000)
    );
    let prepared = super::super::prepare(&source).unwrap();
    let mut converted = prepared.source;
    assert!(Presentation::prepare(&mut converted, prepared.decorations).is_err());
}

#[test]
fn notebook_titles_center_each_wrapped_line_and_draw_the_requested_rule() {
    for theme in ["atelier-dark", "atelier-light"] {
        for width in [340., 1000.] {
            for scale in [1., 2.] {
                for font_size in [14., 20.] {
                    let ctx = egui::Context::default();
                    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
                    let app = EditorApp::new(
                        &cc,
                        Startup {
                            theme: Some(theme.into()),
                            ..Default::default()
                        },
                    );
                    ctx.set_pixels_per_point(scale);
                    let palette = app.themes.active().palette;
                    let mut preview = Preview::new_notebook(Arc::new(Images::default()));
                    assert!(preview.prepare(1, "/project/title.ipynb", TITLE).is_empty());
                    let mut bounds = egui::Rect::NOTHING;
                    let mut frame = || {
                        ctx.run(
                            egui::RawInput {
                                screen_rect: Some(egui::Rect::from_min_size(
                                    egui::Pos2::ZERO,
                                    egui::vec2(width, 700.),
                                )),
                                ..Default::default()
                            },
                            |ctx| {
                                egui::CentralPanel::default().show(ctx, |ui| {
                                    bounds = ui.available_rect_before_wrap();
                                    preview.inline_ui(ui, font_size, palette);
                                });
                            },
                        )
                    };
                    frame();
                    let output = frame();
                    let mut sizes = vec![];
                    for title in [
                        "Machine Learning for Bioinformatics",
                        "Freie Universität Berlin",
                        "Exercise Notebook - Model Selection",
                    ] {
                        let shape = output
                            .shapes
                            .iter()
                            .find_map(|shape| match &shape.shape {
                                egui::Shape::Text(text) if text.galley.text() == title => {
                                    Some(text)
                                }
                                _ => None,
                            })
                            .unwrap_or_else(|| {
                                panic!("Missing {title}: {theme}/{width}/{scale}/{font_size}; source={:?}; shapes={:?}", preview.source, output.shapes.iter().filter_map(|shape| match &shape.shape { egui::Shape::Text(text) => Some((text.galley.text(), text.pos)), _ => None }).collect::<Vec<_>>())
                            });
                        sizes.push(shape.galley.job.sections[0].format.font_id.size);
                        for row in &shape.galley.rows {
                            let rect = row.rect().translate(shape.pos.to_vec2());
                            assert!(
                                (rect.center().x - bounds.center().x).abs() < 1.,
                                "{title}: {rect:?} in {bounds:?}"
                            );
                            assert!(
                                rect.left() >= bounds.left() - 1.
                                    && rect.right() <= bounds.right() + 1.,
                                "{title}: {rect:?} in {bounds:?}"
                            );
                        }
                    }
                    assert!(sizes[0] > sizes[1] && sizes[1] > sizes[2]);
                    assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
                        egui::Shape::LineSegment { points, stroke } if stroke.width == 2. && stroke.color == Color32::from_gray(128)
                            && (points[0].x - bounds.left()).abs() < 1. && (points[1].x - bounds.right()).abs() < 1.
                    )), "Missing 2px gray rule");
                }
            }
        }
    }
}

#[test]
fn styled_headings_keep_links_and_scoped_assets_after_image_rewriting() {
    let source = "![Before](before.png)\n\n<h2 align='center'>Read <em>the</em> <a href='https://example.org/guide'>guide</a></h2>\n\n<h3 align='right'><img src='inside.png'> Illustration</h3>\n\nBody [reference][guide].\n\n[guide]: https://example.org/body";
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let app = EditorApp::new(&cc, Startup::default());
    let palette = app.themes.active().palette;
    let images = Arc::new(Images::default());
    ctx.add_texture_loader(images.clone());
    let mut preview = Preview::new_notebook(images);
    let requests = preview.prepare(1, "/project/title.ipynb", source);
    assert_eq!(
        requests
            .iter()
            .map(|(_, path)| path.as_str())
            .collect::<Vec<_>>(),
        ["/project/before.png", "/project/inside.png"]
    );
    assert_eq!(preview.source.matches(MARKER).count(), 1);
    for event in Parser::new_ext(&preview.source, options()) {
        if let MdEvent::Start(Tag::Image { dest_url, .. }) = event {
            assert!(dest_url.starts_with(SCHEME));
        }
    }
    let mut clicked = None;
    let debug_source = preview.source.clone();
    let mut frame = |events| {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(850., 700.),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    clicked = preview.inline_ui(ui, 16., palette);
                });
            },
        )
    };
    frame(vec![]);
    let output = frame(vec![]);
    let heading = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == "Read the guide" => Some(text),
            _ => None,
        })
        .unwrap_or_else(|| {
            panic!(
                "Aligned linked heading disappeared: {:?}; shapes={:?}",
                debug_source,
                output
                    .shapes
                    .iter()
                    .filter_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) => Some((text.galley.text(), text.pos)),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            )
        });
    assert!(
        heading
            .galley
            .job
            .sections
            .iter()
            .any(|section| section.format.italics)
    );
    let start = heading.galley.pos_from_cursor(egui::text::CCursor::new(9));
    let end = heading.galley.pos_from_cursor(egui::text::CCursor::new(14));
    let point = (start.union(end)).center() + heading.pos.to_vec2();
    frame(vec![
        egui::Event::PointerMoved(point),
        egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    frame(vec![egui::Event::PointerButton {
        pos: point,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    }]);
    assert_eq!(clicked.as_deref(), Some("https://example.org/guide"));
    assert!(
        preview
            .cache
            .link_hooks()
            .contains_key("https://example.org/body")
    );
}
