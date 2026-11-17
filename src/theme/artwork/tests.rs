//! Artwork caching, content clearance and interaction invariants.
use super::*;
use egui::{Color32, Galley, pos2, vec2};

#[test]
fn source_artwork_keeps_its_viewport_size_and_anchor_through_both_scroll_axes() {
    let ctx = egui::Context::default();
    let _ = ctx.run(Default::default(), |ctx| {
        // Long rows enter and leave the picture's area during scrolling. The
        // old adaptive fit shrank the picture to squeeze beside those rows.
        let source =
            "short\n".repeat(50) + &("x".repeat(65) + "\n").repeat(30) + &"short\n".repeat(50);
        let galley = ctx.fonts_mut(|fonts| {
            fonts.layout_no_wrap(source, egui::FontId::monospace(16.), Color32::WHITE)
        });
        let canvas = Rect::from_min_size(pos2(50., 40.), vec2(1000., 700.));
        for artwork in [Artwork::SumiE, Artwork::Samurai] {
            let pictures = artwork.pictures(Area::Source);
            let expected = placements(canvas, pictures, |_| true)[0].unwrap().rect;
            let (mut visible, mut hidden) = (0, 0);
            for y in (-1800..=200).step_by(40) {
                for x in [-200., 64.] {
                    match source_placements(canvas, &galley, pos2(x, y as f32), pictures)[0] {
                        Some(placed) => {
                            assert_eq!(placed.rect, expected, "Scrolling resized or moved artwork");
                            visible += 1;
                        }
                        None => hidden += 1,
                    }
                }
            }
            assert!(
                visible > 1 && hidden > 0,
                "Exercise both open space and overlapping code"
            );
        }
    });
    for area in [Area::Source, Area::Explorer, Area::Welcome] {
        let pictures = Artwork::Samurai.pictures(area);
        assert_eq!(pictures.len(), 1);
        assert!(
            pictures
                .iter()
                .all(|picture| matches!(picture.tone, Tone::Silver))
        );
    }
}

#[test]
fn samurai_picture_sets_stay_clear_of_content_and_each_other() {
    for area in [Area::Source, Area::Explorer, Area::Welcome] {
        for size in [
            egui::vec2(1600., 900.),
            egui::vec2(800., 500.),
            egui::vec2(300., 700.),
            egui::vec2(120., 90.),
        ] {
            let canvas = Rect::from_min_size(Pos2::ZERO, size);
            let occupied =
                Rect::from_center_size(canvas.center(), egui::vec2(size.x * 0.4, size.y * 0.6));
            let placed = placements(canvas, Artwork::Samurai.pictures(area), |rect| {
                !occupied.expand(16.).intersects(rect)
            });
            let rects: Vec<_> = placed.into_iter().flatten().map(|p| p.rect).collect();
            for (index, rect) in rects.iter().enumerate() {
                assert!(canvas.shrink(20.).contains_rect(*rect));
                assert!(!occupied.expand(16.).intersects(*rect));
                assert!(rect.width() >= 80.);
                assert!(
                    rects[..index]
                        .iter()
                        .all(|other| !other.expand(12.).intersects(*rect))
                );
            }
            if size.x > 1000. {
                assert_eq!(rects.len(), 1);
            }
            if size.y < 100. {
                assert!(rects.is_empty());
            }
        }
    }
    let ctx = egui::Context::default();
    let _ = ctx.run(Default::default(), |ctx| {
        let galley = ctx.fonts_mut(|fonts| {
            fonts.layout_no_wrap(
                "fn main() {}".into(),
                egui::FontId::monospace(16.),
                Color32::WHITE,
            )
        });
        let canvas = Rect::from_min_size(Pos2::ZERO, egui::vec2(1200., 800.));
        let pictures = Artwork::Samurai.pictures(Area::Source);
        let positioned = source_placements(canvas, &galley, egui::pos2(8., 40.), pictures);
        assert_eq!(positioned.iter().flatten().count(), 1);
        for positioned in positioned.into_iter().flatten() {
            assert!(galley.rows.iter().all(|row| {
                !row.rect()
                    .translate(egui::vec2(8., 40.))
                    .expand2(egui::vec2(24., 10.))
                    .intersects(positioned.rect)
            }));
        }
        let dense = ctx.fonts_mut(|fonts| {
            fonts.layout_no_wrap(
                ("x".repeat(200) + "\n").repeat(100),
                egui::FontId::monospace(16.),
                Color32::WHITE,
            )
        });
        assert!(
            source_placements(canvas, &dense, egui::pos2(8., 40.), pictures)
                .iter()
                .all(Option::is_none)
        );
    });
}
// Keep the original single-picture regression cases on the shared renderer.
fn placement(canvas: Rect, galley: &Galley, position: Pos2) -> Option<Rect> {
    source_placements(
        canvas,
        galley,
        position,
        Artwork::SumiE.pictures(Area::Source),
    )[0]
    .map(|p| p.rect)
}

#[test]
fn embedded_ink_has_transparent_negative_space_and_reuses_its_texture() {
    let ctx = egui::Context::default();
    let mut ids = Vec::new();
    for motif in [
        Motif::Pine,
        Motif::Bamboo,
        Motif::Plum,
        Motif::Warrior,
        Motif::Kabuto,
        Motif::Castle,
    ] {
        let image = ink_mask(motif).expect("The embedded illustration must decode");
        assert!(image.pixels.iter().any(|pixel| pixel.a() == 0));
        assert!(image.pixels.iter().any(|pixel| pixel.a() > 128));
        let light = texture(&ctx, motif).unwrap();
        let dark = texture(&ctx, motif).unwrap();
        assert_eq!(light.id(), dark.id());
        assert!(!ids.contains(&light.id()));
        ids.push(light.id());
    }
}

#[test]
fn artwork_fits_empty_space_without_crossing_real_rows_after_scroll_and_scaling() {
    let ctx = egui::Context::default();
    for scale in [0.75, 1., 1.25, 2.] {
        ctx.set_pixels_per_point(scale);
        let _ = ctx.run(Default::default(), |ctx| {
            for font_size in [12., 18., 28.] {
                for source in ["let ink = 1;\n".repeat(100), "long_line ".repeat(150)] {
                    let galley = ctx.fonts_mut(|fonts| {
                        fonts.layout_no_wrap(
                            source.clone(),
                            egui::FontId::monospace(font_size),
                            Color32::BLACK,
                        )
                    });
                    for size in [vec2(1000., 700.), vec2(400., 300.), vec2(80., 40.)] {
                        let canvas = Rect::from_min_size(pos2(50., 40.), size);
                        for offset in [-500., -100., 40., 300.] {
                            let position = pos2(64., offset);
                            if let Some(rect) = placement(canvas, &galley, position) {
                                assert!(canvas.shrink(20.).contains_rect(rect));
                                assert!(rect.width() >= 144.);
                                assert!(galley.rows.iter().all(|row| {
                                    !row.rect()
                                        .translate(position.to_vec2())
                                        .expand2(vec2(24., 10.))
                                        .intersects(rect)
                                }));
                            }
                        }
                    }
                }
            }
        });
    }
}

#[test]
fn dense_source_suppresses_artwork_and_short_source_leaves_a_visible_vignette() {
    let ctx = egui::Context::default();
    let _ = ctx.run(Default::default(), |ctx| {
        let layout = |source: String| {
            ctx.fonts_mut(|fonts| {
                fonts.layout_no_wrap(source, egui::FontId::monospace(16.), Color32::BLACK)
            })
        };
        let canvas = Rect::from_min_size(pos2(40., 40.), vec2(1000., 700.));
        let position = pos2(48., 40.);
        let short = layout("fn main() {}".into());
        assert!(placement(canvas, &short, position).is_some());
        let dense = layout(("x".repeat(200) + "\n").repeat(100));
        assert!(placement(canvas, &dense, position).is_none());
        assert!(placement(Rect::EVERYTHING, &short, position).is_none());
        assert!(placement(Rect::NOTHING, &short, position).is_none());
    });
}

#[test]
fn sidebar_and_welcome_artwork_leave_controls_and_layout_untouched() {
    let ctx = egui::Context::default();
    for theme in ["sumie-light", "sumie-dark", "samurai", "phosphor"] {
        let palette = crate::theme::Theme::builtin(theme).unwrap().palette;
        crate::theme::install(&ctx, palette);
        for size in [vec2(300., 700.), vec2(120., 90.)] {
            for sidebar in [false, true] {
                let mut occupied = Rect::NOTHING;
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            let canvas = ui.available_rect_before_wrap();
                            occupied = ui.button("Open folder").rect;
                            let cursor = ui.cursor();
                            let next_id = ui.next_auto_id();
                            if sidebar {
                                palette.paint_explorer_artwork(ui);
                            } else {
                                palette.paint_welcome_artwork(ui, canvas, occupied);
                            }
                            assert_eq!(ui.cursor(), cursor);
                            assert_eq!(ui.next_auto_id(), next_id);
                        });
                    },
                );
                let pictures = palette.artwork.pictures(if sidebar {
                    Area::Explorer
                } else {
                    Area::Welcome
                });
                let ids: Vec<_> = pictures
                    .iter()
                    .map(|picture| texture(&ctx, picture.motif).unwrap().id())
                    .collect();
                let painted: Vec<_> = output
                    .shapes
                    .iter()
                    .filter_map(|shape| match &shape.shape {
                        egui::Shape::Mesh(mesh) if ids.contains(&mesh.texture_id) => {
                            Some((shape.clip_rect, mesh.calc_bounds()))
                        }
                        _ => None,
                    })
                    .collect();
                if palette.artwork == Artwork::Samurai && size.y > 100. {
                    assert!((1..=2).contains(&painted.len()));
                } else {
                    assert_eq!(
                        painted.len(),
                        usize::from(palette.artwork == Artwork::SumiE && size.y > 100.)
                    );
                }
                for (clip, rect) in painted {
                    assert!(clip.contains_rect(rect));
                    assert!(!rect.intersects(occupied));
                }
            }
        }
    }
}

#[test]
fn painting_preserves_selection_layout_focus_and_input_without_hiding_or_moving() {
    let ctx = egui::Context::default();
    let original = "fn main() {\n    // 墨\n}\n";
    let mut source = original.to_owned();
    let id = egui::Id::new("artwork-source");
    let mut ink_id = None;
    for theme in ["sumie-light", "sumie-dark", "samurai", "phosphor"] {
        let palette = crate::theme::Theme::builtin(theme).unwrap().palette;
        crate::theme::install(&ctx, palette);
        let mut artwork_rect = None;
        for end in [0, 2, original.chars().count(), 0] {
            ctx.memory_mut(|memory| memory.request_focus(id));
            let selection = egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(end),
            );
            let mut state = egui::TextEdit::load_state(&ctx, id).unwrap_or_default();
            state.cursor.set_char_range(Some(selection));
            state.store(&ctx, id);
            let output =
                ctx.run(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000., 700.))),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            let viewport = ui.available_rect_before_wrap();
                            let source_output = egui::TextEdit::multiline(&mut source)
                                .id(id)
                                .code_editor()
                                .frame(false)
                                .show(ui);
                            assert_eq!(source_output.cursor_range, Some(selection));
                            if end > 0 {
                                assert!(source_output.galley.rows.iter().any(|row| {
                                    row.visuals.mesh.vertices.iter().any(|vertex| {
                                        vertex.color == ui.visuals().selection.bg_fill
                                    })
                                }));
                            }
                            let next_id = ui.next_auto_id();
                            let cursor = ui.cursor();
                            palette.paint_source_artwork(ui, &source_output, viewport);
                            assert_eq!(ui.next_auto_id(), next_id);
                            assert_eq!(ui.cursor(), cursor);
                            assert!(source_output.response.has_focus());
                            assert!(!source_output.response.changed());
                        });
                    },
                );
            let pine_texture = texture(&ctx, Motif::Pine).unwrap().id();
            assert_eq!(*ink_id.get_or_insert(pine_texture), pine_texture);
            let pictures = palette.artwork.pictures(Area::Source);
            let ids: Vec<_> = pictures
                .iter()
                .map(|picture| texture(&ctx, picture.motif).unwrap().id())
                .collect();
            let paintings = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Mesh(mesh) if ids.contains(&mesh.texture_id) => Some(mesh),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(paintings.len(), pictures.len());
            let rects: Vec<_> = paintings
                .iter()
                .map(|painting| painting.calc_bounds())
                .collect();
            assert_eq!(
                artwork_rect.get_or_insert_with(|| rects.clone()).as_slice(),
                rects.as_slice()
            );
            assert_eq!(source, original);
        }
    }
}
