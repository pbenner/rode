use super::*;
use crate::app::{
    EditorApp, Startup,
    markdown::{Images, Preview},
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

const SOURCE: &str = r"Inline $x_i^2$ and $a+b$ stay with prose.

$$
\frac{\alpha_i^2}{\sqrt{x}}
$$

After equation.

| First | Second |
| --- | --- |
| $c+d$ | $e+f$ |

Another $g+h$.

`$literal$` and escaped \$money.

```latex
$$\frac{literal}{literal}$$
```

Unsupported $\notARealCommand{x}$ stays readable.
";

#[test]
fn image_alt_text_preserves_math_as_literal_text_including_blocked_images() {
    use pulldown_cmark::{Event, Parser, Tag, TagEnd};

    let expected = r"Plot $x_i^2$ and $$\frac{x}{y}$$ and `$code$`";
    for target in ["plot.png", "https://example.invalid/plot.png"] {
        let source =
            format!(r"![Plot $x_i^2$ and $$\frac{{x}}{{y}}$$ and `` `$code$` ``]({target})");
        let mut preview = Preview::new(Arc::new(Images::default()));
        let requests = preview.prepare(1, "/project/math.md", &source);
        assert_eq!(requests.len(), usize::from(target == "plot.png"));
        let mut text = String::new();
        let mut in_image = false;
        for event in Parser::new_ext(&preview.source, super::super::options()) {
            match event {
                Event::Start(Tag::Image { .. }) => in_image = true,
                Event::End(TagEnd::Image) => in_image = false,
                Event::Text(value) if in_image || requests.is_empty() => text.push_str(&value),
                Event::InlineMath(_) | Event::DisplayMath(_) => {
                    panic!("Image caption became a math widget")
                }
                _ => {}
            }
        }
        assert!(text.contains(expected), "Caption lost source: {text}");
    }
}

/// Exercise the actual CommonMark callbacks, asynchronous results and cached
/// scroll layout, rather than only testing the TeX rasterizer in isolation.
#[test]
fn previews_render_math_fit_themes_and_keep_code_and_fallbacks_literal() {
    for notebook in [false, true] {
        for (theme, width, scale, font_size) in [
            ("atelier-dark", 900., 1., 16.),
            ("atelier-light", 320., 2., 20.),
        ] {
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
            let images = Arc::new(Images::default());
            let mut preview = if notebook {
                Preview::new_notebook(images)
            } else {
                Preview::new(images)
            };
            assert!(preview.prepare(1, "/project/math.md", SOURCE).is_empty());
            let expected = [
                ("x_i^2", false),
                ("a+b", false),
                ("\n\\frac{\\alpha_i^2}{\\sqrt{x}}\n", true),
                ("c+d", false),
                ("e+f", false),
                ("g+h", false),
            ];
            let mut bounds = egui::Rect::NOTHING;
            let mut frame = || {
                ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 1800.),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            bounds = ui.available_rect_before_wrap();
                            if notebook {
                                preview.inline_ui(ui, font_size, palette);
                            } else {
                                preview.ui(ui, 1, theme, font_size, palette);
                            }
                        });
                    },
                )
            };
            let deadline = Instant::now() + Duration::from_secs(10);
            let textures = loop {
                frame();
                let textures: Option<Vec<_>> = expected
                    .iter()
                    .map(|(source, display)| {
                        math::texture(&ctx, source, *display, font_size)
                            .map(|(texture, _)| texture.id())
                    })
                    .collect();
                if let Some(textures) = textures {
                    break textures;
                }
                assert!(Instant::now() < deadline, "Math worker did not finish");
                std::thread::sleep(Duration::from_millis(5));
            };
            // A second settled frame uses the document's scroll cache.
            frame();
            let output = frame();
            let text = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) => Some(text.galley.text()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            assert!(text.contains("$literal$"), "{text}");
            assert!(text.contains("$$\\frac{literal}{literal}$$"), "{text}");
            assert!(text.contains("\\notARealCommand{x}"), "{text}");
            assert!(!text.contains("$x_i^2$"), "{text}");
            let mut display_rect = egui::Rect::NOTHING;
            for (index, texture) in textures.iter().enumerate() {
                let image = output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::Shape::Rect(rect)
                            if rect
                                .brush
                                .as_ref()
                                .is_some_and(|brush| brush.fill_texture_id == *texture) =>
                        {
                            Some(rect)
                        }
                        _ => None,
                    })
                    .unwrap_or_else(|| {
                        panic!("Missing math image {index}: notebook={notebook}, {theme}")
                    });
                let rect = image.rect;
                assert!(
                    rect.left() >= bounds.left() - 1. && rect.right() <= bounds.right() + 1.,
                    "Equation escaped: {rect:?} in {bounds:?}"
                );
                assert_eq!(image.fill, palette.text);
                if index == 2 {
                    display_rect = rect;
                }
            }
            let following = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text().contains("After equation.") => {
                        Some(text)
                    }
                    _ => None,
                })
                .unwrap();
            assert!(
                following.pos.y >= display_rect.bottom(),
                "Following paragraph overlaps display math"
            );
            assert!(
                (display_rect.center().x - bounds.center().x).abs() <= 8.,
                "Display math is not centered: {display_rect:?} in {bounds:?}"
            );
            assert_eq!(preview.source, SOURCE, "Preview changed source");
        }
    }
}

#[test]
fn math_source_never_creates_asset_reads_and_wide_equations_fit_after_resize() {
    let ctx = egui::Context::default();
    let mut preview = Preview::new(Arc::new(Images::default()));
    let formula = "x+".repeat(30) + "y";
    let source = format!("$$![not an image](private.png)$$\n\n$${formula}$$");
    assert!(preview.prepare(1, "/project/math.md", &source).is_empty());
    assert!(preview.warnings.is_empty());
    let deadline = Instant::now() + Duration::from_secs(10);
    for width in [900., 180., 400.] {
        loop {
            let mut bounds = egui::Rect::NOTHING;
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 900.),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        bounds = ui.available_rect_before_wrap();
                        preview.ui(ui, 1, "dark", 16., theme::Palette::default());
                    });
                },
            );
            if let Some((texture, _)) = math::texture(&ctx, &formula, true, 16.)
                && let Some(rect) = output.shapes.iter().find_map(|shape| match &shape.shape {
                    egui::Shape::Rect(rect)
                        if rect
                            .brush
                            .as_ref()
                            .is_some_and(|brush| brush.fill_texture_id == texture.id()) =>
                    {
                        Some(rect.rect)
                    }
                    _ => None,
                })
            {
                assert!(rect.width() <= bounds.width() + 1.);
                assert!(rect.right() <= bounds.right() + 1.);
                break;
            }
            assert!(Instant::now() < deadline, "Wide math did not render");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
