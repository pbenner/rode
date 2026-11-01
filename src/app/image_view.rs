//! Read-only, per-tab raster preview. Textures are uploaded once and released
//! with the tab; encoded files and full decoding buffers are not cached here.
//!
//! A successful reload replaces the retained pixels/texture; a failed or stale reload leaves
//! the previous preview intact. Every source read uses the active document host and sandbox,
//! and preview tabs never enter source-save/tool paths.
use super::*;
use crate::images;

/// Preview interaction flags returned to its owner, including unused edge-scroll
/// movement for PDF page navigation; no reload is performed by the renderer.
pub(super) struct ImageResponse {
    pub reload: bool,
    /// Vertical wheel/trackpad movement left over at a page edge.
    pub edge_scroll: f32,
    pub hovered: bool,
    pub at_start: bool,
    pub at_end: bool,
}

#[derive(Clone, Copy)]
enum ScrollOrigin {
    Start,
    End,
}

/// Read-only decoded preview with lazy texture upload and a retained memory lease.
/// Fit/explicit zoom and scroll origin are local presentation state.
pub(super) struct ImageView {
    pub original: [u32; 2],
    pub format: String,
    encoded_bytes: usize,
    pixels: Option<Arc<egui::ColorImage>>,
    texture: Option<egui::TextureHandle>,
    preview_size: [usize; 2],
    _lease: Arc<images::Lease>,
    zoom: Option<f32>,
    displayed_zoom: f32,
    reset_scroll: Option<ScrollOrigin>,
    pub loading: bool,
}
impl ImageView {
    /// Retain decoded pixels and their lease; upload the GPU texture only on the first visible frame.
    pub fn new(decoded: images::Decoded) -> Self {
        Self {
            original: decoded.original,
            format: decoded.format,
            encoded_bytes: decoded.encoded_bytes,
            preview_size: decoded.pixels.size,
            pixels: Some(decoded.pixels),
            texture: None,
            _lease: decoded.lease,
            zoom: None,
            displayed_zoom: 1.,
            reset_scroll: Some(ScrollOrigin::Start),
            loading: false,
        }
    }
    /// Replace successfully decoded content while preserving the user's selected zoom mode.
    pub fn replace(&mut self, decoded: images::Decoded) {
        let zoom = self.zoom;
        *self = Self::new(decoded);
        self.zoom = zoom;
    }
    /// Multiply the displayed/explicit zoom, clamp it to 1–1600% and reveal the image start.
    pub fn zoom_by(&mut self, factor: f32) {
        self.zoom = Some((self.zoom.unwrap_or(self.displayed_zoom) * factor).clamp(0.01, 16.));
        self.reset_scroll = Some(ScrollOrigin::Start);
    }
    /// Request the bottom edge on the next render, used when entering a previous PDF page.
    pub fn scroll_to_end(&mut self) {
        self.reset_scroll = Some(ScrollOrigin::End);
    }
    /// Return a stable per-document preview focus identity shared by its keyboard actions.
    pub fn focus_id(id: u64) -> egui::Id {
        egui::Id::new(("image-preview", id))
    }
    /// Render cached pixels with fit/zoom/pan controls and return reload/edge-navigation intents.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        id: u64,
        palette: theme::Palette,
        focus: bool,
    ) -> ImageResponse {
        if let Some(pixels) = self.pixels.take() {
            self.texture = Some(ctx.load_texture(
                format!("image-{id}"),
                egui::ImageData::Color(pixels),
                egui::TextureOptions::LINEAR,
            ));
        }
        let mut reload = false;
        ui.horizontal_wrapped(|ui| {
            if ui.selectable_label(self.zoom.is_none(), "Fit").clicked() {
                self.zoom = None;
                self.reset_scroll = Some(ScrollOrigin::Start);
            }
            if ui
                .small_button("100%")
                .on_hover_text("Actual size in image pixels")
                .clicked()
            {
                self.zoom = Some(1.);
                self.reset_scroll = Some(ScrollOrigin::Start);
            }
            if ui.small_button("−").on_hover_text("Zoom out").clicked() {
                self.zoom_by(1. / 1.25);
            }
            if ui.small_button("+").on_hover_text("Zoom in").clicked() {
                self.zoom_by(1.25);
            }
            ui.label(palette.tag(format!("{:.0}%", self.displayed_zoom * 100.)));
            if ui
                .add_enabled(!self.loading, egui::Button::new("Reload").small())
                .clicked()
            {
                reload = true;
            }
            if self.loading {
                ui.spinner();
            }
            ui.label(palette.tag("READ ONLY"));
        });
        let metadata = format!(
            "{} · {} × {} px · {:.1} KiB{}",
            self.format,
            self.original[0],
            self.original[1],
            self.encoded_bytes as f64 / 1024.,
            if self.preview_size != [self.original[0] as usize, self.original[1] as usize] {
                " · reduced preview"
            } else {
                ""
            }
        );
        ui.add(egui::Label::new(palette.tag(&metadata)).truncate()).on_hover_text(format!("{metadata}\nScroll or drag to pan. Large images are downsampled to the GPU preview limit. Animated images show their first frame."));
        ui.separator();
        let viewport = ui.available_size().max(egui::vec2(1., 1.));
        let original = egui::vec2(self.original[0] as f32, self.original[1] as f32);
        let fit = ((viewport.x - 16.).max(1.) / original.x)
            .min((viewport.y - 16.).max(1.) / original.y)
            .min(1.);
        let zoom = self.zoom.unwrap_or(fit).max(0.001);
        if (self.displayed_zoom - zoom).abs() > f32::EPSILON {
            self.displayed_zoom = zoom;
            ctx.request_repaint();
        }
        // Retain the actual displayed scale as the starting point for +/- in Fit.
        let size = original * zoom;
        let mut scroll = egui::ScrollArea::both()
            .id_salt(("image-scroll", id))
            .auto_shrink([false, false])
            .scroll_source(egui::scroll_area::ScrollSource::ALL);
        let origin = self.reset_scroll.take();
        if let Some(origin) = origin {
            let y = if matches!(origin, ScrollOrigin::End) {
                (size.y + 16. - viewport.y).max(0.)
            } else {
                0.
            };
            scroll = scroll.scroll_offset(egui::vec2(0., y)).animated(false);
        }
        let mut output = scroll.show(ui, |ui| {
            let (canvas, _) =
                ui.allocate_exact_size(viewport.max(size + egui::vec2(16., 16.)), Sense::hover());
            let image_rect = egui::Rect::from_center_size(canvas.center(), size);
            let visible = image_rect.intersect(ui.clip_rect());
            let cell = 16_f32.max((visible.area() / 8000.).max(0.).sqrt());
            let painter = ui.painter().with_clip_rect(visible);
            painter.rect_filled(visible, 0, palette.raised);
            let start_x = ((visible.left() - image_rect.left()) / cell).floor() as i32;
            let start_y = ((visible.top() - image_rect.top()) / cell).floor() as i32;
            let end_x = ((visible.right() - image_rect.left()) / cell).ceil() as i32;
            let end_y = ((visible.bottom() - image_rect.top()) / cell).ceil() as i32;
            for y in start_y..end_y {
                for x in start_x..end_x {
                    if (x + y) % 2 == 0 {
                        painter.rect_filled(
                            egui::Rect::from_min_size(
                                image_rect.min + egui::vec2(x as f32 * cell, y as f32 * cell),
                                egui::vec2(cell, cell),
                            ),
                            0,
                            palette.panel,
                        );
                    }
                }
            }
            if let Some(texture) = &self.texture {
                painter.image(
                    texture.id(),
                    image_rect,
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
                    egui::Color32::WHITE,
                );
            }
            let response = ui.interact(canvas, Self::focus_id(id), Sense::click());
            if focus || response.clicked() {
                response.request_focus();
            }
            response.on_hover_text(format!(
                "{:.0}% · {} × {} px",
                zoom * 100.,
                self.original[0],
                self.original[1]
            ));
        });
        if matches!(origin, Some(ScrollOrigin::End)) {
            // Account for the scrollbars and clipping margins after layout.
            output.state.offset.y = (output.content_size.y - output.inner_rect.height()).max(0.);
            output.state.store(ctx, output.id);
            ctx.request_repaint();
        }
        ImageResponse {
            reload,
            hovered: ui.rect_contains_pointer(output.inner_rect),
            edge_scroll: if origin.is_some() {
                0.
            } else {
                ui.input(|input| input.smooth_scroll_delta.y)
            },
            at_start: output.state.offset.y <= 0.5,
            at_end: output.state.offset.y
                >= (output.content_size.y - output.inner_rect.height()).max(0.) - 0.5,
        }
    }
}

impl EditorApp {
    /// Route a zoom shortcut to the active image/PDF preview rather than source font sizing.
    pub(super) fn zoom_image_shortcut(&mut self, ctx: &egui::Context, factor: f32) -> bool {
        let Some(doc) = self.documents.get_mut(self.active) else {
            return false;
        };
        if ctx.memory(|m| m.has_focus(ImageView::focus_id(doc.id)))
            && let Some(image) = doc
                .image
                .as_mut()
                .or_else(|| doc.pdf.as_mut().map(|pdf| &mut pdf.image))
                .or_else(|| {
                    doc.marp
                        .as_mut()
                        .filter(|view| view.showing)
                        .and_then(|view| view.pdf.as_mut())
                        .map(|pdf| &mut pdf.image)
                })
        {
            image.zoom_by(factor);
            return true;
        }
        false
    }
    /// Queue a versioned read of the bound host path, retaining the old preview until replacement succeeds.
    pub(super) fn reload_image(&mut self, id: u64, ctx: &egui::Context) {
        let Some(doc) = self.documents.iter_mut().find(|d| d.id == id) else {
            return;
        };
        let Some(image) = &mut doc.image else {
            return;
        };
        if image.loading {
            return;
        }
        let Some(path) = doc.path.clone() else {
            return;
        };
        image.loading = true;
        doc.edit_version = doc.edit_version.wrapping_add(1);
        let version = doc.edit_version;
        self.dispatch(
            Request::ReadImage { path },
            Job::Reload { id, version },
            ctx,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn decoded(budget: &Arc<images::Budget>, width: u32, height: u32) -> images::Decoded {
        use std::io::Cursor;
        let mut data = Cursor::new(vec![]);
        ::image::DynamicImage::ImageRgba8(::image::RgbaImage::new(width, height))
            .write_to(&mut data, ::image::ImageFormat::Png)
            .unwrap();
        images::decode_bytes(data.get_ref(), "image.png", budget, 4096).unwrap()
    }
    fn opened(ctx: &egui::Context) -> EditorApp {
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.tx
            .send(Event::Done {
                generation: app.generation,
                job: Job::Open("image.png".into()),
                result: Box::new(Ok(files::Response {
                    path: "/image.png".into(),
                    revision: "initial".into(),
                    preview: Some(decoded(&app.image_budget, 4, 3)),
                    ..Default::default()
                })),
            })
            .unwrap();
        app.poll(ctx);
        app
    }
    #[test]
    fn previous_pdf_page_can_start_at_the_bottom_without_sticking_there() {
        let ctx = egui::Context::default();
        let budget = Arc::new(images::Budget::default());
        let mut image = ImageView::new(decoded(&budget, 100, 600));
        image.zoom_by(1.);
        let palette = theme::Themes::load(tempfile::tempdir().unwrap().path(), "carbon")
            .0
            .active()
            .palette;
        let frame = |time| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(500., 400.),
            )),
            time: Some(time),
            ..Default::default()
        };
        let mut edge = (false, false);
        let _ = ctx.run(frame(0.), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let result = image.ui(ui, ctx, 1, palette, false);
                edge = (result.at_start, result.at_end);
            });
        });
        assert_eq!(edge, (true, false));
        image.scroll_to_end();
        let _ = ctx.run(frame(0.1), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let result = image.ui(ui, ctx, 1, palette, false);
                edge = (result.at_start, result.at_end);
            });
        });
        assert_eq!(edge, (false, true));
        let mut input = frame(0.2);
        input
            .events
            .push(egui::Event::PointerMoved(egui::pos2(250., 250.)));
        input.events.push(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0., 60.),
            modifiers: egui::Modifiers::NONE,
        });
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let result = image.ui(ui, ctx, 1, palette, false);
                edge = (result.at_start, result.at_end);
            });
        });
        assert_eq!(
            edge,
            (false, false),
            "Scrolling up leaves the bottom of the page"
        );
    }

    #[test]
    fn image_save_commands_cannot_write_empty_text_or_open_save_as() {
        let ctx = egui::Context::default();
        let mut app = opened(&ctx);
        let id = app.documents[0].id;
        app.save_active(&ctx, false);
        app.save_active(&ctx, true);
        app.save_document(id, "/other.rs".into(), true, &ctx);
        assert!(app.dialog == Dialog::None);
        assert!(!app.documents[0].saving);
        assert!(!app.documents[0].dirty());
        assert!(app.documents[0].image.is_some());
        assert_eq!(app.documents[0].revision.as_deref(), Some("initial"));
    }
    #[test]
    fn image_reload_discards_stale_results_and_keeps_the_last_preview_on_error() {
        let ctx = egui::Context::default();
        let mut app = opened(&ctx);
        let budget = app.image_budget.clone();
        let id = app.documents[0].id;
        app.documents[0].edit_version = 5;
        app.documents[0].image.as_mut().unwrap().loading = true;
        for (version, width) in [(4, 2), (5, 8)] {
            app.tx
                .send(Event::Done {
                    generation: app.generation,
                    job: Job::Reload { id, version },
                    result: Box::new(Ok(files::Response {
                        path: "/image.png".into(),
                        revision: "updated".into(),
                        preview: Some(decoded(&budget, width, 4)),
                        ..Default::default()
                    })),
                })
                .unwrap();
        }
        app.poll(&ctx);
        assert_eq!(app.documents[0].image.as_ref().unwrap().original, [8, 4]);
        assert_eq!(budget.used(), 8 * 4 * 4);
        assert!(!app.documents[0].dirty());
        let version = app.documents[0].edit_version;
        app.documents[0].image.as_mut().unwrap().loading = true;
        app.tx
            .send(Event::Done {
                generation: app.generation,
                job: Job::Reload { id, version },
                result: Box::new(Err("Image transfer failed".into())),
            })
            .unwrap();
        app.poll(&ctx);
        assert_eq!(app.documents[0].image.as_ref().unwrap().original, [8, 4]);
        assert!(!app.documents[0].image.as_ref().unwrap().loading);
        drop(app);
        assert_eq!(budget.used(), 0);
    }
}
