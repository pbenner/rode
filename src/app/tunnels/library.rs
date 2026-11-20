//! Shared saved-tunnel presentation for Connections, the welcome sidebar and
//! the tunnel dialog. Reading the library never starts host or network work.
use super::*;
use crate::theme::Palette;

impl EditorApp {
    pub(in crate::app) fn saved_tunnels_ui(&mut self, ui: &mut egui::Ui, palette: Palette) {
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
        layout::subtitle(ui, "Load a setup to review it before starting.", palette);
        ui.add_space(8.);
        let host = self.tunnel_saved_host();
        let mut entries: Vec<_> = self.tunnels.saved.entries.clone().into_iter().collect();
        entries.sort_by(|(_, a), (_, b)| {
            (a.host != host, &a.spec.name).cmp(&(b.host != host, &b.spec.name))
        });
        for (id, saved) in entries {
            ui.push_id(("saved-tunnel", &id), |ui| {
                let card = layout::card(ui, palette, |ui| {
                    ui.add(egui::Label::new(RichText::new(&saved.spec.name).strong()).wrap());
                    ui.add(egui::Label::new(saved.host.label()).wrap());
                    layout::subtitle(ui, &format!("Relay {} · listener {} · {}", ui::endpoint(&saved.spec.relay_host, saved.spec.relay_port), saved.spec.listener(), saved.spec.listen_on.label()), palette);
                    ui.horizontal_wrapped(|ui| {
                        let load = ui.add_enabled(saved.host == host, crate::keyboard::Button::new("Load setup"))
                            .on_disabled_hover_text(format!("Open {} before loading this setup.", saved.host.label()));
                        ui_probe::button(ui, &format!("tunnel-saved-load:{id}"), &load);
                        if load.clicked() { self.open_saved_tunnel(&id); }
                        if crate::keyboard::button(ui, "Delete…").clicked() { self.tunnels.delete_saved = Some(id.clone()); }
                    });
                    if self.tunnels.delete_saved.as_deref() == Some(&id) {
                        ui.label("Delete this saved configuration? Running tunnels keep their current settings.");
                        ui.horizontal_wrapped(|ui| {
                            if crate::keyboard::button(ui, RichText::new("Delete configuration").color(palette.error)).clicked() {
                                self.delete_tunnel_configuration(&id);
                            }
                            if crate::keyboard::button(ui, "Cancel").clicked() { self.tunnels.delete_saved = None; }
                        });
                    }
                });
                ui_probe::button(ui, &format!("tunnel-saved-card:{id}"), &card.response);
            });
            ui.add_space(10.);
        }
    }

    /// Compact saved setups beside the welcome screen's SSH profiles. Foreign
    /// hosts open the library with its existing host-bound loading guard.
    pub(in crate::app) fn saved_tunnel_shortcuts_ui(&mut self, ui: &mut egui::Ui) {
        if self.tunnels.saved.entries.is_empty() && self.tunnels.saved.error.is_none() {
            return;
        }
        let palette = self.themes.active().palette;
        ui.add_space(12.);
        ui.separator();
        ui.label(palette.tag("SAVED TUNNELS"));
        if let Some(error) = &self.tunnels.saved.error {
            ui.add(
                egui::Label::new(
                    RichText::new("Saved tunnels unavailable")
                        .small()
                        .color(palette.error),
                )
                .wrap(),
            )
            .on_hover_text(error);
            return;
        }
        for (id, saved) in self.tunnels.saved.entries.clone() {
            let response = ui
                .add_sized(
                    [ui.available_width(), 32.],
                    IconButton::new(Icon::Remote, &saved.spec.name).truncate(),
                )
                .on_hover_text(format!(
                    "{}\n{}\nLoad and review this setup",
                    saved.spec.name,
                    saved.host.label()
                ));
            ui_probe::button(ui, &format!("saved-tunnel-shortcut:{id}"), &response);
            if response.has_focus() {
                response.scroll_to_me(None);
            }
            if response.clicked() {
                self.set_dialog(Dialog::Tunnels);
                self.tunnels.page = Page::Saved;
                if saved.host == self.tunnel_saved_host() {
                    self.load_tunnel_configuration(&id);
                }
            }
        }
    }

    /// Open the existing setup form after the same host check used by the manager.
    /// No service is started until the form's explicit Start action.
    fn open_saved_tunnel(&mut self, id: &str) {
        if self
            .tunnels
            .saved
            .entries
            .get(id)
            .is_some_and(|saved| saved.host == self.tunnel_saved_host())
        {
            self.load_tunnel_configuration(id);
            if self.dialog != Dialog::Tunnels {
                self.set_dialog(Dialog::Tunnels);
            }
        }
    }

    /// Library counts are presentation data, independent of running tunnel status.
    pub(in crate::app) fn saved_tunnel_count(&self) -> usize {
        self.tunnels.saved.entries.len()
    }

    pub(in crate::app) fn new_tunnel_dialog(&mut self) {
        self.set_dialog(Dialog::Tunnels);
        self.tunnels.page = Page::New;
    }
}
