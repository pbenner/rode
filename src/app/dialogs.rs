//! Guarded native dialogs and transient notices.
//!
//! Dialogs run on their owning viewport and reuse the common save, project and
//! shutdown actions. Rendering a dialog does not bypass those lifecycle guards.

use super::*;

impl EditorApp {
    /// Render the exclusive current dialog and common dirty-buffer confirmation on its owning viewport.
    pub(super) fn dialogs(&mut self, ctx: &egui::Context) {
        if ctx.viewport_id() != self.dialog_viewport {
            return;
        }
        let palette = self.themes.active().palette;
        if let Some(action) = self.pending.clone() {
            let modal = egui::Modal::new(egui::Id::new("unsaved")).show(ctx, |ui| {
                ui.set_width(400.0);
                ui.heading("Keep your changes?");
                ui.add_space(8.0);
                ui.label(
                    "You have unsaved edits. Save them before continuing, or discard them here.",
                );
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    if palette.primary(ui, "Keep editing").clicked() {
                        self.pending = None;
                        self.focus_editor(ctx);
                    }
                    if ui
                        .button(RichText::new("Discard changes").color(palette.error))
                        .clicked()
                    {
                        self.pending = None;
                        self.perform(action, ctx);
                    }
                });
            });
            if modal.should_close() {
                self.pending = None;
                self.focus_editor(ctx);
            }
            return;
        }
        if self.dialog == Dialog::None {
            self.dialog_events.clear();
            return;
        }
        if !self.dialog_events.is_empty() {
            let mut events = std::mem::take(&mut self.dialog_events);
            ctx.input_mut(|input| {
                events.append(&mut input.events);
                input.events = events;
            });
        }
        let encrypted_dialog = self.dialog == Dialog::Encrypted;
        let file_dialog = file_browser::is_file_dialog(&self.dialog);
        let response = egui::Modal::new(egui::Id::new("dialog")).show(ctx, |ui| {
            let sizing = ui.is_sizing_pass();
            let focus = self.dialog_focus;
            if sizing {
                // Area's first pass is invisible and disables widgets. Keep
                // keyboard input for its first interactive pass instead of
                // consuming Enter against an empty command/path field.
                self.dialog_events = ctx.input_mut(|input| {
                    let (keyboard, other) = std::mem::take(&mut input.events)
                        .into_iter()
                        .partition(|event| {
                            matches!(
                                event,
                                egui::Event::Text(_)
                                    | egui::Event::Key { .. }
                                    | egui::Event::Paste(_)
                                    | egui::Event::Copy
                                    | egui::Event::Cut
                                    | egui::Event::Ime(_)
                            )
                        });
                    input.events = other;
                    keyboard
                });
                ctx.request_repaint();
            }
            ui.set_width(
                if matches!(self.dialog, Dialog::Sessions | Dialog::AgentSessions) {
                    680_f32.min(ctx.content_rect().width() - 50.)
                } else if self.dialog == Dialog::Connect {
                    500.
                } else {
                    540.
                },
            );
            if self.dialog == Dialog::Extensions {
                self.extensions_dialog(ui, ctx);
            } else if self.dialog == Dialog::Encrypted {
                self.encrypted_ui(ui, ctx);
            } else if matches!(self.dialog, Dialog::MarpExport(..)) {
                self.marp_export_dialog(ui, ctx);
            } else if self.dialog == Dialog::Connect {
                self.connect_dialog(ui, ctx);
            } else if self.dialog == Dialog::Shortcuts {
                self.help_dialog(ui);
            } else if self.dialog == Dialog::Commands {
                self.command_dialog(ui, ctx);
            } else if self.dialog == Dialog::Sessions {
                self.sessions_dialog(ui, ctx);
            } else if self.dialog == Dialog::Slurm {
                self.slurm_dialog(ui, ctx);
            } else if self.dialog == Dialog::AgentSessions {
                self.background_agents_dialog(ui, ctx);
            } else if self.dialog == Dialog::TerminalExit {
                self.terminal_exit_dialog(ui, ctx);
            } else if self.dialog == Dialog::Folder {
                self.folder_dialog(ui, ctx);
            } else {
                self.path_dialog(ui, ctx);
            }
            if sizing {
                self.dialog_focus = focus;
            }
        });
        if response.should_close()
            && !(self.dialog == Dialog::TerminalExit && self.session_exit_pending())
            && !self.vaults.locking
        {
            self.dialog = Dialog::None;
        }
        if file_dialog && !file_browser::is_file_dialog(&self.dialog) {
            self.file_browser.reset();
        }
        if self.dialog == Dialog::None {
            if encrypted_dialog {
                self.cancel_encrypted_dialog();
            }
            // egui keeps a closed modal's input barrier for one pass. Clear it
            // promptly even when no background worker will request a repaint.
            ctx.request_repaint();
        }
    }

    /// Edit an SSH profile and start authentication only after the explicit guarded connection action.
    pub(super) fn connect_dialog(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        ui.label(palette.tag("REMOTE WORKSPACE"));
        ui.heading("Connect via SSH");
        ui.label(
            RichText::new("OpenSSH under the hood. Your existing configuration works here.")
                .color(palette.muted),
        );
        ui.add_space(12.0);
        if !self.profiles.is_empty() || !self.aliases.is_empty() {
            egui::ComboBox::from_id_salt("profiles")
                .selected_text("Load a saved connection or SSH alias")
                .width(450.0)
                .show_ui(ui, |ui| {
                    for profile in &self.profiles {
                        if ui.selectable_label(false, profile.label()).clicked() {
                            self.profile = profile.clone();
                            self.profile_baseline = Some(profile.clone());
                            self.profile_form += 1;
                        }
                    }
                    if !self.aliases.is_empty() {
                        ui.separator();
                    }
                    for alias in &self.aliases {
                        if ui
                            .selectable_label(false, format!("SSH config / {alias}"))
                            .clicked()
                        {
                            self.profile = Profile {
                                host: alias.clone(),
                                ..Default::default()
                            };
                            self.profile_baseline = None;
                            self.profile_form += 1;
                        }
                    }
                });
            ui.add_space(8.0);
        }
        egui::Grid::new("connection-fields")
            .num_columns(2)
            .spacing([18.0, 10.0])
            .show(ui, |ui| {
                for (label, field, hint) in [
                    (
                        "Host / alias",
                        &mut self.profile.host,
                        "workstation or user@host.example.org",
                    ),
                    (
                        "Remote folder",
                        &mut self.profile.root,
                        "~/projects/my-project",
                    ),
                    ("Username", &mut self.profile.user, "From SSH config"),
                    (
                        "Port",
                        &mut self.profile.port,
                        "From SSH config (usually 22)",
                    ),
                    (
                        "Identity file",
                        &mut self.profile.identity,
                        "~/.ssh/id_ed25519 (optional)",
                    ),
                    (
                        "Jump host(s)",
                        &mut self.profile.jump,
                        "user@login-host (optional, comma-separated)",
                    ),
                    (
                        "SSH config file",
                        &mut self.profile.config,
                        "~/.ssh/config (default)",
                    ),
                    (
                        "Profile name",
                        &mut self.profile.name,
                        "Optional friendly name",
                    ),
                ] {
                    ui.label(label);
                    let response = ui.add(
                        egui::TextEdit::singleline(field)
                            // Loading saved profiles can insert a dropdown above
                            // these fields while the user is already typing.
                            .id(egui::Id::new(("connection-field", label)))
                            .hint_text(hint)
                            .desired_width(340.0),
                    );
                    if self.dialog_focus && label == "Host / alias" {
                        response.request_focus();
                        self.dialog_focus = false;
                    }
                    ui.end_row();
                }
            });
        ui.add_space(12.0);
        ui.checkbox(
            &mut self.profile.sandbox.enabled,
            "Sandbox the remote project when bwrap is available",
        );
        if self.profile.sandbox.enabled {
            ui.collapsing("Sandbox access rules", |ui| {
                egui::ScrollArea::vertical()
                    .max_height(300.)
                    .show(ui, |ui| {
                        workbench::sandbox_options_ui(ui, &mut self.profile.sandbox)
                    });
            });
        }
        ui.label(RichText::new("Authentication and host-key prompts open in the terminal. Remote file access uses Python 3's standard library.").small().color(palette.muted));
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if palette.primary(ui, "Connect").clicked() {
                match self.profile.validate() {
                    Ok(()) => {
                        self.dialog = Dialog::None;
                        self.guarded(Action::Remote(self.profile.clone()), ctx);
                    }
                    Err(error) => self.notify(error.to_string(), true),
                }
            }
            if ui
                .add_enabled(
                    !self.profile_saving,
                    egui::Button::new(if self.profile_saving {
                        "Saving…"
                    } else {
                        "Save profile"
                    }),
                )
                .clicked()
            {
                if let Err(error) = self.profile.validate() {
                    self.notify(error.to_string(), true);
                } else {
                    let profile = self.profile.clone();
                    let previous = self
                        .profile_baseline
                        .as_ref()
                        .filter(|p| p.label() == profile.label())
                        .cloned();
                    let form = self.profile_form;
                    self.profiles_loading = None;
                    let tx = self.tx.clone();
                    let context = ctx.clone();
                    self.profile_saving = true;
                    self.profile_writer = Some(std::thread::spawn(move || {
                        let result = ssh::save_profile(&profile, previous.as_ref())
                            .map_err(|e| format!("{e:#}"));
                        if let Err(error) = &result {
                            eprintln!("Connection profile was not saved: {error}");
                        }
                        let _ = tx.send(Event::ProfileSaved {
                            result,
                            submitted: Box::new(profile),
                            form,
                        });
                        context.request_repaint();
                    }));
                }
            }
            if ui.button("Cancel").clicked() {
                self.dialog = Dialog::None;
            }
        });
    }

    /// Delegate file/hex/save destinations to the shared host-aware file picker.
    pub(super) fn path_dialog(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.file_dialog(ui, ctx);
    }

    /// Render selected-host project discovery and requested sandbox options before project preparation.
    pub(super) fn folder_dialog(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
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
        ui.label(palette.tag(if self.folder_remote {
            format!("SSH · {host}")
        } else {
            "THIS MACHINE".into()
        }));
        ui.heading(if self.folder_remote {
            "Open remote project folder"
        } else {
            "Open project folder"
        });
        ui.label(
            RichText::new(
                "Browse folders, then open the project. Absolute paths and ~/ are supported.",
            )
            .small()
            .color(palette.muted),
        );
        let input_id = egui::Id::new("project-dialog-path");
        if self.dialog_focus {
            ui.memory_mut(|memory| memory.request_focus(input_id));
            self.dialog_focus = false;
        }
        let response = ui.add(
            egui::TextEdit::singleline(&mut self.dialog_input)
                .id(input_id)
                .desired_width(f32::INFINITY)
                .hint_text("Project folder"),
        );
        let enter = (response.has_focus() || response.lost_focus())
            && ui.input(|i| i.key_pressed(Key::Enter));
        let mut browse = None;
        ui.horizontal(|ui| {
            if ui.button("Up").clicked() {
                browse = Some(
                    std::path::Path::new(&self.dialog_input)
                        .parent()
                        .filter(|p| !p.as_os_str().is_empty())
                        .map(|p| p.to_string_lossy().into())
                        .unwrap_or_else(|| "/".into()),
                );
            }
            if ui.button("Browse").clicked() || enter {
                browse = Some(self.dialog_input.clone());
            }
            if self.folder_loading {
                ui.spinner();
                ui.label(palette.tag("Loading folders…"));
            }
        });
        if let Some(error) = &self.folder_error {
            ui.label(RichText::new(error).color(palette.error));
        }
        let query = format!("folder:{}:{}", self.folder_token, self.show_hidden);
        if let Some(action) = file_browser::rows(
            ui,
            "project-folder-browser",
            &self.folder_entries,
            self.show_hidden,
            &mut self.navigation.list,
            &query,
            (0, false),
            None,
            !self.folder_loading,
        ) {
            browse = Some(match action {
                file_browser::Choice::Parent => std::path::Path::new(&self.dialog_input)
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .map_or_else(|| "/".into(), |p| p.to_string_lossy().into()),
                file_browser::Choice::Select(entry) | file_browser::Choice::Open(entry) => {
                    entry.path
                }
            });
        }
        if self
            .folder_entries
            .iter()
            .all(|e| !self.show_hidden && e.name.starts_with('.'))
            && !self.folder_loading
        {
            ui.label(palette.tag("No visible subfolders. You can open this folder."));
        }
        if let Some(path) = browse {
            self.browse_folder(path, ctx);
        }
        ui.separator();
        ui.checkbox(
            &mut self.folder_sandbox.enabled,
            "Sandbox project when bwrap is available",
        );
        if self.folder_sandbox.enabled {
            ui.label(RichText::new("If sandboxing is unavailable on this host, the project opens in clearly labelled normal mode.").small().color(palette.muted));
            ui.collapsing("Sandbox access rules", |ui| {
                egui::ScrollArea::vertical()
                    .max_height(260.)
                    .show(ui, |ui| {
                        workbench::sandbox_options_ui(ui, &mut self.folder_sandbox)
                    });
            });
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if palette.primary(ui, "Open project  ·  Ctrl+Enter").clicked() || open_by_key {
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
            if ui.button("Cancel").clicked() {
                self.dialog = Dialog::None;
            }
        });
    }

    /// Render shortcut/navigation configuration using the same validated bindings as input routing.
    pub(super) fn help_dialog(&mut self, ui: &mut egui::Ui) {
        let palette = self.themes.active().palette;
        ui.label(palette.tag("MAKE YOURSELF AT HOME"));
        ui.heading("Keyboard shortcuts");
        let focus = std::mem::take(&mut self.dialog_focus);
        self.navigation.bindings.ui(ui, focus);
        ui.add_space(8.0);
        ui.collapsing("Standard editor shortcuts", |ui| {
            egui::ScrollArea::vertical()
                .id_salt("editor-shortcut-reference")
                .max_height(180.)
                .show(ui, |ui| {
                    egui::Grid::new("shortcut-list")
                        .spacing([32.0, 10.0])
                        .show(ui, |ui| {
                            for (key, description) in [
                                ("Ctrl+Shift+P", "Command palette"),
                                ("Ctrl+P", "Open file by path"),
                                ("Ctrl+Shift+O", "Open project folder"),
                                ("Ctrl+Shift+R", "Toggle Markdown preview"),
                                ("Ctrl+N", "New file"),
                                ("Ctrl+S", "Save file"),
                                ("Ctrl+Shift+S", "Save to a different path"),
                                ("Ctrl+W", "Close tab"),
                                ("Ctrl+PageUp / Down", "Previous / next tab"),
                                ("Ctrl+Tab", "Next tab (Shift for previous)"),
                                ("Ctrl+Shift+A", "Agent chat"),
                                ("Ctrl+Shift+X", "Manage extensions"),
                                ("F5 / Shift+F5", "Debug: start/continue / stop/detach"),
                                ("F6 / F9", "Debug: pause / toggle breakpoint"),
                                ("F10 / F11 / Shift+F11", "Debug: step over / into / out"),
                                ("Ctrl+B", "Toggle explorer"),
                                (
                                    "Ctrl+~ / Ctrl+`",
                                    "Toggle tools sidebar (remember selected tab)",
                                ),
                                ("Ctrl+Shift+T", "Toggle / focus terminal"),
                                ("Ctrl+F", "Find and replace"),
                                ("Ctrl+Plus / Minus", "Editor font size"),
                                ("Escape", "Close dialog / find"),
                            ] {
                                ui.label(RichText::new(key).monospace().color(palette.accent));
                                ui.label(description);
                                ui.end_row();
                            }
                        });
                });
        });
        ui.add_space(12.0);
        ui.label(RichText::new("On macOS use Command for editor shortcuts. Terminal keys go to the shell when it has focus. Use the Copy button to copy terminal output.").small().color(palette.muted));
        if ui.button("Done").clicked() {
            self.dialog = Dialog::None;
        }
    }

    /// Render the current timestamped notification with bounded screen placement.
    pub(super) fn notice_ui(&mut self, ctx: &egui::Context) {
        if egui::Popup::is_any_open(ctx) {
            return;
        }
        let palette = self.themes.active().palette;
        let Some((message, error, time)) = &self.notice else {
            return;
        };
        if !error && time.elapsed() > Duration::from_secs(8) {
            self.notice = None;
            return;
        }
        let text = message.clone();
        let error = *error;
        egui::Area::new(egui::Id::new("notification"))
            .anchor(egui::Align2::RIGHT_BOTTOM, [-20.0, -42.0])
            .order(egui::Order::Tooltip)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(palette.raised)
                    .stroke(Stroke::new(
                        1.0_f32,
                        if error {
                            palette.error
                        } else {
                            palette.success
                        },
                    ))
                    .corner_radius(6)
                    .inner_margin(egui::Margin::same(14))
                    .show(ui, |ui| {
                        ui.set_max_width(520.0);
                        ui.horizontal_top(|ui| {
                            ui.label(RichText::new(if error { "!" } else { "✓" }).color(
                                if error {
                                    palette.error
                                } else {
                                    palette.success
                                },
                            ));
                            ui.add(egui::Label::new(text).wrap());
                            if icons::small(ui, Icon::Close, "Dismiss notification").clicked() {
                                self.notice = None;
                            }
                        });
                    });
            });
    }
}
