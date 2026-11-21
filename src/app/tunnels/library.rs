//! Saved-tunnel rows for the Connections sidebar and its detached owner.
//! Loading only fills a host-bound draft; the explicit Start action owns I/O.
use super::*;
use crate::theme::Palette;

impl EditorApp {
    pub(in crate::app) fn saved_tunnels_ui(&mut self, ui: &mut egui::Ui, palette: Palette) {
        if let Some(error) = &self.tunnels.saved.error {
            ui.colored_label(palette.error, "Saved tunnels unavailable")
                .on_hover_text(error);
            return;
        }
        if self.tunnels.saved.entries.is_empty() {
            ui.label(palette.tag("No saved tunnels yet."));
            return;
        }
        let host = self.tunnel_saved_host();
        let mut entries: Vec<_> = self.tunnels.saved.entries.clone().into_iter().collect();
        entries.sort_by(|(_, a), (_, b)| {
            (a.host != host, &a.spec.name).cmp(&(b.host != host, &b.spec.name))
        });
        for (id, saved) in entries {
            ui.push_id(("saved-tunnel", &id), |ui| {
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let remove = ui.add(IconButton::toolbar(
                            Icon::Close,
                            &format!("Remove saved tunnel {}", saved.spec.name),
                        ));
                        ui_probe::button(ui, &format!("tunnel-saved-remove:{id}"), &remove);
                        if remove.clicked() {
                            self.tunnels.delete_saved = Some(id.clone());
                        }
                        let load = ui
                            .add_enabled(
                                saved.host == host,
                                IconButton::new(Icon::Remote, &saved.spec.name)
                                    .truncate()
                                    .min_size(egui::vec2(ui.available_width(), 32.)),
                            )
                            .on_hover_text(format!(
                                "{}\nRelay {} · {}\nLoad and review this setup",
                                saved.host.label(),
                                ui::endpoint(&saved.spec.relay_host, saved.spec.relay_port),
                                saved.spec.listener()
                            ))
                            .on_disabled_hover_text(format!(
                                "Open {} before loading this setup.",
                                saved.host.label()
                            ));
                        ui_probe::button(ui, &format!("tunnel-saved-load:{id}"), &load);
                        if load.has_focus() {
                            load.scroll_to_me(None);
                        }
                        if load.clicked() {
                            self.open_saved_tunnel(&id);
                        }
                    });
                });
                if self.tunnels.delete_saved.as_deref() == Some(&id) {
                    ui.label("Delete this saved configuration? Running tunnels are unaffected.");
                    ui.horizontal_wrapped(|ui| {
                        let delete = crate::keyboard::button(
                            ui,
                            RichText::new("Delete configuration").color(palette.error),
                        );
                        ui_probe::button(ui, &format!("tunnel-saved-confirm-delete:{id}"), &delete);
                        if delete.clicked() {
                            self.delete_tunnel_configuration(&id);
                        }
                        let cancel = crate::keyboard::button(ui, "Cancel");
                        ui_probe::button(ui, &format!("tunnel-saved-cancel-delete:{id}"), &cancel);
                        if cancel.clicked() {
                            self.tunnels.delete_saved = None;
                        }
                    });
                }
            });
        }
    }

    fn open_saved_tunnel(&mut self, id: &str) {
        if self
            .tunnels
            .saved
            .entries
            .get(id)
            .is_some_and(|saved| saved.host == self.tunnel_saved_host())
        {
            self.load_tunnel_configuration(id);
            self.set_dialog(Dialog::Tunnels);
        }
    }

    pub(in crate::app) fn new_tunnel_dialog(&mut self) {
        self.set_dialog(Dialog::Tunnels);
        self.tunnels.page = Page::New;
    }
}
