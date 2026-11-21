//! Shared-folder presentation. Rendering edits only its draft and emits explicit
//! intents; mount controls, recipe admission and worker lifetimes stay in Sharing.
use super::super::Side;
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
    /// Resolve the field's host from its direction, never from the project or
    /// whichever transfer pane happened to be selected most recently.
    pub(super) fn folder(&self, field: Field) -> (Side, &str) {
        let source = match self.draft.direction {
            Direction::LocalToRemote => Side::Local,
            Direction::RemoteToLocal => Side::Remote,
        };
        match field {
            Field::Source => (source, &self.draft.source),
            Field::Destination => (source.other(), &self.draft.destination),
        }
    }
    pub(super) fn select_folder(&mut self, field: Field, path: String) {
        if self.adding {
            match field {
                Field::Source => self.draft.source = path,
                Field::Destination => self.draft.destination = path,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_fields_keep_their_direction_and_cancelled_forms_ignore_selections() {
        let mut view = View::default();
        for (direction, source, destination) in [
            (Direction::LocalToRemote, Side::Local, Side::Remote),
            (Direction::RemoteToLocal, Side::Remote, Side::Local),
        ] {
            view.draft.direction = direction;
            assert_eq!(view.folder(Field::Source).0, source);
            assert_eq!(view.folder(Field::Destination).0, destination);
        }
        view.select_folder(Field::Source, "/late".into());
        assert_eq!(view.draft.source, "");
        view.adding = true;
        view.select_folder(Field::Source, "/chosen".into());
        view.select_folder(Field::Destination, "/mount".into());
        assert_eq!(view.draft.source, "/chosen");
        assert_eq!(view.draft.destination, "/mount");
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Field {
    Source,
    Destination,
}
pub(super) enum Intent {
    Browse(Field),
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
            if self.adding
                && let Some(action) = self.form(ui, rows, available)
            {
                intent = Some(action);
            }
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
        });
        intent
    }
}

impl View {
    fn form(&mut self, ui: &mut egui::Ui, rows: &[Row], available: bool) -> Option<Intent> {
        let mut intent = None;
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
        let local_source = self.draft.direction == Direction::LocalToRemote;
        for (field, name, label, local, value) in [
            (
                Field::Source,
                "share-source",
                "Source folder",
                local_source,
                &mut self.draft.source,
            ),
            (
                Field::Destination,
                "share-destination",
                "Mount folder",
                !local_source,
                &mut self.draft.destination,
            ),
        ] {
            ui.add_space(6.);
            ui.horizontal_wrapped(|ui| {
                ui.strong(label);
                ui.weak(if local { "This computer" } else { "SSH host" });
            });
            ui.horizontal(|ui| {
                let browse =
                    ui.add_enabled(local || available, crate::keyboard::Button::new("Browse…"));
                ui_probe::button(ui, &format!("{name}-browse"), &browse);
                if browse.clicked() {
                    intent = Some(Intent::Browse(field));
                }
                let path = ui.add_sized(
                    [ui.available_width().max(1.), theme::control_height(ui)],
                    crate::keyboard::named(
                        label,
                        egui::TextEdit::singleline(value)
                            .desired_width(f32::INFINITY)
                            .hint_text("Choose a folder or enter a path…"),
                    ),
                );
                ui_probe::button(ui, name, &path);
                ui_probe::value(ui, &format!("{name}-value"), &path, value);
            });
        }
        let readonly = crate::keyboard::checkbox(ui, &mut self.draft.read_only, "Read only");
        ui_probe::button(ui, "share-read-only", &readonly);
        ui.weak("Choose an empty mount folder, or create one with Browse. Only the source folder is shared; existing files are never replaced.");
        let valid = self.draft.validate().is_ok()
            && !rows.iter().any(|row| {
                row.config.direction == self.draft.direction
                    && row.config.destination == self.draft.destination
            });
        ui.horizontal(|ui| {
            let add = ui.add_enabled(valid, crate::keyboard::Button::new("Save shared folder"));
            ui_probe::button(ui, "share-save", &add);
            if add.clicked() {
                intent = Some(Intent::Add(self.draft.clone()));
            }
            let cancel = crate::keyboard::button(ui, "Cancel");
            ui_probe::button(ui, "share-cancel", &cancel);
            if cancel.clicked() {
                self.adding = false;
            }
        });
        intent
    }
}
