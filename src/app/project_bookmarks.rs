//! A bundled Explorer extension for explicit project bookmarks.
//!
//! The extension preference map is the only persistence owner. UI edits touch
//! individual records so the existing writer can merge concurrent changes.
//! Reopening delegates to the common project/connection guards and preparation.
use super::*;
use crate::extensions::Feature;
mod model;
pub(super) use model::State;
use model::{Bookmark, Project};

const FEATURE: Feature = Feature::ProjectBookmarks;
const SETTING: &str = "projects";

enum Edit {
    Insert(Bookmark),
    Remove(String),
    Rename(String, String),
}

impl EditorApp {
    fn bookmark_settings(&self) -> Option<&serde_json::Value> {
        self.extensions
            .registry
            .preferences
            .get(FEATURE.id())?
            .settings
            .get(SETTING)
    }
    fn sync_bookmarks(&mut self) {
        let raw = self
            .extensions
            .registry
            .preferences
            .get(FEATURE.id())
            .and_then(|p| p.settings.get(SETTING));
        self.project_bookmarks.sync(raw);
    }

    fn bookmark_candidate(&self) -> Option<Bookmark> {
        let workspace = self.workspace.as_ref().filter(|w| w.ready)?;
        if self.workspace.preparing() || self.vaults.has_active() {
            return None;
        }
        let project = if let Some(remote) = &workspace.remote {
            let mut profile = remote.profile.clone();
            profile.root.clone_from(&workspace.root);
            profile.sandbox = workspace.sandbox_options.clone();
            Project::Ssh {
                profile: Box::new(profile),
            }
        } else {
            Project::Local {
                root: workspace.root.clone(),
                sandbox: workspace.sandbox_options.clone(),
            }
        };
        let mut name: String = ssh::display_name(&workspace.root)
            .chars()
            .filter(|c| !c.is_control())
            .collect();
        while name.len() > 120 {
            name.pop();
        }
        if name.trim().is_empty() {
            name = "Project".into();
        }
        Some(Bookmark { name, project })
    }

    /// Patch raw records, retaining fields introduced by newer versions of the extension.
    fn edit_bookmarks(&mut self, edit: Edit) -> anyhow::Result<()> {
        model::decode(self.bookmark_settings())?;
        let mut raw = self
            .bookmark_settings()
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        let map = raw
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("Invalid project bookmarks"))?;
        match edit {
            Edit::Insert(bookmark) => {
                map.insert(bookmark.project.id(), serde_json::to_value(bookmark)?);
            }
            Edit::Remove(id) => {
                map.remove(&id);
            }
            Edit::Rename(id, name) => {
                model::validate_name(&name)?;
                let record = map
                    .get_mut(&id)
                    .and_then(serde_json::Value::as_object_mut)
                    .ok_or_else(|| anyhow::anyhow!("Project bookmark no longer exists"))?;
                record.insert("name".into(), serde_json::Value::String(name));
            }
        }
        model::decode(Some(&raw))?;
        let mut preferences = self.extensions.registry.preferences.clone();
        preferences
            .entry(FEATURE.id().into())
            .or_default()
            .settings
            .insert(SETTING.into(), raw);
        crate::extensions::preferences(&serde_json::to_string(&preferences)?)?;
        self.extensions.registry.preferences = preferences;
        self.extensions.registry.pending = true;
        self.sync_bookmarks();
        Ok(())
    }

    /// Toggle the current validated project location in saved preferences; untitled/disconnected
    /// state cannot become a live-session bookmark.
    pub(super) fn toggle_project_bookmark(&mut self) {
        if !self.feature(FEATURE) {
            self.notify("Enable Project bookmarks in View → Extensions.", false);
            return;
        }
        self.sync_bookmarks();
        let Some(bookmark) = self.bookmark_candidate() else {
            self.notify("Open a local or SSH project before bookmarking. Encrypted projects use Open encrypted project.", false);
            return;
        };
        let id = bookmark.project.id();
        let saved = self.project_bookmarks.projects.contains_key(&id);
        let edit = if saved {
            Edit::Remove(id)
        } else {
            Edit::Insert(bookmark)
        };
        match self.edit_bookmarks(edit) {
            Ok(()) => self.notify(
                if saved {
                    "Project bookmark removed"
                } else {
                    "Project bookmarked"
                },
                false,
            ),
            Err(error) => self.notify(format!("Cannot save project bookmark: {error:#}"), true),
        }
    }

    /// Persisted locations are intents, never proof of a live transport or an active sandbox.
    fn reopen_bookmark(&mut self, bookmark: Bookmark, ctx: &egui::Context) {
        if self
            .bookmark_candidate()
            .is_some_and(|current| current.project.id() == bookmark.project.id())
            && self.project_ready()
        {
            return;
        }
        match bookmark.project {
            Project::Local { root, sandbox } => {
                let options = self.saved_project_sandbox(&root, None).unwrap_or(sandbox);
                self.guarded(
                    Action::Project {
                        path: root,
                        remote: false,
                        options,
                    },
                    ctx,
                );
            }
            Project::Ssh { mut profile } => {
                if let Some(options) = self.saved_project_sandbox(&profile.root, Some(&profile)) {
                    profile.sandbox = options;
                }
                let same_host = self
                    .workspace
                    .as_ref()
                    .filter(|w| w.connected)
                    .and_then(|w| w.remote.as_ref())
                    .is_some_and(|remote| Project::same_connection(&remote.profile, &profile));
                if same_host {
                    self.guarded(
                        Action::Project {
                            path: profile.root,
                            remote: true,
                            options: profile.sandbox,
                        },
                        ctx,
                    );
                } else {
                    self.guarded(Action::Remote(profile), ctx);
                }
            }
        }
    }

    /// Render the current project’s bookmark toggle using the same stored catalogue and mutation
    /// path as the sidebar list.
    pub(super) fn project_bookmark_button(&mut self, ui: &mut egui::Ui) {
        if !self.feature(FEATURE) {
            return;
        }
        self.sync_bookmarks();
        let candidate = self.bookmark_candidate();
        let saved = candidate.as_ref().is_some_and(|b| {
            self.project_bookmarks
                .projects
                .contains_key(&b.project.id())
        });
        let enabled = candidate.is_some()
            && self.project_bookmarks.error.is_none()
            && self.dialog == Dialog::None
            && !self.pending.modal();
        let button = ui.add_enabled(
            enabled,
            IconButton::icon(
                Icon::Bookmark,
                if saved {
                    "Remove project bookmark"
                } else {
                    "Bookmark current project"
                },
            )
            .selected(saved),
        );
        let button = button.on_disabled_hover_text(if self.vaults.has_active() {
            "Reopen encrypted projects through Open encrypted project."
        } else {
            "Open a project and wait for it to be ready."
        });
        ui_probe::button(ui, "bookmark-project", &button);
        ui_probe::button(
            ui,
            if saved {
                "project-bookmarked"
            } else {
                "project-not-bookmarked"
            },
            &button,
        );
        if button.clicked() {
            self.toggle_project_bookmark();
        }
    }

    /// Render saved local/SSH locations and their management actions in Explorer. Reopening uses
    /// normal guarded project/connection transitions.
    pub(super) fn project_bookmarks_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if !self.feature(FEATURE) {
            return;
        }
        self.sync_bookmarks();
        let palette = self.themes.active().palette;
        let mut open = None;
        let mut edit = None;
        let mut rename = None;
        egui::CollapsingHeader::new("Saved projects").id_salt("project-bookmarks").default_open(true).show(ui, |ui| {
            if self.foreign_modal(ctx) || self.dialog != Dialog::None || self.pending.modal() || self.exit_pending() { ui.disable(); }
            if let Some(error) = &self.project_bookmarks.error {
                ui.colored_label(palette.error, error);
                ui.small("Existing bookmarks were preserved. Correct the project-bookmarks extension settings to edit this list.");
                return;
            }
            if self.project_bookmarks.projects.is_empty() {
                ui.label(palette.tag("Bookmark a project using the icon above."));
            }
            let current = self.bookmark_candidate().map(|b| b.project.id());
            let mut projects: Vec<_> = self.project_bookmarks.projects.iter().collect();
            projects.sort_by(|(a_id, a), (b_id, b)| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a_id.cmp(b_id)));
            egui::ScrollArea::vertical().id_salt("saved-project-list").max_height((ui.ctx().content_rect().height() * 0.25).clamp(64., 220.)).auto_shrink([false, true]).show(ui, |ui| {
                for (id, bookmark) in projects {
                    let label = format!("{} · {}", bookmark.name, bookmark.project.host());
                    let response = ui.add_sized([ui.available_width(), 30.], IconButton::new(if matches!(bookmark.project, Project::Local { .. }) { Icon::Folder } else { Icon::Remote }, &label).selected(current.as_ref() == Some(id)).truncate()).on_hover_text(format!("{}\n{}\nRight-click to rename or remove", bookmark.project.host(), bookmark.project.root()));
                    ui_probe::button(ui, &format!("saved-project:{id}"), &response);
                    if response.clicked() { open = Some(bookmark.clone()); }
                    response.context_menu(|ui| {
                        let change_name = ui.button("Rename bookmark…");
                        ui_probe::button(ui, &format!("bookmark-rename:{id}"), &change_name);
                        if change_name.clicked() { rename = Some((id.clone(), bookmark.name.clone())); ui.close(); }
                        let remove = ui.button(RichText::new("Remove bookmark").color(palette.error));
                        ui_probe::button(ui, &format!("bookmark-remove:{id}"), &remove);
                        if remove.clicked() { edit = Some(Edit::Remove(id.clone())); ui.close(); }
                    });
                }
            });
            if let Some((id, name)) = &mut self.project_bookmarks.rename {
                let field = ui.add(egui::TextEdit::singleline(name).hint_text("Project name").desired_width(ui.available_width()));
                ui_probe::button(ui, "bookmark-name", &field);
                ui_probe::value(ui, "bookmark-name-value", &field, name);
                if std::mem::take(&mut self.project_bookmarks.rename_focus) { field.request_focus(); }
                let enter = (field.has_focus() || field.lost_focus()) && ui.input(|i| i.key_pressed(Key::Enter));
                let mut cancel = false;
                ui.horizontal(|ui| {
                    let save = ui.add_enabled(model::validate_name(name).is_ok(), egui::Button::new("Save name"));
                    ui_probe::button(ui, "bookmark-name-save", &save);
                    if save.clicked() || (enter && model::validate_name(name).is_ok()) { edit = Some(Edit::Rename(id.clone(), name.trim().into())); }
                    cancel = ui.button("Cancel").clicked();
                });
                if cancel { self.project_bookmarks.rename = None; }
            }
        });
        if let Some(form) = rename {
            self.project_bookmarks.rename = Some(form);
            self.project_bookmarks.rename_focus = true;
        }
        if let Some(edit) = edit {
            match self.edit_bookmarks(edit) {
                Ok(()) => self.project_bookmarks.rename = None,
                Err(error) => self.notify(format!("Cannot save project bookmark: {error:#}"), true),
            }
        }
        if let Some(bookmark) = open {
            self.reopen_bookmark(bookmark, ctx);
        }
        ui.separator();
    }
}

#[cfg(test)]
mod tests;
