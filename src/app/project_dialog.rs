//! Project discovery keeps navigation and confirmation visible while access
//! options scroll independently. Host preparation still uses the common guards.
use super::*;

#[cfg(test)]
mod tests;

/// Owns candidate-host discovery independently of document pickers and other modals.
/// A reply must match both request token and the still-visible path. Canonical
/// policy restoration is allowed only while the submitted options remain intact.
#[derive(Default)]
pub(super) struct State {
    pub remote: bool,
    pub path: String,
    pub entries: Vec<Entry>,
    pub loading: bool,
    pub needs_load: bool,
    pub token: u64,
    pub error: Option<String>,
    pub policy_path: String,
    pub sandbox: sandbox::Options,
    selection: navigation::List,
}

/// A browse intent captures policy before later controls in the same frame edit it.
pub(super) struct Browse {
    pub path: String,
    pub remote: bool,
    pub token: u64,
    pub options: sandbox::Options,
}

enum Intent {
    Browse(Browse),
    Open {
        path: String,
        remote: bool,
        options: sandbox::Options,
    },
    Cancel,
}

impl State {
    pub(super) fn invalidate(&mut self) {
        self.token = self.token.wrapping_add(1);
    }

    pub(super) fn reset(&mut self) {
        let token = self.token.wrapping_add(1);
        // Encrypted-project controls inherit the last access draft. Invalidating
        // discovery must not silently replace those choices with defaults.
        let sandbox = std::mem::take(&mut self.sandbox);
        *self = Self {
            token,
            sandbox,
            ..Default::default()
        };
    }

    pub(super) fn open(
        &mut self,
        remote: bool,
        path: String,
        policy_path: String,
        sandbox: sandbox::Options,
    ) {
        self.reset();
        self.remote = remote;
        self.path = path;
        self.policy_path = policy_path;
        self.sandbox = sandbox;
        self.needs_load = true;
    }

    pub(super) fn begin(&mut self, path: String) -> Browse {
        self.path = path.clone();
        self.token = self.token.wrapping_add(1);
        self.loading = true;
        self.error = None;
        Browse {
            remote: self.remote,
            token: self.token,
            path,
            options: self.sandbox.clone(),
        }
    }

    pub(super) fn matches(&self, token: u64, path: &str) -> bool {
        self.token == token && self.path == path
    }

    pub(super) fn loaded(
        &mut self,
        response: Response,
        submitted: &sandbox::Options,
        saved: Option<sandbox::Options>,
    ) {
        if self.policy_path != response.path {
            if self.sandbox == *submitted
                && let Some(saved) = saved
            {
                self.sandbox = saved;
            }
            self.policy_path.clone_from(&response.path);
        }
        self.path = response.path;
        self.entries = response.entries.into_iter().filter(|e| e.dir).collect();
        self.loading = false;
        self.error = None;
    }
}

impl EditorApp {
    /// Execute selected-host discovery separately from sandboxed document browsing.
    pub(super) fn folder_dialog(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.project_picker.needs_load {
            self.project_picker.needs_load = false;
            self.browse_folder(self.project_picker.path.clone(), ctx);
        }
        let host = self
            .workspace
            .as_ref()
            .and_then(|w| w.remote.as_ref())
            .map(|r| r.profile.endpoint())
            .unwrap_or_default();
        let old_hidden = self.show_hidden;
        let intents = self.project_picker.ui(
            ui,
            self.themes.active().palette,
            &host,
            &mut self.sandbox_presets,
            &mut self.dialog_focus,
            &mut self.show_hidden,
        );
        if self.show_hidden != old_hidden {
            self.explorer_state.tree_dirty = true;
        }
        for intent in intents {
            match intent {
                Intent::Browse(browse) => self.dispatch_folder_browse(browse, ctx),
                Intent::Open {
                    path,
                    remote,
                    options,
                } => {
                    self.sandbox_defaults = options.clone();
                    self.workbench.preferences_pending = true;
                    self.dialog = Dialog::None;
                    self.guarded(
                        Action::Project {
                            path,
                            remote,
                            options,
                        },
                        ctx,
                    );
                }
                Intent::Cancel => self.dialog = Dialog::None,
            }
        }
    }
}

impl State {
    /// Browse on the selected host and review access before preparing a project.
    fn ui(
        &mut self,
        ui: &mut egui::Ui,
        palette: theme::Palette,
        host: &str,
        presets: &mut sandbox_presets::State,
        focus: &mut bool,
        show_hidden: &mut bool,
    ) -> Vec<Intent> {
        let ctx = ui.ctx().clone();
        let mut intents = Vec::new();
        let top = ui.cursor().min.y;
        let open_by_key = ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::Enter));
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
                    let label = if self.remote {
                        format!("SSH · {host}")
                    } else {
                        "This machine".into()
                    };
                    let response = ui.add(egui::Label::new(palette.tag(&label)).truncate());
                    ui_probe::button(
                        ui,
                        if self.remote {
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
        let (path, browse) = self.project_folder_path(ui, focus);
        if let Some(path) = browse {
            intents.push(Intent::Browse(self.begin(path)));
        }
        if let Some(error) = &self.error {
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
                if let Some(path) = self.project_folder_browser(ui, &ctx, palette, show_hidden) {
                    intents.push(Intent::Browse(self.begin(path)));
                }
                ui.add_space(12.);
                ui.horizontal_wrapped(|ui| {
                    workbench::sandbox_backend_ui(ui, &mut self.sandbox);
                });
                if self.sandbox.enabled {
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
                            presets.ui(ui, &mut self.sandbox, palette);
                        });
                        ui.add_space(8.);
                        ui.push_id("folder-sandbox-rules", |ui| {
                            workbench::sandbox_rules_ui(ui, &mut self.sandbox, palette);
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
        if !self.loading && self.error.is_none() && self.path == self.policy_path {
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
                    let options = self.sandbox.clone();
                    if let Err(error) = options.validate() {
                        self.error = Some(error.to_string());
                    } else {
                        intents.push(Intent::Open {
                            path: self.path.clone(),
                            remote: self.remote,
                            options,
                        });
                    }
                }
                let cancel = ui.button("Cancel");
                ui_probe::button(ui, "project-dialog-cancel", &cancel);
                if cancel.clicked() {
                    intents.push(Intent::Cancel);
                }
            });
        });
        intents
    }

    fn project_folder_path(
        &mut self,
        ui: &mut egui::Ui,
        focus: &mut bool,
    ) -> (egui::Response, Option<String>) {
        let input_id = egui::Id::new("project-dialog-path");
        if *focus || ui.memory(|memory| memory.focused().is_none()) {
            ui.memory_mut(|memory| memory.request_focus(input_id));
            *focus = false;
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
                    browse = Some(project_parent(&self.path));
                }
                let width =
                    (ui.available_width() - browse_width - ui.spacing().item_spacing.x).max(1.);
                let path = ui.add_sized(
                    [width, height],
                    egui::TextEdit::singleline(&mut self.path)
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
                    browse = Some(self.path.clone());
                }
                path.on_hover_text("Enter an absolute path or ~/folder, then Browse.")
            })
            .inner;
        (response, browse)
    }

    fn project_folder_browser(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: theme::Palette,
        show_hidden: &mut bool,
    ) -> Option<String> {
        let count = self
            .entries
            .iter()
            .filter(|entry| *show_hidden || !entry.name.starts_with('.'))
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
                    if self.loading {
                        ui.spinner();
                        ui.label(palette.tag("Loading folders…"));
                    } else {
                        ui.label(palette.tag(format!("FOLDERS · {count}")));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let hidden = ui.checkbox(show_hidden, "Show hidden");
                        ui_probe::button(ui, "project-dialog-show-hidden", &hidden);
                    });
                });
                ui.add_space(6.);
                let row_pitch = layout::control_height(ui).max(25.) + 2.;
                let height = (count as f32 * row_pitch + 4.)
                    .clamp(64., (ctx.content_rect().height() * 0.3).clamp(64., 220.));
                let query = format!("folder:{}:{}", self.token, *show_hidden);
                let listing = ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), height),
                    Layout::top_down(Align::Min),
                    |ui| {
                        file_browser::rows(
                            ui,
                            "project-folder-browser",
                            &self.entries,
                            *show_hidden,
                            &mut self.selection,
                            &query,
                            (0, false),
                            None,
                            !self.loading,
                        )
                    },
                );
                ui_probe::button(ui, "project-dialog-folders", &listing.response);
                if let Some(action) = listing.inner {
                    browse = Some(match action {
                        file_browser::Choice::Parent => project_parent(&self.path),
                        file_browser::Choice::Select(entry) | file_browser::Choice::Open(entry) => {
                            entry.path
                        }
                    });
                }
                if count == 0 && !self.loading && self.error.is_none() {
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
        browse
    }
}

fn project_parent(path: &str) -> String {
    std::path::Path::new(path)
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .map_or_else(|| "/".into(), |path| path.to_string_lossy().into())
}
