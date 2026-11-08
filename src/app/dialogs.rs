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
            let modal = egui::Modal::new(egui::Id::new("unsaved"))
                .frame(layout::dialog_frame(&ctx.style(), palette))
                .show(ctx, |ui| {
                    ui.set_width(400_f32.min((ctx.content_rect().width() - 64.).max(1.)));
                    ui.heading("Keep your changes?");
                    ui.add_space(8.0);
                    ui.label(
                    "You have unsaved edits. Save them before continuing, or discard them here.",
                );
                    ui.add_space(16.0);
                    ui.horizontal(|ui| {
                        let keep = palette.primary(ui, "Keep editing");
                        ui_probe::button(ui, "guard-keep-editing", &keep);
                        if keep.clicked() {
                            self.pending = None;
                            self.focus_editor(ctx);
                        }
                        let discard =
                            ui.button(RichText::new("Discard changes").color(palette.error));
                        ui_probe::button(ui, "guard-discard", &discard);
                        if discard.clicked() {
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
        let response = egui::Modal::new(egui::Id::new("dialog"))
            .frame(layout::dialog_frame(&ctx.style(), palette))
            .show(ctx, |ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
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
                let width: f32 = if matches!(
                    self.dialog,
                    Dialog::Sessions | Dialog::AgentSessions | Dialog::Tunnels
                ) {
                    680.
                } else if self.dialog == Dialog::Folder {
                    720.
                } else if self.dialog == Dialog::Connect {
                    500.
                } else {
                    540.
                };
                ui.set_width(width.min((ctx.content_rect().width() - 64.).max(1.)));
                if self.dialog == Dialog::Folder {
                    self.folder_dialog(ui, ctx);
                } else if self.dialog == Dialog::Tunnels {
                    // Like the project picker, the tunnel manager owns its
                    // scrolling body and keeps its actions visible.
                    self.tunnels_dialog(ui, ctx);
                } else {
                    egui::ScrollArea::vertical()
                        .id_salt(("dialog-body", std::mem::discriminant(&self.dialog)))
                        .auto_shrink([false, true])
                        // The modal Area can retain a short command palette's
                        // height. Give forms their viewport budget before measuring
                        // content, then shrink short dialogs to their actual height.
                        .min_scrolled_height((ctx.content_rect().height() - 80.).max(1.))
                        .max_height((ctx.content_rect().height() - 80.).max(1.))
                        .show(ui, |ui| {
                            if self.dialog == Dialog::Extensions {
                                self.extensions_dialog(ui, ctx);
                            } else if self.dialog == Dialog::Explorer {
                                self.explorer_dialog(ui, ctx);
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
                            } else {
                                self.path_dialog(ui, ctx);
                            }
                        });
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
        layout::heading(
            ui,
            "Connect via SSH",
            "Rust SSH is built in. OpenSSH is available for advanced configurations.",
            palette,
        );
        if !self.profiles.is_empty() || !self.aliases.is_empty() {
            egui::ComboBox::from_id_salt("profiles")
                .selected_text("Load a saved connection or SSH alias")
                .width(ui.available_width())
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
        let compact = ui.available_width() < 460.;
        layout::card(ui, palette, |ui| {
            let field_width =
                (ui.available_width() - if compact { 8. } else { 154. }).clamp(1., 340.);
            egui::Grid::new("connection-fields")
                .num_columns(if compact { 1 } else { 2 })
                .spacing([18.0, 10.0])
                .show(ui, |ui| {
                    for (label, field, hint) in [
                        (
                            "Connection name",
                            &mut self.profile.name,
                            "e.g. Work cluster or Home server (optional)",
                        ),
                        (
                            "Host / alias",
                            &mut self.profile.host,
                            "e.g. workstation or user@host.example.org",
                        ),
                        (
                            "Remote folder",
                            &mut self.profile.root,
                            "e.g. ~/projects/my-project",
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
                            "Optional; e.g. ~/.ssh/id_ed25519",
                        ),
                        (
                            "Jump host(s)",
                            &mut self.profile.jump,
                            "Optional; e.g. user@login-host",
                        ),
                        (
                            "SSH config file",
                            &mut self.profile.config,
                            "~/.ssh/config (default)",
                        ),
                        (
                            "Remote helper",
                            &mut self.profile.remote_helper,
                            "Auto, or /path/to/rode-sessiond",
                        ),
                    ] {
                        layout::field_label(ui, label, compact);
                        let id = egui::Id::new(("connection-field", label));
                        let initial_focus = if self.profile_name_focus {
                            "Connection name"
                        } else {
                            "Host / alias"
                        };
                        if self.dialog_focus && label == initial_focus {
                            ui.memory_mut(|memory| memory.request_focus(id));
                            if self.profile_name_focus {
                                let mut state =
                                    egui::TextEdit::load_state(ctx, id).unwrap_or_default();
                                state
                                    .cursor
                                    .set_char_range(Some(egui::text::CCursorRange::two(
                                        egui::text::CCursor::new(0),
                                        egui::text::CCursor::new(field.chars().count()),
                                    )));
                                state.store(ctx, id);
                            }
                            self.dialog_focus = false;
                        }
                        let response = ui.add(
                            egui::TextEdit::singleline(field)
                                // Loading saved profiles can insert a dropdown above
                                // these fields while the user is already typing.
                                .id(id)
                                .hint_text(hint)
                                .desired_width(field_width),
                        );
                        ui_probe::button(ui, &format!("ssh-field-{label}"), &response);
                        ui.end_row();
                    }
                });
        });
        ui.small(
            "The connection name labels your saved entry. Leave it blank to use the SSH address.",
        );
        ui.small(
            "Muted text in empty fields shows examples or defaults; it is not an entered value.",
        );
        if let Some(route) = &self.profile.tunnel {
            ui.label(format!(
                "Tunnel route: relay loopback port {} · verified as {}",
                route.port, route.host_key_alias
            ));
            ui.small("Host / alias and identity settings still describe the compute host. Jump host(s) reach the relay.");
            if ui.button("Remove tunnel route").clicked() {
                self.profile.tunnel = None;
            }
        }
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.label("SSH client");
            egui::ComboBox::from_id_salt("ssh-backend")
                .selected_text(match self.profile.backend {
                    ssh::Backend::Russh => "Rust (default)",
                    ssh::Backend::OpenSsh => "OpenSSH",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut self.profile.backend,
                        ssh::Backend::Russh,
                        "Rust (default)",
                    );
                    ui.selectable_value(
                        &mut self.profile.backend,
                        ssh::Backend::OpenSsh,
                        "OpenSSH",
                    );
                });
        });
        ui.checkbox(
            &mut self.profile.x11_forwarding,
            "Forward X11 when available",
        )
        .on_hover_text("Launch remote X applications on your desktop. Requires a local X server and remote SSH X11 support. Trust and timeout follow your SSH config; sandbox restrictions still apply. Reconnect and open a new terminal after changing this setting.");
        ui.push_id("ssh-sandbox-presets", |ui| {
            self.sandbox_presets
                .ui(ui, &mut self.profile.sandbox, palette);
        });
        workbench::sandbox_backend_ui(ui, &mut self.profile.sandbox);
        if self.profile.sandbox.enabled {
            ui.collapsing("Sandbox access rules", |ui| {
                egui::ScrollArea::vertical()
                    .max_height(300.)
                    .show(ui, |ui| {
                        ui.push_id("ssh-sandbox-rules", |ui| {
                            workbench::sandbox_rules_ui(ui, &mut self.profile.sandbox, palette)
                        });
                    });
            });
        }
        ui.label(RichText::new("Authentication and host-key prompts open in the terminal. RODE uses a compatible installed helper or deploys a matching native helper.").small().color(palette.muted));
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
            let save = ui.add_enabled(
                !self.profile_saving,
                egui::Button::new(if self.profile_saving {
                    "Saving…"
                } else {
                    "Save profile"
                }),
            );
            ui_probe::button(ui, "ssh-save-profile", &save);
            if save.clicked() {
                if let Err(error) = self.profile.validate() {
                    self.notify(error.to_string(), true);
                } else {
                    let profile = self.profile.clone();
                    let previous = self.profile_baseline.clone();
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

    /// Render shortcut/navigation configuration using the same validated bindings as input routing.
    pub(super) fn help_dialog(&mut self, ui: &mut egui::Ui) {
        let palette = self.themes.active().palette;
        ui.label(palette.tag("MAKE YOURSELF AT HOME"));
        layout::heading(
            ui,
            "Keyboard shortcuts",
            "Customize navigation and review editor shortcuts",
            palette,
        );
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
        // Project browsing owns the foreground until confirmation. Retain
        // background notices without painting them over its fixed actions.
        let order = if self.dialog == Dialog::Folder && self.dialog_viewport == ctx.viewport_id() {
            egui::Order::Middle
        } else {
            egui::Order::Tooltip
        };
        egui::Area::new(egui::Id::new("notification"))
            .anchor(egui::Align2::RIGHT_BOTTOM, [-20.0, -42.0])
            .order(order)
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
                            let dismiss = icons::small(ui, Icon::Close, "Dismiss notification");
                            ui_probe::button(ui, "notice-dismiss", &dismiss);
                            if dismiss.clicked() {
                                self.notice = None;
                            }
                        });
                    });
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forms_expand_after_a_short_command_palette_and_keep_actions_visible() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        let mut render = |dialog| {
            app.set_dialog(dialog);
            let mut visible = Vec::new();
            for _ in 0..4 {
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(1380., 900.),
                        )),
                        ..Default::default()
                    },
                    |ctx| app.dialogs(ctx),
                );
                if !output.shapes.is_empty() {
                    for shape in &output.shapes {
                        if let egui::Shape::Text(text) = &shape.shape
                            && ["Host / alias", "Remote folder", "GPUs (total)"]
                                .contains(&text.galley.text())
                        {
                            assert_eq!(
                                text.galley.rows.len(),
                                1,
                                "Form label broke into fragments: {}",
                                text.galley.text()
                            );
                        }
                    }
                }
                visible = output
                    .shapes
                    .iter()
                    .filter_map(|shape| {
                        if let egui::Shape::Text(text) = &shape.shape {
                            let center = text.pos + text.galley.size() / 2.;
                            if shape.clip_rect.contains(center) {
                                return Some(text.galley.text().to_owned());
                            }
                        }
                        None
                    })
                    .collect();
            }
            visible
        };
        for (dialog, field, action) in [
            (Dialog::Connect, "Host / alias", "Connect"),
            (Dialog::Slurm, "GPUs (total)", "Allocate in new terminal"),
            (Dialog::Extensions, "Extensions", "Close"),
        ] {
            render(Dialog::Commands);
            let visible = render(dialog);
            assert!(
                visible.iter().any(|label| label == field),
                "Missing {field}: {visible:?}"
            );
            assert!(
                visible.iter().any(|label| label == action),
                "Missing {action}: {visible:?}"
            );
        }
    }
}
