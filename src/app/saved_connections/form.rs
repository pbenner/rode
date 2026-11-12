//! Connection form rendering emits intents without changing the workspace.
use super::*;

impl State {
    /// Edit an SSH profile and start authentication only after the explicit guarded connection action.
    pub(in crate::app) fn form(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: theme::Palette,
        presets: &mut sandbox_presets::State,
        focus: &mut bool,
    ) -> Option<FormIntent> {
        let mut intent = None;
        ui.label(palette.tag("REMOTE WORKSPACE"));
        layout::heading(
            ui,
            "Connect via SSH",
            "Rust SSH is built in. OpenSSH is available for advanced configurations.",
            palette,
        );
        let mut selected = None;
        if !self.profiles.is_empty() || !self.aliases.is_empty() {
            egui::ComboBox::from_id_salt("profiles")
                .selected_text("Load a saved connection or SSH alias")
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    for profile in &self.profiles {
                        if ui.selectable_label(false, profile.label()).clicked() {
                            selected = Some((profile.clone(), true));
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
                            selected = Some((
                                Profile {
                                    host: alias.clone(),
                                    ..Default::default()
                                },
                                false,
                            ));
                        }
                    }
                });
            ui.add_space(8.0);
        }
        if let Some((profile, saved)) = selected {
            self.select(profile, saved);
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
                            &mut self.draft.name,
                            "e.g. Work cluster or Home server (optional)",
                        ),
                        (
                            "Host / alias",
                            &mut self.draft.host,
                            "e.g. workstation or user@host.example.org",
                        ),
                        (
                            "Remote folder",
                            &mut self.draft.root,
                            "e.g. ~/projects/my-project",
                        ),
                        ("Username", &mut self.draft.user, "From SSH config"),
                        ("Port", &mut self.draft.port, "From SSH config (usually 22)"),
                        (
                            "Identity file",
                            &mut self.draft.identity,
                            "Optional; e.g. ~/.ssh/id_ed25519",
                        ),
                        (
                            "Jump host(s)",
                            &mut self.draft.jump,
                            "Optional; e.g. user@login-host",
                        ),
                        (
                            "SSH config file",
                            &mut self.draft.config,
                            "~/.ssh/config (default)",
                        ),
                        (
                            "Remote helper",
                            &mut self.draft.remote_helper,
                            "Auto, or /path/to/rode-sessiond",
                        ),
                    ] {
                        layout::field_label(ui, label, compact);
                        let id = egui::Id::new(("connection-field", label));
                        let initial_focus = if self.name_focus {
                            "Connection name"
                        } else {
                            "Host / alias"
                        };
                        if *focus && label == initial_focus {
                            ui.memory_mut(|memory| memory.request_focus(id));
                            if self.name_focus {
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
                            *focus = false;
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
        if let Some(route) = &self.draft.tunnel {
            ui.label(format!(
                "Tunnel route: relay loopback port {} · verified as {}",
                route.port, route.host_key_alias
            ));
            ui.small("Host / alias and identity settings still describe the compute host. Jump host(s) reach the relay.");
            if ui.button("Remove tunnel route").clicked() {
                self.draft.tunnel = None;
            }
        }
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.label("SSH client");
            egui::ComboBox::from_id_salt("ssh-backend")
                .selected_text(match self.draft.backend {
                    ssh::Backend::Russh => "Rust (default)",
                    ssh::Backend::OpenSsh => "OpenSSH",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut self.draft.backend,
                        ssh::Backend::Russh,
                        "Rust (default)",
                    );
                    ui.selectable_value(&mut self.draft.backend, ssh::Backend::OpenSsh, "OpenSSH");
                });
        });
        ui.checkbox(
            &mut self.draft.x11_forwarding,
            "Forward X11 when available",
        )
        .on_hover_text("Launch remote X applications on your desktop. Requires a local X server and remote SSH X11 support. Sandbox restrictions still apply. Reconnect and open a new terminal after changing this setting.");
        if self.draft.x11_forwarding {
            ui.horizontal_wrapped(|ui| {
                ui.label("X11 trust");
                egui::ComboBox::from_id_salt("ssh-x11-trust")
                    .selected_text(self.draft.x11_trust.label())
                    .show_ui(ui, |ui| {
                        for trust in [
                            ssh::X11Trust::Config,
                            ssh::X11Trust::Untrusted,
                            ssh::X11Trust::Trusted,
                        ] {
                            ui.selectable_value(&mut self.draft.x11_trust, trust, trust.label());
                        }
                    });
            });
            ui.label(RichText::new(match self.draft.x11_trust {
                ssh::X11Trust::Config => "Uses SSH configuration. Rust defaults to untrusted; OpenSSH defaults can differ by system.",
                ssh::X11Trust::Untrusted => "Restricted desktop access. Requires the X server's SECURITY extension.",
                ssh::X11Trust::Trusted => "Remote X applications can access other windows and desktop input.",
            }).small().color(palette.muted));
        }
        ui.push_id("ssh-sandbox-presets", |ui| {
            presets.ui(ui, &mut self.draft.sandbox, palette);
        });
        workbench::sandbox_backend_ui(ui, &mut self.draft.sandbox);
        if self.draft.sandbox.enabled {
            ui.collapsing("Sandbox access rules", |ui| {
                egui::ScrollArea::vertical()
                    .max_height(300.)
                    .show(ui, |ui| {
                        ui.push_id("ssh-sandbox-rules", |ui| {
                            workbench::sandbox_rules_ui(ui, &mut self.draft.sandbox, palette)
                        });
                    });
            });
        }
        ui.label(RichText::new("Authentication and host-key prompts open in the terminal. RODE uses a compatible installed helper or deploys a matching native helper.").small().color(palette.muted));
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if palette.primary(ui, "Connect").clicked() {
                match self.draft.validate() {
                    Ok(()) => {
                        intent = Some(FormIntent::Connect(Box::new(self.draft.clone())));
                    }
                    Err(error) => intent = Some(FormIntent::Error(error.to_string())),
                }
            }
            let save = ui.add_enabled(
                !self.saving,
                egui::Button::new(if self.saving {
                    "Saving…"
                } else {
                    "Save profile"
                }),
            );
            ui_probe::button(ui, "ssh-save-profile", &save);
            if save.clicked() {
                intent = Some(match self.draft.validate() {
                    Ok(()) => FormIntent::Save,
                    Err(error) => FormIntent::Error(error.to_string()),
                });
            }
            if ui.button("Cancel").clicked() {
                intent = Some(FormIntent::Cancel);
            }
        });
        intent
    }
}
