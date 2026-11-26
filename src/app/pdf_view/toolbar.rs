//! Fixed, compact controls shared by PDF, PowerPoint, Word and slide previews.
//! Secondary reading choices expand independently of page scrolling and native fullscreen.
use super::*;

/// Deferred navigation/reload intents; drawing the toolbar performs no host I/O.
pub(super) struct Controls {
    pub page: Option<usize>,
    pub reload: bool,
    pub toggle_fullscreen: bool,
}
impl PdfView {
    /// Draw the primary row and optional settings above the shared document boundary.
    pub(super) fn toolbar(
        &mut self,
        ui: &mut egui::Ui,
        palette: theme::Palette,
        display: Display,
    ) -> Controls {
        let kind = if matches!(display, Display::Slides) || self.source.format == pdf::Format::Pptx
        {
            "Slide"
        } else {
            "Page"
        };
        let limit = ui.ctx().input(|i| i.max_texture_side);
        let selectable =
            self.source.format.supports_text() && matches!(display, Display::Document { .. });
        let mut page = None;
        let mut toggle_fullscreen = false;
        let mut reload_requested = false;
        let arrow_size = egui::Vec2::splat(layout::control_height(ui));
        let toolbar = layout::document_toolbar(ui, palette, |ui| {
            ui.horizontal_wrapped(|ui| {
            let previous = ui.add_enabled(
                !self.reloading && self.wanted > 1,
                IconButton::icon(Icon::ChevronLeft, "Previous page").min_size(arrow_size),
            );
            ui_probe::button(ui, "pdf-previous", &previous);
            if previous.clicked() {
                page = Some(self.wanted - 1);
            }
            ui.label(kind);
            let number = ui.add_enabled(
                !self.reloading,
                crate::keyboard::named(
                    "Page number",
                    egui::DragValue::new(&mut self.requested)
                        .range(1..=self.source.pages)
                        .speed(0.1),
                ),
            );
            ui_probe::button(ui, "pdf-page-number", &number);
            ui_probe::button(ui, &format!("pdf-current-page:{}", self.page), &number);
            ui.label(format!("of {}", self.source.pages));
            let go = ui.add_enabled(!self.reloading, crate::keyboard::Button::new("Go"));
            ui_probe::button(ui, "pdf-go", &go);
            if go.clicked() || (number.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter))) {
                page = Some(self.requested);
            }
            let next = ui.add_enabled(
                !self.reloading && self.wanted < self.source.pages,
                IconButton::icon(Icon::ChevronRight, "Next page").min_size(arrow_size),
            );
            ui_probe::button(ui, "pdf-next", &next);
            if next.clicked() {
                page = Some(self.wanted + 1);
            }
            self.image.zoom_controls(ui, palette);
            if selectable { self.text.search_button(ui); }
            let settings = ui.add(crate::keyboard::Button::new("View settings").selected(self.settings_open));
            ui_probe::button(ui, "pdf-view-settings", &settings);
            if settings.clicked() { self.settings_open = !self.settings_open; }
            let format = ui.label(palette.tag(if matches!(display, Display::Slides) { "MARP" } else { self.source.format.label() }))
                .on_hover_text(match self.source.format {
                    pdf::Format::Pptx => "Native PowerPoint preview. Uses bundled Inter when original fonts are unavailable; layout may differ from PowerPoint.",
                    pdf::Format::Docx => "Native Word preview with bundled fonts. Font substitution and unsupported content may change layout and pagination.",
                    pdf::Format::Pdf => "Native PDF preview",
                });
            ui_probe::button(ui, &format!("preview-format:{}", self.source.format.label()), &format);
            if !self.source.warnings.is_empty() {
                let warning = ui.label(palette.tag("Preview limitations")).on_hover_text(self.source.warnings.join("\n"));
                ui_probe::button(ui, "document-preview-warning", &warning);
            }
            let skipped = self.source.skipped_images(self.page);
            if skipped > 0 {
                let unit = if skipped == 1 { "image" } else { "images" };
                let warning = ui.colored_label(palette.error, format!("{skipped} {unit} could not be displayed"));
                ui_probe::button(ui, "presentation-missing-images", &warning);
            }
            if let Display::Document { fullscreen } = display {
                let button = ui
                    .add(
                        crate::keyboard::Button::new(if fullscreen {
                            "Exit fullscreen"
                        } else {
                            "Fullscreen"
                        })
                        .shortcut_text("F11"),
                    )
                    .on_hover_text("Toggle reader fullscreen · F11\nEscape exits fullscreen");
                ui_probe::button(ui, "pdf-fullscreen-toggle", &button);
                toggle_fullscreen = button.clicked();
            }
            if self.image.loading {
                ui.spinner().on_hover_text(format!("Loading {kind} {}…", self.wanted));
            }
        });
            if self.settings_open {
                ui.add_space(4.);
                ui.horizontal_wrapped(|ui| {
                    if matches!(display, Display::Document { .. }) {
                        for (value, label, name) in [
                            (false, "Single page", "pdf-single"),
                            (true, "Continuous", "pdf-continuous"),
                        ] {
                            let mode = crate::keyboard::selectable_label(
                                ui,
                                self.continuous == value,
                                label,
                            );
                            ui_probe::button(ui, name, &mode);
                            if mode.clicked() && self.continuous != value {
                                self.continuous = value;
                                self.jump = Some((self.page, PageEntry::Start));
                                self.scroll.reset();
                                self.wanted = self.page;
                                if !value {
                                    self.cached_page(self.page, limit);
                                    self.textures.clear();
                                }
                            }
                        }
                    }
                    self.image.fit_controls(ui, true);
                    let reload =
                        ui.add_enabled(!self.reloading, crate::keyboard::Button::new("Reload"));
                    ui_probe::button(ui, "pdf-reload", &reload);
                    reload_requested = reload.clicked();
                    ui.label(palette.tag("READ ONLY"));
                });
            }
            if selectable {
                self.text.search_ui(ui, self.source.format);
            }
        });
        ui_probe::button(ui, "pdf-toolbar", &toolbar.response);
        Controls {
            page,
            reload: reload_requested,
            toggle_fullscreen,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine as _, engine::general_purpose::STANDARD};

    #[test]
    fn document_headers_are_compact_and_keep_a_separate_canvas_at_all_sizes() {
        for (format, bytes) in [
            (
                pdf::Format::Pdf,
                include_bytes!("../../../tests/fixtures/pages.pdf").as_slice(),
            ),
            (
                pdf::Format::Pptx,
                include_bytes!("../../../tests/fixtures/slides.pptx").as_slice(),
            ),
            (
                pdf::Format::Docx,
                include_bytes!("../../../tests/fixtures/word.docx").as_slice(),
            ),
        ] {
            let budget = Arc::new(crate::images::Budget::default());
            let loaded = pdf::load_format_cancellable(
                &STANDARD.encode(bytes),
                format,
                1,
                &budget,
                128,
                &AtomicBool::new(false),
            )
            .unwrap();
            for theme in ["atelier-dark", "atelier-light"] {
                for width in [320., 650., 1100.] {
                    for scale in [1., 1.5, 2.] {
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
                        let mut view = PdfView::new(loaded.clone());
                        for expanded in [false, true] {
                            view.settings_open = expanded;
                            for _ in 0..3 {
                                let mut top = 0.;
                                let mut body = egui::Rect::NOTHING;
                                let output = ctx.run(
                                    egui::RawInput {
                                        screen_rect: Some(egui::Rect::from_min_size(
                                            egui::Pos2::ZERO,
                                            egui::vec2(width, 560.),
                                        )),
                                        ..Default::default()
                                    },
                                    |ctx| {
                                        egui::CentralPanel::default().show(ctx, |ui| {
                                            top = ui.cursor().top();
                                            view.toolbar(
                                                ui,
                                                palette,
                                                Display::Document { fullscreen: false },
                                            );
                                            body = ui.available_rect_before_wrap();
                                        });
                                    },
                                );
                                let detail = format!(
                                    "{format:?}/{theme}/{width}/{scale}, expanded={expanded}"
                                );
                                assert!(body.height() >= 160., "{detail}: {body:?}");
                                if !expanded {
                                    assert!(
                                        body.top() - top < if width >= 1100. { 65. } else { 150. },
                                        "{detail}: {}",
                                        body.top() - top
                                    );
                                }
                                let labels: Vec<_> = output
                                    .shapes
                                    .iter()
                                    .filter_map(|s| match &s.shape {
                                        egui::Shape::Text(text) => Some(text.galley.text()),
                                        _ => None,
                                    })
                                    .collect();
                                for label in ["Go", "View settings", "Fullscreen"] {
                                    assert!(labels.contains(&label), "{detail}: {label}");
                                }
                                for label in [
                                    "Single page",
                                    "Continuous",
                                    "Fit page",
                                    "Fit width",
                                    "Reload",
                                ] {
                                    assert_eq!(
                                        labels.contains(&label),
                                        expanded,
                                        "{detail}: {label}"
                                    );
                                }
                                assert!(output.shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Rect(rect) if rect.fill == palette.panel && rect.rect.width() >= width-20. && (rect.rect.top()-top).abs()<1.)), "{detail}: partial header surface");
                                assert!(output.shapes.iter().any(|s| matches!(&s.shape, egui::Shape::LineSegment { points, stroke } if stroke.width == 1.5 && points[0].y < body.top() && points[0].y > top && points[1].x-points[0].x >= width-20.)), "{detail}: missing divider");
                            }
                        }
                        assert_eq!(view.page, 1);
                        assert!(Arc::ptr_eq(&view.source, &loaded.source));
                    }
                }
            }
        }
    }
}
