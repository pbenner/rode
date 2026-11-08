//! Saved connection rows emit explicit intents. Only the background profile writer
//! changes shared storage; list management never starts or stops an SSH connection.
use super::*;
use ssh::ProfileListEdit;

struct DraggedConnection(String);

enum Intent {
    Connect(Profile),
    Edit { profile: Profile, focus_name: bool },
    Change(ProfileListEdit),
}

impl EditorApp {
    pub(super) fn saved_connections_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        let intent = ui
            .add_enabled_ui(!self.profile_saving, |ui| {
                connection_rows(ui, &self.profiles, palette)
            })
            .inner;
        if self.profile_saving {
            ui.label(
                RichText::new("Saving connections…")
                    .small()
                    .color(palette.muted),
            );
        }
        match intent {
            Some(Intent::Connect(profile)) => {
                self.profile_baseline = Some(profile.clone());
                self.profile = profile;
                self.profile_form += 1;
                self.guarded(Action::Remote(self.profile.clone()), ctx);
            }
            Some(Intent::Edit {
                profile,
                focus_name,
            }) => {
                self.profile_baseline = Some(profile.clone());
                self.profile = profile;
                self.profile_form += 1;
                self.set_dialog(Dialog::Connect);
                self.profile_name_focus = focus_name;
            }
            Some(Intent::Change(edit)) => self.edit_saved_connections(edit, ctx),
            None => {}
        }
    }

    fn edit_saved_connections(&mut self, edit: ProfileListEdit, ctx: &egui::Context) {
        if self.profile_saving {
            return;
        }
        // An older picker reload must not restore rows removed or reordered by
        // this write. Opening the picker also defers reloads while a write runs.
        self.profiles_loading = None;
        self.profile_saving = true;
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        self.profile_writer = Some(std::thread::spawn(move || {
            let removed = match &edit {
                ProfileListEdit::Remove(profile) => Some(profile.label()),
                ProfileListEdit::Move { .. } => None,
            };
            let result = ssh::edit_profiles(&edit).map_err(|e| format!("{e:#}"));
            if let Err(error) = &result {
                eprintln!("Saved connections were not updated: {error}");
            }
            let _ = tx.send(Event::ProfilesChanged { result, removed });
            ctx.request_repaint();
        }));
    }
}

fn connection_rows(
    ui: &mut egui::Ui,
    profiles: &[Profile],
    palette: theme::Palette,
) -> Option<Intent> {
    let mut intent = None;
    let size = egui::vec2(ui.available_width(), 32.);
    for (index, profile) in profiles.iter().enumerate() {
        let label = profile.label();
        ui.push_id(("saved-connection", &label, index), |ui| {
            let response = ui
                .add_sized(
                    size,
                    IconButton::new(Icon::Remote, &label)
                        .color(palette.accent)
                        .truncate()
                        .sense(Sense::click_and_drag()),
                )
                .on_hover_text(format!(
                    "{label}\n{}\nDrag to reorder · Right-click to manage",
                    profile.endpoint()
                ));
            ui_probe::button(ui, &format!("saved-connection-{index}"), &response);
            if response.clicked() && !ui.input(|i| i.pointer.primary_down()) {
                intent = Some(Intent::Connect(profile.clone()));
            }
            response.dnd_set_drag_payload(DraggedConnection(label.clone()));
            if let Some(payload) = response.dnd_hover_payload::<DraggedConnection>()
                && payload.0 != label
            {
                let after = ui.input(|i| {
                    i.pointer
                        .hover_pos()
                        .is_some_and(|p| p.y > response.rect.center().y)
                });
                let y = if after {
                    response.rect.bottom()
                } else {
                    response.rect.top()
                };
                ui.painter().line_segment(
                    [
                        egui::pos2(response.rect.left(), y),
                        egui::pos2(response.rect.right(), y),
                    ],
                    Stroke::new(2.0_f32, palette.accent),
                );
                if let Some(payload) = response.dnd_release_payload::<DraggedConnection>() {
                    intent = Some(Intent::Change(ProfileListEdit::Move {
                        label: payload.0.clone(),
                        relative_to: label.clone(),
                        after,
                    }));
                }
            }
            response.context_menu(|ui| {
                if ui
                    .add(IconButton::new(Icon::Settings, "Edit connection…"))
                    .clicked()
                {
                    intent = Some(Intent::Edit {
                        profile: profile.clone(),
                        focus_name: false,
                    });
                    ui.close();
                }
                if ui.add(IconButton::new(Icon::SaveAs, "Rename…")).clicked() {
                    intent = Some(Intent::Edit {
                        profile: profile.clone(),
                        focus_name: true,
                    });
                    ui.close();
                }
                ui.separator();
                for (enabled, target, after, icon, text) in [
                    (
                        index > 0,
                        index.saturating_sub(1),
                        false,
                        Icon::ChevronUp,
                        "Move up",
                    ),
                    (
                        index + 1 < profiles.len(),
                        index + 1,
                        true,
                        Icon::ChevronDown,
                        "Move down",
                    ),
                ] {
                    if ui
                        .add_enabled(enabled, IconButton::new(icon, text))
                        .clicked()
                    {
                        intent = Some(Intent::Change(ProfileListEdit::Move {
                            label: label.clone(),
                            relative_to: profiles[target].label(),
                            after,
                        }));
                        ui.close();
                    }
                }
                ui.separator();
                if ui
                    .add(
                        IconButton::new(Icon::Close, "Remove saved connection")
                            .color(palette.error),
                    )
                    .clicked()
                {
                    intent = Some(Intent::Change(ProfileListEdit::Remove(Box::new(
                        profile.clone(),
                    ))));
                    ui.close();
                }
            });
        });
    }
    intent
}

#[cfg(test)]
mod tests;
