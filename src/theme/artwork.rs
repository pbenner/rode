//! Embedded, static ink illustrations. Placement uses the editor's actual visible
//! text rows, so artwork never changes source contrast or owns an input region.
//! Each alpha-mask texture is shared by every viewport of an egui context; theme
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
    /// Original pine/crane, bamboo and plum-blossom ink-wash illustrations.
    SumiE,
}

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
enum Motif {
    Pine,
    Bamboo,
    Plum,
}

impl Motif {
    fn bytes(self) -> &'static [u8] {
        match self {
            Self::Pine => include_bytes!("../../assets/themes/sumie-ink.png"),
            Self::Bamboo => include_bytes!("../../assets/themes/sumie-bamboo.png"),
            Self::Plum => include_bytes!("../../assets/themes/sumie-plum.png"),
        }
    }
}

#[derive(Clone)]
struct InkTexture(Option<TextureHandle>);

/// Treat dark strokes as ink coverage rather than retaining a paper-colored
/// backdrop. The same transparent mask can then take light or dark palette ink.
fn ink_mask(motif: Motif) -> Option<egui::ColorImage> {
    let image = image::load_from_memory_with_format(motif.bytes(), image::ImageFormat::Png)
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

fn texture(ctx: &egui::Context, motif: Motif) -> Option<TextureHandle> {
    let id = egui::Id::new(("embedded-sumi-e-ink", motif));
    if let Some(cached) = ctx.data_mut(|data| data.get_temp::<InkTexture>(id)) {
        return cached.0;
    }
    let texture = ink_mask(motif)
        .map(|image| ctx.load_texture("Sumi-e ink", image, egui::TextureOptions::LINEAR));
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
    let first = galley
        .rows
        .partition_point(|row| row.rect().bottom() + position.y < canvas.top() - 10.);
    let last = galley
        .rows
        .partition_point(|row| row.rect().top() + position.y <= canvas.bottom() + 10.);
    let rows = &galley.rows[first..last];
    corner_placement(canvas, 1.5, 440., 144., |rect| {
        rows.iter().all(|row| {
            !row.rect()
                .translate(position.to_vec2())
                .expand2(vec2(24., 10.))
                .intersects(rect)
        })
    })
}

/// Fit decoration inside genuinely unused space without allocating layout or
/// input. Every consumer shares the same finite bounds, padding and size budget.
fn corner_placement(
    canvas: Rect,
    aspect: f32,
    maximum_width: f32,
    minimum_width: f32,
    clear: impl Fn(Rect) -> bool,
) -> Option<Rect> {
    if !canvas.is_finite() || !canvas.is_positive() {
        return None;
    }
    let bounds = canvas.shrink(20.);
    if !bounds.is_positive() {
        return None;
    }
    let width = bounds
        .width()
        .min(maximum_width)
        .min(bounds.height() * aspect);
    for scale in [1., 0.85, 0.7, 0.55, 0.4] {
        let width = width * scale;
        if width < minimum_width {
            break;
        }
        let size = vec2(width, width / aspect);
        let rect = Rect::from_min_size(bounds.max - size, size);
        if clear(rect) {
            return Some(rect);
        }
    }
    None
}

impl Palette {
    /// Paint only in unused source-canvas space. Selection does not hide the
    /// vignette; dense/narrow buffers still suppress it to protect source text.
    pub(crate) fn paint_source_artwork(
        self,
        ui: &egui::Ui,
        output: &egui::text_edit::TextEditOutput,
    ) {
        if self.artwork != Artwork::SumiE {
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
        self.paint_ink(ui, Motif::Pine, rect, canvas);
    }

    /// The file tree owns its full scroll region. Paint bamboo only in whatever
    /// space remains below it, never under rows, scrollbars or sidebar controls.
    pub(crate) fn paint_explorer_artwork(self, ui: &egui::Ui) {
        if self.artwork != Artwork::SumiE {
            return;
        }
        let canvas = ui.available_rect_before_wrap().intersect(ui.clip_rect());
        if let Some(rect) = corner_placement(canvas, 2. / 3., 180., 90., |_| true) {
            self.paint_ink(ui, Motif::Bamboo, rect, canvas);
        }
    }

    /// Welcome text and actions report their occupied bounds; the blossom branch
    /// yields to them in compact windows instead of becoming their background.
    pub(crate) fn paint_welcome_artwork(self, ui: &egui::Ui, canvas: Rect, occupied: Rect) {
        if self.artwork != Artwork::SumiE {
            return;
        }
        let canvas = canvas.intersect(ui.clip_rect());
        if let Some(rect) = corner_placement(canvas, 1.5, 440., 144., |rect| {
            !occupied.expand(16.).intersects(rect)
        }) {
            self.paint_ink(ui, Motif::Plum, rect, canvas);
        }
    }

    fn paint_ink(self, ui: &egui::Ui, motif: Motif, rect: Rect, canvas: Rect) {
        if let Some(texture) = texture(ui.ctx(), motif) {
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
        let ctx = egui::Context::default();
        let mut ids = Vec::new();
        for motif in [Motif::Pine, Motif::Bamboo, Motif::Plum] {
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
        for theme in ["sumie-light", "sumie-dark", "phosphor"] {
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
                    let motif = if sidebar { Motif::Bamboo } else { Motif::Plum };
                    let id = texture(&ctx, motif).unwrap().id();
                    let painted: Vec<_> = output
                        .shapes
                        .iter()
                        .filter_map(|shape| match &shape.shape {
                            egui::Shape::Mesh(mesh) if mesh.texture_id == id => {
                                Some((shape.clip_rect, mesh.calc_bounds()))
                            }
                            _ => None,
                        })
                        .collect();
                    assert_eq!(
                        painted.len(),
                        usize::from(palette.artwork == Artwork::SumiE && size.y > 100.)
                    );
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
        for theme in ["sumie-light", "sumie-dark", "phosphor"] {
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
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000., 700.))),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
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
                            palette.paint_source_artwork(ui, &source_output);
                            assert_eq!(ui.next_auto_id(), next_id);
                            assert_eq!(ui.cursor(), cursor);
                            assert!(source_output.response.has_focus());
                            assert!(!source_output.response.changed());
                        });
                    },
                );
                let texture = texture(&ctx, Motif::Pine).unwrap().id();
                assert_eq!(*ink_id.get_or_insert(texture), texture);
                let paintings = output
                    .shapes
                    .iter()
                    .filter_map(|shape| match &shape.shape {
                        egui::Shape::Mesh(mesh) if mesh.texture_id == texture => Some(mesh),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                assert_eq!(
                    paintings.len(),
                    usize::from(palette.artwork == Artwork::SumiE)
                );
                if let Some(painting) = paintings.first() {
                    let rect = painting.calc_bounds();
                    assert_eq!(*artwork_rect.get_or_insert(rect), rect);
                }
                assert_eq!(source, original);
            }
        }
    }
}
