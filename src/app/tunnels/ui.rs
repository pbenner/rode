//! Tunnel-manager presentation. Navigation and primary actions stay outside the
//! scrolling body; form widget identities survive tab switches and resizing.
use super::*;
use crate::theme::Palette;

impl EditorApp {
    /// Present only observed connections; offline/failed discovery is unknown,
    /// rather than a stale positive count or an invented zero.
    pub(in crate::app) fn tunnel_status_button(
        &self,
        ui: &mut egui::Ui,
        palette: Palette,
    ) -> egui::Response {
        let host = self
            .workspace
            .as_ref()
            .and_then(|w| w.remote.as_ref())
            .map_or_else(
                || "this computer".to_owned(),
                |remote| format!("SSH host {}", remote.profile.endpoint()),
            );
        let unavailable = self
            .tunnel_status_blocked()
            .or(match &self.tunnels.observation {
                Some(Err(error)) => Some(error.as_str()),
                None => Some("Checking tunnel status…"),
                Some(Ok(())) => None,
            });
        let (label, color, details) = if let Some(reason) = unavailable {
            ("Tunnels (?)".to_owned(), palette.muted, reason.to_owned())
        } else {
            let mut connected = 0;
            let mut connecting = 0;
            let mut reconnecting = 0;
            let mut attention = 0;
            for info in &self.tunnels.available {
                match info.status {
                    Status::Connected => connected += 1,
                    Status::Connecting => connecting += 1,
                    Status::Reconnecting => reconnecting += 1,
                    Status::HostKeyRequired | Status::Attention => attention += 1,
                }
            }
            let total = self.tunnels.available.len();
            let label = if total == 0 {
                "Tunnels".to_owned()
            } else if connected == total {
                format!("Tunnels ({connected})")
            } else {
                format!("Tunnels ({connected}/{total})")
            };
            let color = if attention > 0 {
                palette.error
            } else if connecting + reconnecting > 0 {
                palette.accent
            } else if connected > 0 {
                palette.success
            } else {
                palette.text
            };
            (
                label,
                color,
                format!(
                    "{connected} connected · {connecting} connecting · {reconnecting} reconnecting · {attention} need attention\nCounts show connected / total retained tunnels."
                ),
            )
        };
        ui.button(RichText::new(label).color(color))
            .on_hover_text(format!("Manage SSH tunnels on {host}.\n{details}"))
    }

    pub(in crate::app) fn tunnels_dialog(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        let top = ui.cursor().top();
        layout::heading(
            ui,
            "SSH tunnels",
            "Keep access to this compute host through a relay.",
            palette,
        );
        let remote = self.workspace.as_ref().and_then(|w| w.remote.as_ref());
        let host = remote.map_or_else(
            || "This computer · local service".to_owned(),
            |remote| format!("Compute host · {}", remote.profile.endpoint()),
        );
        ui.add(egui::Label::new(RichText::new(&host).strong()).truncate())
            .on_hover_text(&host);
        layout::subtitle(ui, "Host controls · outside the project sandbox", palette);
        if remote.is_some() && self.workspace.as_ref().is_some_and(|w| !w.connected) {
            layout::empty(
                ui,
                "SSH connection unavailable",
                "Reconnect SSH to manage this host's tunnels.",
                palette,
            );
            if ui.button("Close").clicked() {
                self.dialog = Dialog::None;
            }
            return;
        }
        ui.add_space(8.);
        ui.horizontal_wrapped(|ui| {
            let list = ui.selectable_value(
                &mut self.tunnels.page,
                Page::Tunnels,
                format!("Tunnels ({})", self.tunnels.available.len()),
            );
            ui_probe::button(ui, "tunnel-tab-list", &list);
            let saved = ui.selectable_value(
                &mut self.tunnels.page,
                Page::Saved,
                format!("Saved ({})", self.tunnels.saved.entries.len()),
            );
            ui_probe::button(ui, "tunnel-tab-saved", &saved);
            let new = ui.selectable_value(&mut self.tunnels.page, Page::New, "New tunnel");
            ui_probe::button(ui, "tunnel-tab-new", &new);
        });
        ui.separator();
        ui.add_space(4.);

        let page = self.tunnels.page;
        let header_height = ui.cursor().top() - top;
        // Reserve room for wrapped validation/lifetime copy and the action row,
        // independent of an Area retaining its previous dialog's dimensions.
        let footer_height = if page == Page::New { 144. } else { 90. };
        let body_height =
            (ctx.content_rect().height() - 80. - header_height - footer_height).max(40.);
        egui::ScrollArea::vertical()
            .id_salt((
                "tunnel-page",
                page,
                if page == Page::New {
                    self.tunnels.draft_epoch
                } else {
                    0
                },
            ))
            .auto_shrink([false, true])
            .min_scrolled_height(body_height)
            .max_height(body_height)
            .show(ui, |ui| {
                if !self.tunnels.error.is_empty() {
                    layout::card(ui, palette, |ui| {
                        tunnel_error(ui, ctx, &self.tunnels.error, palette.error);
                    });
                    ui.add_space(8.);
                }
                match page {
                    Page::Tunnels => self.tunnel_list_ui(ui, ctx, palette),
                    Page::Saved => self.saved_tunnels_ui(ui, palette),
                    Page::New => self.tunnel_form_ui(ui, palette),
                }
            });
        ui.add_space(8.);
        ui.separator();
        self.tunnel_footer(ui, ctx, page, palette);
    }

    fn saved_tunnels_ui(&mut self, ui: &mut egui::Ui, palette: Palette) {
        if let Some(error) = &self.tunnels.saved.error {
            ui.colored_label(palette.error, error);
            return;
        }
        if self.tunnels.saved.entries.is_empty() {
            layout::empty(
                ui,
                "No saved tunnel configurations",
                "Save a setup from New tunnel or from a running tunnel to reuse it after a restart.",
                palette,
            );
            return;
        }
        layout::subtitle(
            ui,
            "Saved on this computer. Load a setup for the selected host, review it, then start the tunnel.",
            palette,
        );
        ui.add_space(8.);
        let host = self.tunnel_saved_host();
        let mut entries: Vec<_> = self.tunnels.saved.entries.clone().into_iter().collect();
        entries.sort_by(|(_, a), (_, b)| {
            (a.host != host, &a.spec.name).cmp(&(b.host != host, &b.spec.name))
        });
        for (id, saved) in entries {
            ui.push_id(("saved-tunnel", &id), |ui| {
                layout::card(ui, palette, |ui| {
                    ui.add(egui::Label::new(RichText::new(&saved.spec.name).strong()).wrap());
                    ui.add(egui::Label::new(saved.host.label()).wrap());
                    layout::subtitle(ui, &format!("Relay {} · listener {} · {}", endpoint(&saved.spec.relay_host, saved.spec.relay_port), saved.spec.listener(), saved.spec.listen_on.label()), palette);
                    ui.horizontal_wrapped(|ui| {
                        let load = ui.add_enabled(saved.host == host, egui::Button::new("Load setup"))
                            .on_disabled_hover_text(format!("Open {} before loading this setup.", saved.host.label()));
                        ui_probe::button(ui, &format!("tunnel-saved-load:{id}"), &load);
                        if load.clicked() { self.load_tunnel_configuration(&id); }
                        if ui.button("Delete…").clicked() { self.tunnels.delete_saved = Some(id.clone()); }
                    });
                    if self.tunnels.delete_saved.as_deref() == Some(&id) {
                        ui.label("Delete this saved configuration? Running tunnels keep their current settings.");
                        ui.horizontal_wrapped(|ui| {
                            if ui.button(RichText::new("Delete configuration").color(palette.error)).clicked() {
                                self.delete_tunnel_configuration(&id);
                            }
                            if ui.button("Cancel").clicked() { self.tunnels.delete_saved = None; }
                        });
                    }
                });
            });
            ui.add_space(10.);
        }
    }

    fn tunnel_list_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, palette: Palette) {
        if self.tunnels.available.is_empty() {
            layout::empty(
                ui,
                if self.tunnels.pending.is_some() {
                    "Loading tunnels…"
                } else {
                    "No tunnels on this host"
                },
                "Create a tunnel to reach this compute node through a relay after disconnecting.",
                palette,
            );
            ui.add_space(8.);
            if ui.button("New tunnel").clicked() {
                self.tunnels.page = Page::New;
            }
            return;
        }
        ui.collapsing("Return connection settings", |ui| {
            layout::subtitle(ui, "Optional desktop SSH alias or jump chain. Leave blank to use each tunnel’s relay address.", palette);
            ui.label("Desktop relay alias / jump chain");
            ui.add(egui::TextEdit::singleline(&mut self.tunnels.relay_jump)
                .id_salt("tunnel-desktop-relay").desired_width(f32::INFINITY).hint_text("From the tunnel’s relay settings"));
        });
        ui.add_space(8.);
        for info in self.tunnels.available.clone() {
            ui.push_id(&info.id, |ui| {
                layout::card(ui, palette, |ui| {
                    self.tunnel_card_ui(ui, ctx, &info, palette)
                });
            });
            ui.add_space(10.);
        }
    }

    fn tunnel_card_ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        info: &Info,
        palette: Palette,
    ) {
        let status_color = match info.status {
            Status::Connected => palette.success,
            Status::Attention => palette.error,
            Status::HostKeyRequired if !info.spec.relay_key.is_empty() => palette.error,
            _ => palette.accent,
        };
        ui.horizontal_wrapped(|ui| {
            ui.strong(&info.spec.name);
            ui.label(
                RichText::new(info.status.label())
                    .small()
                    .color(status_color)
                    .background_color(palette.raised),
            );
        });
        ui.add_space(8.);
        let compute_host = self.tunnel_compute_host(info);
        let details_width = (ui.available_width() - 100.).max(1.);
        egui::Grid::new("tunnel-endpoints")
            .num_columns(2)
            .min_col_width(0.)
            .max_col_width(details_width)
            .spacing([12., 6.])
            .show(ui, |ui| {
                ui.label(RichText::new("Tunnel").color(palette.muted));
                ui.add(
                    egui::Label::new(endpoint(&info.spec.relay_host, info.spec.listen_port)).wrap(),
                )
                .on_hover_text(format!(
                    "Requested listener: {} on {}.",
                    info.spec.listener(),
                    info.spec.relay_host
                ));
                ui.end_row();
                ui.label(RichText::new("Destination").color(palette.muted));
                ui.add(egui::Label::new(endpoint(&compute_host, info.spec.ssh_port)).wrap())
                    .on_hover_text(format!(
                        "Forwards to 127.0.0.1:{} on {compute_host}.",
                        info.spec.ssh_port
                    ));
                ui.end_row();
                ui.label(RichText::new("Listen on").color(palette.muted));
                ui.add(egui::Label::new(info.spec.listen_on.label()).wrap());
                ui.end_row();
            });
        layout::subtitle(
            ui,
            if info.spec.listen_on == ListenOn::Loopback {
                "Requests localhost access through the relay's SSH connection."
            } else {
                "Requests access from other devices. The relay's SSH policy and firewall determine reachability."
            },
            palette,
        );
        if !info.error.is_empty() {
            ui.add_space(8.);
            tunnel_error(ui, ctx, &info.error, status_color);
        }
        let busy = self.tunnels.pending.is_some();
        if info.status == Status::HostKeyRequired {
            ui.add_space(8.);
            layout::subtitle(
                ui,
                "Verify this relay fingerprint through a trusted source before accepting it.",
                palette,
            );
            ui.add(egui::Label::new(RichText::new(&info.fingerprint).monospace()).wrap());
            ui.horizontal_wrapped(|ui| {
                if ui.button("Copy fingerprint").clicked() {
                    ctx.copy_text(info.fingerprint.clone());
                }
                ui.add_enabled_ui(!busy, |ui| {
                    let label = if info.spec.relay_key.is_empty() {
                        "Trust relay key and start"
                    } else {
                        "Replace trusted relay key and restart"
                    };
                    if ui.button(label).clicked() {
                        let mut spec = info.spec.clone();
                        spec.relay_key = info.observed_key.clone();
                        self.tunnel_request(
                            Control::StartTunnel {
                                id: info.id.clone(),
                                spec,
                            },
                            ctx,
                        );
                    }
                });
            });
        }
        ui.add_space(12.);
        let local = self.workspace.as_ref().is_none_or(|w| w.remote.is_none());
        if local && info.status == Status::Connected {
            layout::subtitle(
                ui,
                "Use the connection details in Connect SSH on your other computer. Verify this computer’s SSH host key when connecting.",
                palette,
            );
        }
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(!busy && info.status == Status::Connected, |ui| {
                if local {
                    if palette.primary(ui, "Copy connection details").clicked() {
                        ctx.copy_text(self.tunnels.connection_details(info));
                    }
                } else if palette
                    .primary(ui, "Configure return route")
                    .on_hover_text("Open Connect SSH with this route, then save the profile.")
                    .clicked()
                {
                    self.tunnel_route(info, ctx);
                }
                if !local
                    && info.spec.listen_on == ListenOn::AllInterfaces
                    && ui.button("Copy connection details").clicked()
                {
                    ctx.copy_text(self.tunnels.connection_details(info));
                }
            });
            if info.status == Status::Attention
                && ui.add_enabled(!busy, egui::Button::new("Retry")).clicked()
            {
                self.tunnel_request(
                    Control::StartTunnel {
                        id: info.id.clone(),
                        spec: info.spec.clone(),
                    },
                    ctx,
                );
            }
            if ui.button("Copy settings").clicked() {
                self.tunnels.spec = info.spec.clone();
                self.tunnels.saved_id = None;
                self.tunnels.draft_epoch += 1;
                self.tunnels.page = Page::New;
            }
            if ui
                .add_enabled(
                    self.tunnels.saved.error.is_none(),
                    egui::Button::new(self.tunnel_save_label(&info.spec, false)),
                )
                .clicked()
            {
                self.save_tunnel_configuration(info.spec.clone(), false);
            }
            if ui
                .add_enabled(
                    !busy,
                    egui::Button::new(RichText::new("Stop…").color(palette.error)),
                )
                .clicked()
            {
                self.tunnels.stop = Some(info.id.clone());
            }
        });
        if self.tunnels.stop.as_deref() == Some(&info.id) {
            ui.add_space(8.);
            ui.separator();
            ui.label("Stopping closes all connections through this tunnel, including this workspace if it uses this route.");
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        !busy,
                        egui::Button::new(RichText::new("Stop tunnel").color(palette.error)),
                    )
                    .clicked()
                {
                    self.tunnel_request(
                        Control::StopTunnel {
                            id: info.id.clone(),
                        },
                        ctx,
                    );
                    self.tunnels.stop = None;
                }
                if ui.button("Cancel").clicked() {
                    self.tunnels.stop = None;
                }
            });
        }
    }

    fn tunnel_compute_host(&self, info: &Info) -> String {
        if !info.hostname.is_empty() {
            return info.hostname.clone();
        }
        // Older services do not report a hostname. Use the selected SSH host or
        // alias remotely; never substitute the desktop's name for a remote host.
        if let Some(remote) = self.workspace.as_ref().and_then(|w| w.remote.as_ref()) {
            return remote
                .profile
                .host
                .rsplit('@')
                .next()
                .unwrap_or(&remote.profile.host)
                .to_owned();
        }
        static LOCAL_HOST: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
        LOCAL_HOST
            .get_or_init(rode_session::tunnels::local_hostname)
            .clone()
            .unwrap_or_else(|| "This computer (hostname unavailable)".into())
    }

    fn tunnel_form_ui(&mut self, ui: &mut egui::Ui, palette: Palette) {
        let local = self.workspace.as_ref().is_none_or(|w| w.remote.is_none());
        ui.push_id(("tunnel-draft", self.tunnels.draft_epoch), |ui| {
            let spec = &mut self.tunnels.spec;
            layout::section(ui, "Relay connection", palette, |ui| {
                form_grid(ui, "relay-fields", |ui, compact, width| {
                    text_field(ui, "Name", &mut spec.name, "Compute tunnel", compact, width);
                    text_field(ui, "Relay host", &mut spec.relay_host, "login.example.org", compact, width);
                    text_field(ui, "Username", &mut spec.relay_user, "Your relay login", compact, width);
                    port_field(ui, "SSH port", &mut spec.relay_port, compact);
                });
            });
            layout::section(ui, "Port forwarding", palette, |ui| {
                form_grid(ui, "forwarding-fields", |ui, compact, _| {
                    port_field(ui, "Relay port", &mut spec.listen_port, compact);
                    layout::field_label(ui, "Relay access", compact);
                    let access = egui::ComboBox::from_id_salt("tunnel-listen-on")
                        .selected_text(spec.listen_on.label())
                        .show_ui(ui, |ui| {
                            for scope in [ListenOn::Loopback, ListenOn::AllInterfaces] {
                                let option = ui.selectable_value(&mut spec.listen_on, scope, scope.label());
                                ui_probe::button(ui, if scope == ListenOn::Loopback { "tunnel-listen-loopback" } else { "tunnel-listen-all" }, &option);
                            }
                        });
                    ui_probe::button(ui, "tunnel-listen-on", &access.response);
                    ui.end_row();
                    port_field(ui, "Compute SSH port", &mut spec.ssh_port, compact);
                });
                layout::subtitle(ui, if local { "This computer must already run sshd." } else { "The compute node must already run sshd." }, palette);
                layout::subtitle(ui, if spec.listen_on == ListenOn::Loopback { "Requests a listener on the relay's loopback interface." } else { "Requests access to this host's SSH server from other devices. Configure GatewayPorts clientspecified on the relay and allow the listener port through its firewall." }, palette);
            });
            layout::section(ui, if local { "Credentials on this computer" } else { "Credentials on the compute host" }, palette, |ui| {
                form_grid(ui, "credential-fields", |ui, compact, width| {
                    text_field(ui, "Identity file", &mut spec.identity, "Automatic SSH key files", compact, width);
                    text_field(ui, "Agent socket", &mut spec.agent_socket, "Persistent SSH-agent socket path", compact, width);
                });
                layout::subtitle(ui, if local { "Leave both blank to try default SSH keys on this computer. Set a key path to override, or an agent socket to use only that agent." } else { "Leave both blank to try default SSH keys on the compute host. Set a key path to override, or an agent socket to use only that agent." }, palette);
                if !spec.relay_key.is_empty() {
                    ui.add_space(4.);
                    layout::subtitle(ui, "This setup retains the verified relay key.", palette);
                }
            });
            ui.collapsing("Connection requirements", |ui| {
                ui.label("Use a literal relay hostname or IP. The relay must allow SSH forwarding, and the compute node must be able to reach it.");
                ui.label("Default key files: ~/.ssh/id_rsa, ~/.ssh/id_ecdsa, then ~/.ssh/id_ed25519. Encrypted or hardware-backed keys require a persistent SSH agent.");
                ui.label("Desktop credentials are not copied. A forwarded desktop agent disappears when that connection ends; use an agent owned by the compute host.");
                ui.label("The service reconnects after network loss. Tunnels end when stopped or when the service, node or allocation ends.");
            });
        });
    }

    fn tunnel_footer(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        page: Page,
        palette: Palette,
    ) {
        let validation = self.tunnels.spec.validate();
        if page == Page::New {
            let hint = if self.tunnels.spec.relay_host.is_empty()
                || self.tunnels.spec.relay_user.is_empty()
            {
                "Enter the relay host and username.".to_owned()
            } else if let Err(error) = &validation {
                error.to_string()
            } else if !self.tunnels.spec.relay_key.is_empty() {
                "The relay must match the saved host key.".to_owned()
            } else {
                "Next: verify the relay’s host key.".to_owned()
            };
            layout::subtitle(ui, &hint, palette);
        } else if page == Page::Tunnels {
            layout::subtitle(
                ui,
                "Tunnels keep running when you close this window.",
                palette,
            );
        } else {
            layout::subtitle(ui, "Loading a saved setup opens it for review.", palette);
        }
        ui.add_space(6.);
        ui.horizontal_wrapped(|ui| {
            if page == Page::New {
                ui.add_enabled_ui(validation.is_ok() && self.tunnels.pending.is_none(), |ui| {
                    let start = palette.primary(ui, "Start tunnel");
                    ui_probe::button(ui, "tunnel-start", &start);
                    if start.clicked() {
                        let spec = self.tunnels.spec.clone();
                        match rode_session::new_id() {
                            Ok(id) => {
                                self.tunnel_request(Control::StartTunnel { id, spec }, ctx);
                                self.tunnels.page = Page::Tunnels;
                            }
                            Err(error) => self.tunnels.error = error.to_string(),
                        }
                    }
                });
                let save = ui.add_enabled(
                    validation.is_ok() && self.tunnels.saved.error.is_none(),
                    egui::Button::new(self.tunnel_save_label(&self.tunnels.spec, true)),
                );
                ui_probe::button(ui, "tunnel-save", &save);
                if save.clicked() {
                    self.save_tunnel_configuration(self.tunnels.spec.clone(), true);
                }
            }
            let close = ui.button("Close");
            ui_probe::button(ui, "tunnel-close", &close);
            if close.clicked() {
                self.dialog = Dialog::None;
                ctx.request_repaint();
            }
            if self.tunnels.pending.is_some() {
                ui.label(RichText::new("Updating…").small().color(palette.muted));
            }
        });
    }
}

fn tunnel_error(ui: &mut egui::Ui, ctx: &egui::Context, error: &str, color: egui::Color32) {
    ui.add(
        egui::Label::new(RichText::new(error).color(color))
            .wrap()
            .selectable(true),
    );
    if ui.small_button("Copy error").clicked() {
        ctx.copy_text(error.to_owned());
    }
}

fn endpoint(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn form_grid(ui: &mut egui::Ui, id: &str, fields: impl FnOnce(&mut egui::Ui, bool, f32)) {
    let compact = ui.available_width() < 460.;
    let width = (ui.available_width() - if compact { 0. } else { 146. }).max(1.);
    egui::Grid::new(id)
        .num_columns(if compact { 1 } else { 2 })
        .spacing([18., 8.])
        .min_col_width(0.)
        .max_col_width(width)
        .show(ui, |ui| fields(ui, compact, width));
}
fn text_field(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut String,
    hint: &str,
    compact: bool,
    width: f32,
) {
    layout::field_label(ui, label, compact);
    let field = ui.add(
        egui::TextEdit::singleline(value)
            .id_salt(label)
            .hint_text(hint)
            .char_limit(4096)
            .desired_width(width),
    );
    ui_probe::button(ui, &format!("tunnel-field-{label}"), &field);
    ui.end_row();
}
fn port_field(ui: &mut egui::Ui, label: &str, value: &mut u16, compact: bool) {
    layout::field_label(ui, label, compact);
    ui.add(egui::DragValue::new(value).range(1..=65535));
    ui.end_row();
}
