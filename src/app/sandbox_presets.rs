//! Shared preset controls for local projects, SSH forms and project settings.
//!
//! Saving affects only the desktop's reusable library. Loading replaces a form
//! draft; the existing explicit open/connect/apply action still prepares and
//! probes the policy on its destination host.
use super::*;
use crate::sandbox::presets::{self, Presets};

#[cfg(test)]
mod tests;

#[derive(Default)]
pub(super) struct State {
    entries: Presets,
    pending: bool,
    load_error: Option<String>,
}

/// Each form keeps its own selection, name and feedback, while sharing saved policies.
#[derive(Clone, Default)]
struct Draft {
    selected: String,
    name: String,
    feedback: Option<(String, bool)>,
}

impl State {
    pub(super) fn load(storage: Option<&dyn eframe::Storage>, warnings: &mut Vec<String>) -> Self {
        let Some(text) = storage.and_then(|s| s.get_string(presets::STORAGE_KEY)) else {
            return Self::default();
        };
        match presets::decode(&text) {
            Ok(entries) => Self {
                entries,
                ..Default::default()
            },
            Err(error) => {
                let error = format!(
                    "Sandbox presets could not be loaded; saved data is preserved: {error:#}"
                );
                warnings.push(error.clone());
                Self {
                    load_error: Some(error),
                    ..Default::default()
                }
            }
        }
    }

    /// Queue only explicit library edits; an idle or malformed window never
    /// replaces saved presets with its initial/empty snapshot.
    pub(super) fn store(&mut self, storage: &mut dyn eframe::Storage) {
        if self.pending {
            storage.set_string(
                presets::STORAGE_KEY,
                serde_json::to_string(&self.entries).expect("serializable presets"),
            );
            self.pending = false;
        }
    }

    fn save(&mut self, name: &str, options: &sandbox::Options) -> anyhow::Result<()> {
        let mut next = self.entries.clone();
        next.insert(name.into(), options.clone());
        presets::decode(&serde_json::to_string(&next)?)?;
        if self.entries != next {
            self.entries = next;
            self.pending = true;
        }
        Ok(())
    }

    pub(super) fn ui(
        &mut self,
        ui: &mut egui::Ui,
        options: &mut sandbox::Options,
        palette: theme::Palette,
    ) {
        ui.collapsing("Reusable sandbox presets", |ui| {
            if let Some(error) = &self.load_error {
                ui.colored_label(palette.error, error);
                return;
            }
            ui.small("Reuse rules for your shell, tools and caches. ~/ refers to the project host's home. Loading changes this form; apply/open/connect to activate it.");
            let id = ui.id().with("sandbox-preset-draft");
            let mut draft = ui.data_mut(|data| data.get_temp::<Draft>(id)).unwrap_or_default();
            ui.horizontal_wrapped(|ui| {
                egui::ComboBox::from_id_salt("sandbox-preset")
                    .width(180.)
                    .truncate()
                    .selected_text(if draft.selected.is_empty() { "Choose preset…" } else { &draft.selected })
                    .show_ui(ui, |ui| {
                        for name in self.entries.keys() {
                            if ui.selectable_value(&mut draft.selected, name.clone(), name).changed() {
                                draft.name = name.clone();
                                draft.feedback = None;
                            }
                        }
                    });
                let selected = self.entries.get(&draft.selected);
                if ui.add_enabled(selected.is_some(), egui::Button::new("Load into form")).clicked() {
                    *options = selected.unwrap().clone();
                    draft.feedback = Some(("Preset loaded into this form. Review the rules before applying.".into(), false));
                }
                if ui.add_enabled(selected.is_some(), egui::Button::new("Delete preset")).clicked() {
                    self.entries.remove(&draft.selected);
                    self.pending = true;
                    draft.selected.clear();
                    draft.name.clear();
                    draft.feedback = Some(("Preset deleted. Current project rules are unchanged.".into(), false));
                }
            });
            ui.horizontal_wrapped(|ui| {
                ui.add(egui::TextEdit::singleline(&mut draft.name).desired_width(180.).hint_text("Preset name").char_limit(128));
                let name = draft.name.trim();
                let label = if self.entries.contains_key(name) { "Replace saved preset" } else { "Save current rules" };
                if ui.add_enabled(!name.is_empty(), egui::Button::new(label)).clicked() {
                    draft.feedback = Some(match self.save(name, options) {
                        Ok(()) => {
                            draft.selected = name.into();
                            ("Preset saved for reuse in local and SSH projects.".into(), false)
                        }
                        Err(error) => (format!("Cannot save preset: {error:#}"), true),
                    });
                }
            });
            if let Some((text, error)) = &draft.feedback {
                ui.colored_label(if *error { palette.error } else { palette.muted }, text);
            }
            ui.data_mut(|data| data.insert_temp(id, draft));
        });
    }
}
