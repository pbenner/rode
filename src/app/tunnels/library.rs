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
                let can_load = saved.host == host;
                let load = ui
                    .add_enabled_ui(can_load, |ui| {
                        ui.add_sized(
                            egui::vec2(ui.available_width(), 32.),
                            IconButton::new(Icon::Remote, &saved.spec.name).truncate(),
                        )
                    })
                    .inner;
                ui_probe::button(ui, &format!("tunnel-saved-load:{id}"), &load);
                // Keep the host guard on loading, while allowing management of
                // a disabled foreign-host row. An enabled load already supplies
                // the row's interaction; only disabled loads need an overlay.
                let row = if can_load {
                    load.clone()
                } else {
                    let row = ui.interact(load.rect, load.id.with("manage"), Sense::click());
                    row.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::Button,
                            ui.is_enabled(),
                            format!("Manage saved tunnel {}", saved.spec.name),
                        )
                    });
                    crate::keyboard::target(
                        ui,
                        &format!("Manage saved tunnel {}", saved.spec.name),
                        &row,
                    );
                    row
                }
                .on_hover_text(format!(
                    "{}\nRelay {} · {}\n{}\nRight-click to manage",
                    saved.host.label(),
                    ui::endpoint(&saved.spec.relay_host, saved.spec.relay_port),
                    saved.spec.listener(),
                    if can_load {
                        "Load and review this setup".into()
                    } else {
                        format!("Open {} before loading this setup.", saved.host.label())
                    },
                ));
                ui_probe::button(ui, &format!("tunnel-saved-row:{id}"), &row);
                if row.has_focus() {
                    row.scroll_to_me(None);
                }
                if load.clicked() {
                    self.open_saved_tunnel(&id);
                }
                crate::keyboard::context_menu(&row, |ui| {
                    let remove = ui.add(
                        IconButton::new(Icon::Close, "Remove saved tunnel").color(palette.error),
                    );
                    ui_probe::button(ui, &format!("tunnel-saved-remove:{id}"), &remove);
                    if remove.clicked() {
                        self.tunnels.delete_saved = Some(id.clone());
                        ui.close();
                    }
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
