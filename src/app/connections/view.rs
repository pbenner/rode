//! Connections landing page. Saved SSH profiles and tunnel setups reuse their
//! existing catalogues and guarded actions in both root and detached viewports.
use super::*;

impl EditorApp {
    pub(in crate::app) fn connections_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        layout::header(
            ui,
            "Connections",
            "Your SSH hosts and saved tunnel setups",
            palette,
            |ui| {
                let connection = ui.add(IconButton::new(Icon::Remote, "New SSH connection…"));
                ui_probe::button(ui, "connections-new-ssh", &connection);
                if connection.clicked() {
                    self.set_dialog(Dialog::Connect);
                }
                let tunnel = crate::keyboard::button(ui, "New tunnel…");
                ui_probe::button(ui, "connections-new-tunnel", &tunnel);
                if tunnel.clicked() {
                    self.new_tunnel_dialog();
                }
            },
        );
        ui.add_space(12.);
        self.connection_host_card(ui, ctx);
        ui.add_space(18.);
        // Each catalogue has a useful reading width. Compact/detached windows
        // stack the same sections; there is only one outer scrolling region.
        if ui.available_width() >= 760. {
            let spacing = ui.spacing().item_spacing.x;
            ui.spacing_mut().item_spacing.x = 20.;
            ui.columns(2, |columns| {
                for column in columns.iter_mut() {
                    column.spacing_mut().item_spacing.x = spacing;
                }
                self.connection_profiles_section(&mut columns[0], ctx);
                self.connection_tunnels_section(&mut columns[1]);
            });
            ui.spacing_mut().item_spacing.x = spacing;
        } else {
            self.connection_profiles_section(ui, ctx);
            ui.add_space(14.);
            self.connection_tunnels_section(ui);
        }
    }

    fn connection_host_card(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        let remote = self
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.remote.as_ref());
        let label = remote.map_or_else(
            || "This computer".to_owned(),
            |remote| remote.profile.endpoint(),
        );
        let is_remote = remote.is_some();
        let connected = self
            .workspace
            .as_ref()
            .is_some_and(|workspace| workspace.connected);
        let connecting = !connected && self.ssh_master.as_ref().is_some_and(Terminal::is_alive);
        let preparing = connected
            && self
                .workspace
                .as_ref()
                .is_some_and(|workspace| !workspace.ready);
        let folder = self
            .workspace
            .as_ref()
            .map(|workspace| workspace.root.clone());
        layout::card(ui, palette, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong(&label);
                let (state, color) = if !is_remote {
                    ("LOCAL", palette.muted)
                } else if connecting {
                    ("CONNECTING", palette.accent)
                } else if preparing {
                    ("PREPARING", palette.accent)
                } else if connected {
                    ("CONNECTED", palette.success)
                } else {
                    ("OFFLINE", palette.error)
                };
                ui.label(palette.tag(state).color(color));
            });
            if let Some(folder) = folder.filter(|folder| !folder.is_empty()) {
                ui.add(egui::Label::new(&folder).truncate())
                    .on_hover_text(&folder);
            } else {
                ui.weak("No project folder open");
            }
            ui.add_space(8.);
            ui.horizontal_wrapped(|ui| {
                let tunnels = self.tunnel_status_button(ui, palette);
                ui_probe::button(ui, "connections-manage-tunnels", &tunnels);
                if tunnels.clicked() {
                    self.set_dialog(Dialog::Tunnels);
                }
                if is_remote {
                    if connected || connecting {
                        if crate::keyboard::button(ui, "Disconnect").clicked() {
                            self.disconnect_ssh(ctx);
                        }
                    } else if crate::keyboard::button(ui, "Reconnect").clicked() {
                        self.reconnect_ssh(ctx);
                    }
                }
            });
        });
    }

    fn connection_profiles_section(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        ui.horizontal_wrapped(|ui| {
            ui.strong("SSH connections");
            ui.label(palette.tag(self.connection_profiles.profiles.len().to_string()));
        });
        ui.add_space(8.);
        if self.connection_profiles.profiles.is_empty() {
            layout::empty(
                ui,
                "Save your SSH hosts",
                "Choose New SSH connection, enter your host and account, then Save profile to keep it here.",
                palette,
            );
        } else {
            layout::subtitle(ui, "Connect to a host or edit its profile.", palette);
            ui.add_space(8.);
        }
        self.saved_connections_ui(ui, ctx, saved_connections::View::Details);
    }

    fn connection_tunnels_section(&mut self, ui: &mut egui::Ui) {
        let palette = self.themes.active().palette;
        ui.horizontal_wrapped(|ui| {
            ui.strong("Saved tunnels");
            ui.label(palette.tag(self.saved_tunnel_count().to_string()));
        });
        ui.add_space(8.);
        self.saved_tunnels_ui(ui, palette);
    }
}
