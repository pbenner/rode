//! Shared-folder presentation. Rendering edits only its draft and emits explicit
//! intents; mount controls, recipe admission and worker lifetimes stay in Sharing.
use super::Row;
use crate::{
    app::ui_probe,
    shared_folders::{Config, Direction},
    theme,
};
use eframe::egui::{self, RichText};

#[derive(Default)]
pub(super) struct View {
    draft: Config,
    adding: bool,
}
impl View {
    pub(super) fn saved(&mut self) {
        *self = Self::default();
    }
}
pub(super) enum Intent {
    Add(Config),
    Remove(u64),
    Mount(u64),
    Stop(u64),
}
impl View {
    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        rows: &[Row],
        palette: theme::Palette,
        available: bool,
    ) -> Option<Intent> {
        let mut intent = None;
        super::super::ui::surface(palette).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong("Shared folders");
                let add = ui.add_enabled(
                    rows.len() < 8,
                    crate::keyboard::Button::new("Add shared folder"),
                );
                ui_probe::button(ui, "share-add", &add);
                if add.clicked() {
                    self.adding = true;
                }
            });
            ui.weak("Live access · SSHFS and FUSE are needed on the receiving host");
            for row in rows {
                ui.push_id(row.id, |ui| {
                    ui.separator();
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(match row.config.direction {
                            Direction::LocalToRemote => "Local → SSH host",
                            Direction::RemoteToLocal => "SSH host → Local",
                        });
                        ui.weak(if row.config.read_only {
                            "Read only"
                        } else {
                            "Read and write"
                        });
                        if row.running.is_some() {
                            if !row.ready || row.unmounting {
                                ui.spinner();
                            }
                            let button = ui.add_enabled(
                                !row.unmounting,
                                crate::keyboard::Button::new(if row.ready {
                                    "Unmount"
                                } else {
                                    "Cancel mount"
                                }),
                            );
                            ui_probe::button(ui, &format!("share-unmount:{}", row.id), &button);
                            if button.clicked() {
                                intent = Some(Intent::Stop(row.id));
                            }
                        } else {
                            let button =
                                ui.add_enabled(available, crate::keyboard::Button::new("Mount"));
                            ui_probe::button(ui, &format!("share-mount:{}", row.id), &button);
                            if button.clicked() {
                                intent = Some(Intent::Mount(row.id));
                            }
                            let button = crate::keyboard::button(ui, "Remove");
                            ui_probe::button(ui, &format!("share-remove:{}", row.id), &button);
                            if button.clicked() {
                                intent = Some(Intent::Remove(row.id));
                            }
                        }
                    });
                    ui.add(
                        egui::Label::new(format!(
                            "{} → {}",
                            row.config.source, row.config.destination
                        ))
                        .wrap(),
                    );
                    let color = if row.ready && row.message == "Mounted" {
                        palette.success
                    } else if !row.unmounting
                        && row.message != "Not mounted"
                        && (row.running.is_none() || row.ready)
                    {
                        palette.error
                    } else {
                        palette.muted
                    };
                    let message =
                        ui.add(egui::Label::new(RichText::new(&row.message).color(color)).wrap());
                    ui_probe::value(
                        ui,
                        &format!("share-status:{}", row.id),
                        &message,
                        &row.message,
                    );
                    if row.ready {
                        ui_probe::button(ui, &format!("share-ready:{}", row.id), &message);
                        if !row.unmounting && row.message != "Mounted" {
                            ui_probe::button(
                                ui,
                                &format!("share-unmount-error:{}", row.id),
                                &message,
                            );
                        }
                    } else if row.running.is_none() && row.message != "Not mounted" {
                        ui_probe::button(ui, &format!("share-error:{}", row.id), &message);
                    }
                });
            }
            if self.adding
                && let Some(config) = self.form(ui, rows)
            {
                intent = Some(Intent::Add(config));
            }
        });
        intent
    }
}

impl View {
    fn form(&mut self, ui: &mut egui::Ui, rows: &[Row]) -> Option<Config> {
        let mut config = None;
        ui.separator();
        let direction = crate::keyboard::ComboBox::new("share-direction", "Share direction")
            .selected_text(match self.draft.direction {
                Direction::LocalToRemote => "Local folder on SSH host",
                Direction::RemoteToLocal => "SSH folder on this computer",
            })
            .show_ui(ui, |ui| {
                for (direction, label, name) in [
                    (
                        Direction::LocalToRemote,
                        "Local folder on SSH host",
                        "share-local-to-remote",
                    ),
                    (
                        Direction::RemoteToLocal,
                        "SSH folder on this computer",
                        "share-remote-to-local",
                    ),
                ] {
                    let choice = crate::keyboard::selectable_value(
                        ui,
                        &mut self.draft.direction,
                        direction,
                        label,
                    );
                    ui_probe::button(ui, name, &choice);
                }
            });
        ui_probe::button(ui, "share-direction", &direction.response);
        for (name, label, value) in [
            ("share-source", "Source folder", &mut self.draft.source),
            (
                "share-destination",
                "Empty mount folder",
                &mut self.draft.destination,
            ),
        ] {
            ui.label(label);
            let field = ui.add(crate::keyboard::named(
                label,
                egui::TextEdit::singleline(value)
                    .desired_width(ui.available_width())
                    .hint_text("/absolute/path or ~/folder"),
            ));
            ui_probe::button(ui, name, &field);
            ui_probe::value(ui, &format!("{name}-value"), &field, value);
        }
        let readonly = crate::keyboard::checkbox(ui, &mut self.draft.read_only, "Read only");
        ui_probe::button(ui, "share-read-only", &readonly);
        ui.weak("Create an empty mount folder first. Only the selected source is shared. Sandboxed programs still need permission to access the mount path.");
        let valid = self.draft.validate().is_ok()
            && !rows.iter().any(|row| {
                row.config.direction == self.draft.direction
                    && row.config.destination == self.draft.destination
            });
        ui.horizontal(|ui| {
            let add = ui.add_enabled(valid, crate::keyboard::Button::new("Save shared folder"));
            ui_probe::button(ui, "share-save", &add);
            if add.clicked() {
                config = Some(self.draft.clone());
            }
            let cancel = crate::keyboard::button(ui, "Cancel");
            ui_probe::button(ui, "share-cancel", &cancel);
            if cancel.clicked() {
                self.adding = false;
            }
        });
        config
    }
}
