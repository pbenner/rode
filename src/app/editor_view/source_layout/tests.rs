//! Whole text, styled meshes, row placement and every character cursor must
//! survive chunk boundaries, wrapping and actual font-definition replacement.
use super::*;

#[test]
fn chunked_layout_matches_native_rows_and_cursor_geometry() {
    for wrap in [f32::INFINITY, 170.] {
        for scale in [1., 1.25, 2.] {
            let ctx = egui::Context::default();
            ctx.set_pixels_per_point(scale);
            let themes =
                crate::theme::Themes::load(tempfile::tempdir().unwrap().path(), "studio-neutral").0;
            let mut syntax = crate::theme::source::Source::default();
            let mut cache = Cache::default();
            for end in ["", "\n", "\r\n", "\n\n"] {
                let text = (0..200)
                    .map(|i| format!("{i}: 日本語 αβ\tlong source line\r\n"))
                    .collect::<String>()
                    + end;
                for reset in [false, true] {
                    if reset {
                        let mut fonts = egui::FontDefinitions::default();
                        fonts.font_data = fonts
                            .font_data
                            .into_iter()
                            .map(|(name, data)| {
                                (
                                    name,
                                    Arc::new((*data).clone().tweak(
                                        egui::epaint::text::FontTweak {
                                            scale: 1.1,
                                            ..Default::default()
                                        },
                                    )),
                                )
                            })
                            .collect();
                        ctx.set_fonts(fonts);
                    }
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                    let _ = loop {
                        let source = syntax.job(&ctx, &text, "rs", 17., themes.source_settings());
                        if syntax.settled() {
                            break source;
                        }
                        assert!(std::time::Instant::now() < deadline);
                        std::thread::sleep(std::time::Duration::from_millis(2));
                    };
                    for _ in 0..3 {
                        let _ = ctx.run(egui::RawInput::default(), |ctx| {
                            egui::CentralPanel::default().show(ctx, |ui| {
                                let source =
                                    syntax.job(ctx, &text, "rs", 17., themes.source_settings());
                                let mut expected_job = themes.highlight(ui, &text, "rs", 17.);
                                expected_job.wrap.max_width = wrap;
                                let expected = ui.fonts_mut(|fonts| fonts.layout_job(expected_job));
                                let actual = cache.layout(ui, source, wrap);
                                assert_eq!(actual.text(), expected.text());
                                assert_eq!(actual.rect, expected.rect);
                                assert_eq!(actual.rows.len(), expected.rows.len());
                                for (a, b) in actual.rows.iter().zip(&expected.rows) {
                                    assert_eq!(a.pos, b.pos);
                                    assert_eq!(a.ends_with_newline, b.ends_with_newline);
                                    assert_eq!(a.glyphs.len(), b.glyphs.len());
                                    assert_eq!(a.visuals.mesh.vertices, b.visuals.mesh.vertices);
                                    for (a, b) in a.glyphs.iter().zip(&b.glyphs) {
                                        assert_eq!(a.chr, b.chr);
                                        assert_eq!(a.pos, b.pos);
                                    }
                                }
                                for index in 0..=text.chars().count() {
                                    let cursor = egui::text::CCursor::new(index);
                                    assert_eq!(
                                        actual.pos_from_cursor(cursor),
                                        expected.pos_from_cursor(cursor)
                                    );
                                }
                            });
                        });
                    }
                }
            }
        }
    }
}
