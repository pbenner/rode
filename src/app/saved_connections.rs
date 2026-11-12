//! Saved connection rows emit explicit intents. Only the background profile writer
//! changes shared storage; list management never starts or stops an SSH connection.
use super::*;
use ssh::ProfileListEdit;

mod state;
pub(super) use state::State;
mod form;

/// Form rendering cannot directly replace a connection or start a storage write.
pub(super) enum FormIntent {
    Connect(Box<Profile>),
    Save,
    Cancel,
    Error(String),
}

/// Shared preference results are independent of the current host/project generation.
pub(super) enum Completion {
    Saved {
        result: Result<Vec<Profile>, String>,
        submitted: Box<Profile>,
        form: u64,
    },
    Changed {
        result: Result<Vec<Profile>, String>,
        removed: Option<String>,
    },
}

struct DraggedConnection(String);

enum Intent {
    Connect(Profile),
    Edit { profile: Profile, focus_name: bool },
    Change(ProfileListEdit),
}

impl EditorApp {
    /// Window-wide writes survive project changes; only the form identity can reject a baseline update.
    pub(super) fn profile_completed(&mut self, completion: Completion) {
        let result = match completion {
            Completion::Saved {
                result,
                submitted,
                form,
            } => self
                .connection_profiles
                .finish_save(result, *submitted, form),
            Completion::Changed { result, removed } => {
                self.connection_profiles.finish_edit(result, removed)
            }
        };
        self.profile_notice(result);
    }

    pub(super) fn profile_notice(&mut self, result: Result<&'static str, String>) {
        match result {
            Ok(message) => self.notify(message, false),
            Err(error) => self.notify(error, true),
        }
    }

    /// Render saved connection intents and route connects through the shared dirty-buffer guard.
    /// Disable row edits while a preference write is awaiting acknowledgement.
    pub(super) fn saved_connections_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        let intent = ui
            .add_enabled_ui(!self.connection_profiles.saving(), |ui| {
                connection_rows(ui, &self.connection_profiles.profiles, palette)
            })
            .inner;
        if self.connection_profiles.saving() {
            ui.label(
                RichText::new("Saving connections…")
                    .small()
                    .color(palette.muted),
            );
        }
        match intent {
            Some(Intent::Connect(profile)) => {
                self.connection_profiles.select(profile.clone(), true);
                self.guarded(Action::Remote(profile), ctx);
            }
            Some(Intent::Edit {
                profile,
                focus_name,
            }) => {
                self.connection_profiles.select(profile, true);
                self.set_dialog(Dialog::Connect);
                self.connection_profiles.name_focus = focus_name;
            }
            Some(Intent::Change(edit)) => self.connection_profiles.edit(edit, &self.tx, ctx),
            None => {}
        }
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
