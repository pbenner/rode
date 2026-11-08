//! Project discovery keeps navigation and confirmation visible while access
//! options scroll independently. Host preparation still uses the common guards.
use super::*;

#[cfg(test)]
mod tests;

impl EditorApp {
    /// Browse on the selected host and review access before preparing a project.
    pub(super) fn folder_dialog(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        let top = ui.cursor().min.y;
        let open_by_key = ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::Enter));
        if self.folder_needs_load {
            self.folder_needs_load = false;
            self.browse_folder(self.dialog_input.clone(), ctx);
        }
        let host = self
            .workspace
            .as_ref()
            .and_then(|w| w.remote.as_ref())
            .map(|r| r.profile.endpoint())
            .unwrap_or_default();
        let title = |ui: &mut egui::Ui| {
            ui.label(
                RichText::new("Open project")
                    .size(palette.ui.map_or(20., |style| style.heading_size))
                    .strong()
                    .color(palette.heading_color()),
            );
        };
        let host_label = |ui: &mut egui::Ui| {
            egui::Frame::new()
                .fill(palette.raised)
                .corner_radius(4)
                .inner_margin(egui::Margin::symmetric(8, 4))
                .show(ui, |ui| {
                    let label = if self.folder_remote {
                        format!("SSH · {host}")
                    } else {
                        "This machine".into()
                    };
                    let response = ui.add(egui::Label::new(palette.tag(&label)).truncate());
                    ui_probe::button(
                        ui,
                        if self.folder_remote {
                            "project-dialog-host:ssh"
                        } else {
                            "project-dialog-host:local"
                        },
                        &response,
                    );
                    response.on_hover_text(label);
                });
        };
        if ui.available_width() >= 480. {
            ui.horizontal(|ui| {
                title(ui);
                ui.with_layout(Layout::right_to_left(Align::Center), host_label);
            });
        } else {
            title(ui);
            host_label(ui);
        }
        ui.add_space(12.);
        let path = self.project_folder_path(ui, ctx);
        if let Some(error) = &self.folder_error {
            let mut text = egui::text::LayoutJob::simple(
                error.clone(),
                egui::TextStyle::Body.resolve(ui.style()),
                palette.error,
                ui.available_width(),
            );
            text.wrap.max_rows = 3;
            text.wrap.break_anywhere = true;
            let response = ui.add(egui::Label::new(text)).on_hover_text(error);
            ui_probe::button(ui, "project-dialog-error", &response);
        }
        ui.add_space(12.);

        // Reserve the footer before measuring the scrollable body. Neither an
        // expanded rules editor nor a short viewport may hide confirmation.
        let footer_height = layout::control_height(ui)
            .max(palette.ui.map_or(32., |style| style.control_height))
            + 24.;
        let body_height =
            (ctx.content_rect().height() - 64. - (ui.cursor().min.y - top) - footer_height).max(1.);
        egui::ScrollArea::vertical()
            .id_salt("project-dialog-body")
            .auto_shrink([false, true])
            .min_scrolled_height(body_height)
            .max_height(body_height)
            .show(ui, |ui| {
                self.project_folder_browser(ui, ctx);
                ui.add_space(12.);
                ui.horizontal_wrapped(|ui| {
                    workbench::sandbox_backend_ui(ui, &mut self.folder_sandbox);
                });
                if self.folder_sandbox.enabled {
                    layout::subtitle(
                        ui,
                        "If sandboxing is unavailable, this project opens in labelled normal mode.",
                        palette,
                    );
                }
                let options = egui::CollapsingHeader::new("Access rules and presets")
                    .id_salt("project-access-options")
                    .show(ui, |ui| {
                        ui.push_id("folder-sandbox-presets", |ui| {
                            self.sandbox_presets
                                .ui(ui, &mut self.folder_sandbox, palette);
                        });
                        ui.add_space(8.);
                        ui.push_id("folder-sandbox-rules", |ui| {
                            workbench::sandbox_rules_ui(ui, &mut self.folder_sandbox, palette);
                        });
                    });
                ui_probe::button(ui, "project-dialog-options", &options.header_response);
                if options.fully_open() {
                    ui_probe::button(ui, "project-dialog-options-open", &options.header_response);
                }
                if options.header_response.clicked() {
                    ui.scroll_to_rect(options.header_response.rect, Some(Align::Min));
                }
            });
        if !self.folder_loading
            && self.folder_error.is_none()
            && self.dialog_input == self.folder_policy_path
        {
            ui_probe::button(ui, "project-dialog-ready", &path);
        }
        ui.add_space(8.);
        ui.separator();
        ui.add_space(8.);
        ui.horizontal(|ui| {
            if ui.available_width() >= 360. {
                ui.label(palette.tag("Ctrl+Enter to open"));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let open = palette.primary(ui, "Open project");
                ui_probe::button(ui, "project-dialog-open", &open);
                if open.clicked() || open_by_key {
                    let options = self.folder_sandbox.clone();
                    if let Err(error) = options.validate() {
                        self.folder_error = Some(error.to_string());
                    } else {
                        self.sandbox_defaults = options.clone();
                        self.workbench.preferences_pending = true;
                        self.dialog = Dialog::None;
                        self.guarded(
                            Action::Project {
                                path: self.dialog_input.clone(),
                                remote: self.folder_remote,
                                options,
                            },
                            ctx,
                        );
                    }
                }
                let cancel = ui.button("Cancel");
                ui_probe::button(ui, "project-dialog-cancel", &cancel);
                if cancel.clicked() {
                    self.dialog = Dialog::None;
                }
            });
        });
    }

    fn project_folder_path(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) -> egui::Response {
        let input_id = egui::Id::new("project-dialog-path");
        if self.dialog_focus || ui.memory(|memory| memory.focused().is_none()) {
            ui.memory_mut(|memory| memory.request_focus(input_id));
            self.dialog_focus = false;
        }
        let height = layout::control_height(ui);
        let browse_width = ui.fonts_mut(|fonts| {
            fonts
                .layout_no_wrap(
                    "Browse".into(),
                    egui::TextStyle::Button.resolve(ui.style()),
                    ui.visuals().text_color(),
                )
                .size()
                .x
                + 2. * ui.spacing().button_padding.x
        });
        let mut browse = None;
        let response = ui
            .horizontal(|ui| {
                let up = ui.add_sized(
                    [height, height],
                    IconButton::icon(Icon::ChevronUp, "Parent folder").icon_size(16.),
                );
                ui_probe::button(ui, "project-dialog-up", &up);
                if up.clicked() {
                    browse = Some(project_parent(&self.dialog_input));
                }
                let width =
                    (ui.available_width() - browse_width - ui.spacing().item_spacing.x).max(1.);
                let path = ui.add_sized(
                    [width, height],
                    egui::TextEdit::singleline(&mut self.dialog_input)
                        .id(input_id)
                        .vertical_align(Align::Center)
                        .desired_width(f32::INFINITY)
                        .hint_text("Project folder"),
                );
                ui_probe::button(ui, "project-dialog-path", &path);
                let enter = (path.has_focus() || path.lost_focus())
                    && ui.input(|i| i.key_pressed(Key::Enter));
                let button = ui.add_sized([browse_width, height], egui::Button::new("Browse"));
                ui_probe::button(ui, "project-dialog-browse", &button);
                if button.clicked() || enter {
                    browse = Some(self.dialog_input.clone());
                }
                path.on_hover_text("Enter an absolute path or ~/folder, then Browse.")
            })
            .inner;
        if let Some(path) = browse {
            self.browse_folder(path, ctx);
        }
        response
    }

    fn project_folder_browser(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        let count = self
            .folder_entries
            .iter()
            .filter(|entry| self.show_hidden || !entry.name.starts_with('.'))
            .count();
        let mut browse = None;
        egui::Frame::new()
            .fill(palette.background)
            .stroke(Stroke::new(1_f32, palette.border))
            .corner_radius(6)
            .inner_margin(10)
            .show(ui, |ui| {
                ui.set_width(ui.available_width().max(1.));
                ui.horizontal(|ui| {
                    if self.folder_loading {
                        ui.spinner();
                        ui.label(palette.tag("Loading folders…"));
                    } else {
                        ui.label(palette.tag(format!("FOLDERS · {count}")));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let hidden = ui.checkbox(&mut self.show_hidden, "Show hidden");
                        ui_probe::button(ui, "project-dialog-show-hidden", &hidden);
                        if hidden.changed() {
                            self.tree_dirty = true;
                        }
                    });
                });
                ui.add_space(6.);
                let row_pitch = layout::control_height(ui).max(25.) + 2.;
                let height = (count as f32 * row_pitch + 4.)
                    .clamp(64., (ctx.content_rect().height() * 0.3).clamp(64., 220.));
                let query = format!("folder:{}:{}", self.folder_token, self.show_hidden);
                let listing = ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), height),
                    Layout::top_down(Align::Min),
                    |ui| {
                        file_browser::rows(
                            ui,
                            "project-folder-browser",
                            &self.folder_entries,
                            self.show_hidden,
                            &mut self.navigation.list,
                            &query,
                            (0, false),
                            None,
                            !self.folder_loading,
                        )
                    },
                );
                ui_probe::button(ui, "project-dialog-folders", &listing.response);
                if let Some(action) = listing.inner {
                    browse = Some(match action {
                        file_browser::Choice::Parent => project_parent(&self.dialog_input),
                        file_browser::Choice::Select(entry) | file_browser::Choice::Open(entry) => {
                            entry.path
                        }
                    });
                }
                if count == 0 && !self.folder_loading && self.folder_error.is_none() {
                    ui.put(
                        listing.response.rect,
                        egui::Label::new(
                            RichText::new("No subfolders. You can open this folder.")
                                .color(palette.muted),
                        )
                        .wrap(),
                    );
                }
            });
        if let Some(path) = browse {
            self.browse_folder(path, ctx);
        }
    }
}

fn project_parent(path: &str) -> String {
    std::path::Path::new(path)
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .map_or_else(|| "/".into(), |path| path.to_string_lossy().into())
}
