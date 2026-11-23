//! Compact Connections library shared by the root sidebar and its detached
//! viewport. Entry actions stay visible while the saved lists scroll together.
use super::*;

impl EditorApp {
    /// Docked and detached Connections share their pane, content and artwork.
    pub(in crate::app) fn connections_content(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut content = self.pane_ui(ui, navigation::Pane::Connections);
        self.connections_ui(&mut content, ctx);
        self.themes
            .active()
            .palette
            .paint_explorer_artwork(&content);
    }

    pub(in crate::app) fn connections_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        let header_size = egui::vec2(ui.available_width(), 24.);
        ui.allocate_ui_with_layout(header_size, Layout::left_to_right(Align::Center), |ui| {
            ui.set_min_size(header_size);
            ui.add(egui::Label::new(palette.tag("CONNECTIONS")).truncate());
        });
        ui.add_space(8.);
        let size = egui::vec2(ui.available_width(), 32.);
        let connection = ui.add_sized(size, IconButton::new(Icon::Remote, "Connect SSH…"));
        ui_probe::button(ui, "connections-new-ssh", &connection);
        if connection.clicked() {
            self.set_dialog(Dialog::Connect);
        }
        let tunnel = ui.add_sized(size, IconButton::new(Icon::Remote, "New tunnel…"));
        ui_probe::button(ui, "connections-new-tunnel", &tunnel);
        if tunnel.clicked() {
            self.new_tunnel_dialog();
        }
        ui.add_space(12.);
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt("connections-library")
            .auto_shrink([false, false])
            .min_scrolled_height(0.)
            .show(ui, |ui| {
                ui.add_space(8.);
                if self.feature(crate::extensions::Feature::ProjectBookmarks) {
                    self.project_bookmarks_section(ui, ctx);
                    ui.separator();
                }
                ui.add_space(12.);
                sidebar::section(ui, "SSH connections", "ssh-connections", |ui| {
                    self.saved_connections_ui(ui, ctx);
                    if self.connection_profiles.profiles.is_empty() {
                        ui.label(palette.tag("Saved SSH profiles appear here."));
                    }
                });
                ui.add_space(12.);
                ui.separator();
                sidebar::section(ui, "Saved tunnels", "saved-tunnels", |ui| {
                    let running = self.tunnel_status_button(ui, palette);
                    ui_probe::button(ui, "connections-manage-tunnels", &running);
                    if running.clicked() {
                        self.set_dialog(Dialog::Tunnels);
                    }
                    self.saved_tunnels_ui(ui, palette);
                });
            });
    }
}
