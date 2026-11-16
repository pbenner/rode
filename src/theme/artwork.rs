//! Embedded, static ink illustrations. Placement uses the editor's actual visible
//! text rows, so artwork never changes source contrast or owns an input region.
//! One alpha-mask texture is shared by every viewport of an egui context; theme
//! switches only change its tint. No files, network requests or timers are used.
use super::Palette;
use eframe::egui::{self, Galley, Pos2, Rect, TextureHandle, pos2, vec2};
use serde::Deserialize;

/// Optional embedded illustration, inherited independently of colors and chrome.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Artwork {
    /// Keep the editor canvas unillustrated.
    #[default]
    None,
    /// Original pine, crane and mountain ink-wash vignette.
    SumiE,
}

#[derive(Clone)]
struct InkTexture(Option<TextureHandle>);

/// Treat dark strokes as ink coverage rather than retaining a paper-colored
/// backdrop. The same transparent mask can then take light or dark palette ink.
fn ink_mask() -> Option<egui::ColorImage> {
    let image = image::load_from_memory_with_format(
        include_bytes!("../../assets/themes/sumie-ink.png"),
        image::ImageFormat::Png,
    )
    .ok()?
    .into_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    let mut rgba = image.into_raw();
    for pixel in rgba.chunks_exact_mut(4) {
        let luminance =
            (54 * u32::from(pixel[0]) + 183 * u32::from(pixel[1]) + 19 * u32::from(pixel[2])) / 256;
        pixel[3] = ((u32::from(pixel[3]) * (255 - luminance) + 127) / 255) as u8;
        pixel[..3].fill(255);
    }
    Some(egui::ColorImage::from_rgba_unmultiplied(size, &rgba))
}

fn texture(ctx: &egui::Context) -> Option<TextureHandle> {
    let id = egui::Id::new("embedded-sumi-e-ink");
    if let Some(cached) = ctx.data_mut(|data| data.get_temp::<InkTexture>(id)) {
        return cached.0;
    }
    let texture =
        ink_mask().map(|image| ctx.load_texture("Sumi-e ink", image, egui::TextureOptions::LINEAR));
    // Cache failure as well: optional decoration must not retry decoding every frame.
    ctx.data_mut(|data| data.insert_temp(id, InkTexture(texture.clone())));
    texture
}

/// Try a bounded number of sizes, anchored to the visible canvas rather than
/// the scrollable document. Padding also protects carets and line-end selection.
fn placement(canvas: Rect, galley: &Galley, position: Pos2) -> Option<Rect> {
    if !canvas.is_finite() || !canvas.is_positive() || !position.is_finite() {
        return None;
    }
    let bounds = canvas.shrink(20.);
    if !bounds.is_positive() {
        return None;
    }
    let first = galley
        .rows
        .partition_point(|row| row.rect().bottom() + position.y < canvas.top() - 10.);
    let last = galley
        .rows
        .partition_point(|row| row.rect().top() + position.y <= canvas.bottom() + 10.);
    let rows = &galley.rows[first..last];
    let width = bounds.width().min(440.).min(bounds.height() * 1.5);
    for scale in [1., 0.85, 0.7, 0.55, 0.4] {
        let width = width * scale;
        if width < 144. {
            break;
        }
        let size = vec2(width, width / 1.5);
        let rect = Rect::from_min_size(bounds.max - size, size);
        if rows.iter().all(|row| {
            !row.rect()
                .translate(position.to_vec2())
                .expand2(vec2(24., 10.))
                .intersects(rect)
        }) {
            return Some(rect);
        }
    }
    None
}

impl Palette {
    /// Paint only in unused source-canvas space. Dense/narrow buffers suppress
    /// the vignette, and selecting text hides it until the selection is cleared.
    pub(crate) fn paint_source_artwork(
        self,
        ui: &egui::Ui,
        output: &egui::text_edit::TextEditOutput,
    ) {
        if self.artwork != Artwork::SumiE
            || output.cursor_range.is_some_and(|range| !range.is_empty())
        {
            return;
        }
        let canvas = Rect::from_min_max(
            pos2(
                ui.clip_rect().left().max(output.response.rect.left()),
                ui.clip_rect().top(),
            ),
            ui.clip_rect().max,
        );
        let Some(rect) = placement(canvas, &output.galley, output.galley_pos) else {
            return;
        };
        if let Some(texture) = texture(ui.ctx()) {
            ui.painter().with_clip_rect(canvas).image(
                texture.id(),
                rect,
                Rect::from_min_max(Pos2::ZERO, pos2(1., 1.)),
                self.muted.gamma_multiply(0.72),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Color32;

    #[test]
    fn embedded_ink_has_transparent_negative_space_and_reuses_its_texture() {
        let image = ink_mask().expect("The embedded illustration must decode");
        assert_eq!(image.size, [1536, 1024]);
        assert!(image.pixels.iter().any(|pixel| pixel.a() == 0));
        assert!(image.pixels.iter().any(|pixel| pixel.a() > 128));
        let ctx = egui::Context::default();
        let light = texture(&ctx).unwrap();
        let dark = texture(&ctx).unwrap();
        assert_eq!(light.id(), dark.id());
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
    fn painting_preserves_native_layout_focus_and_input_and_hides_for_selections() {
        let ctx = egui::Context::default();
        let mut source = String::from("fn main() {}\n");
        let id = egui::Id::new("artwork-source");
        let mut ink_id = None;
        for (theme, selected) in [
            ("sumie-light", false),
            ("sumie-light", true),
            ("sumie-dark", false),
            ("phosphor", false),
        ] {
            let palette = crate::theme::Theme::builtin(theme).unwrap().palette;
            crate::theme::install(&ctx, palette);
            ctx.memory_mut(|memory| memory.request_focus(id));
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000., 700.))),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let mut source_output = egui::TextEdit::multiline(&mut source)
                            .id(id)
                            .code_editor()
                            .frame(false)
                            .show(ui);
                        if selected {
                            source_output.cursor_range = Some(egui::text::CCursorRange::two(
                                egui::text::CCursor::new(0),
                                egui::text::CCursor::new(2),
                            ));
                        }
                        let next_id = ui.next_auto_id();
                        let cursor = ui.cursor();
                        palette.paint_source_artwork(ui, &source_output);
                        assert_eq!(ui.next_auto_id(), next_id);
                        assert_eq!(ui.cursor(), cursor);
                        assert!(source_output.response.has_focus());
                        assert!(!source_output.response.changed());
                    });
                },
            );
            let texture = texture(&ctx).unwrap().id();
            assert_eq!(*ink_id.get_or_insert(texture), texture);
            let paintings = output.shapes.iter().filter(|shape| {
                matches!(&shape.shape, egui::Shape::Mesh(mesh) if mesh.texture_id == texture)
            });
            assert_eq!(
                paintings.count(),
                usize::from(palette.artwork == Artwork::SumiE && !selected)
            );
            assert_eq!(source, "fn main() {}\n");
        }
    }
}
